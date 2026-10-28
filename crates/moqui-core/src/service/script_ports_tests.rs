/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The framework services that were Groovy and are JavaScript now (`WikiServices`, `InstanceServices`, `verify#ReCaptcha`),
//! with the script facade calls they brought: `ec.service.rest()`, the wiki's ResourceReference methods and dbresource
//! versions, admin SQL on a conf connection, and the registry token command.
//!
//! The HTTP calls go to a server on a loopback port, not to an installed sender: the sender slot is process-wide and
//! other tests send real requests through the default client.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::driver::{ConfDb, DbCancel, DbConnection, DbDriver, DbRow};
use crate::entity::EntityFacade;
use crate::service::ServicePools;

fn setup() -> (EntityFacade, ExecutionContext) {
    let facade = crate::testing::open();
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), crate::context::framework_dir());
    context.factory().set_entity(facade.clone());
    (facade, context)
}

fn install(files: &[&str]) {
    let dir = crate::context::framework_dir().join("service/org/moqui");
    let paths: Vec<_> = files.iter().map(|file| dir.join(file)).collect();
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::string(*value)))
        .collect()
}

fn text(map: &BTreeMap<String, Value>, name: &str) -> String {
    map.get(name).map(to_plain_string).unwrap_or_default()
}

fn errors(context: &ExecutionContext) -> String {
    context.message().errors().join("\n")
}

fn messages(context: &ExecutionContext) -> String {
    context.message().messages().join("\n")
}

fn seed(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn field(facade: &EntityFacade, entity: &str, pk: (&str, &str), name: &str) -> String {
    facade
        .find(entity)
        .unwrap()
        .eq(pk.0, Value::string(pk.1))
        .one()
        .unwrap()
        .and_then(|row| row.get(name).map(to_plain_string))
        .unwrap_or_default()
}

/// Answer one connection per reply, in order, and return each request as text. A request that does not arrive
/// within ten seconds ends the server.
fn serve(listener: TcpListener, replies: Vec<(u16, &'static str, &'static str)>) -> thread::JoinHandle<Vec<String>> {
    thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut requests = Vec::new();
        for (status, reason, body) in replies {
            let started = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() > Duration::from_secs(10) {
                            return requests;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return requests,
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0_u8; 4096];
            loop {
                if let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + length {
                        break;
                    }
                }
                match stream.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => buf.extend_from_slice(&tmp[..size]),
                }
            }
            let header = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            requests.push(String::from_utf8_lossy(&buf).into_owned());
        }
        requests
    })
}

fn loopback() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn the_rest_client_sends_what_its_builder_says_and_reads_the_reply() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let (listener, port) = loopback();
    let server = serve(listener, vec![(201, "Created", r#"{"Id":"abc","Warnings":[]}"#)]);
    crate::script::run_actions(&format!(
        r#"<actions><script><![CDATA[
            const rc = ec.service.rest().method("POST").addHeader("X-Test", "yes")
            rc.uri().host("127.0.0.1").port({port}).path("containers").path("/create/").parameter("name", "a b").build()
            const response = rc.addBodyParameter("secret", "s&t").jsonObject({{Image: "img"}}).call()
            status = response.statusCode
            reason = response.getReasonPhrase()
            id = response.jsonObject().Id
            uri = rc.getUri()
        ]]></script></actions>"#
    ))
    .unwrap();
    let requests = server.join().unwrap();
    let request = requests.first().expect("one request");
    assert!(
        request.starts_with("POST /containers/create?name=a%20b&secret=s%26t "),
        "{request}"
    );
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("x-test: yes"), "{request}");
    assert!(lower.contains("content-type: application/json"), "{request}");
    assert!(request.ends_with(r#"{"Image":"img"}"#), "{request}");
    let value = |name: &str| crate::script::context_value(name).map(|value| to_plain_string(&value));
    assert_eq!(value("status").as_deref(), Some("201"));
    assert_eq!(value("reason").as_deref(), Some("Created"));
    assert_eq!(value("id").as_deref(), Some("abc"));
    assert_eq!(
        value("uri"),
        Some(format!("http://127.0.0.1:{port}/containers/create?name=a%20b"))
    );

    // sim mode denies before anything is sent
    facade.set_sim_active(true);
    let err = crate::script::run_actions(
        r#"<actions><script>ec.service.rest().uri("http://127.0.0.1:9/x").call()</script></actions>"#,
    )
    .unwrap_err();
    assert!(err.message().contains("denied because sim mode is active"), "{err}");
    facade.set_sim_active(false);
}

#[test]
fn recaptcha_without_a_secret_is_skipped_and_without_a_token_fails() {
    let (_facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["impl/GoogleServices.xml"]);
    crate::service::sync(
        "org.moqui.impl.GoogleServices.verify#ReCaptcha",
        &params(&[("reCaptchaToken", "t")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    let _ = crate::service::sync(
        "org.moqui.impl.GoogleServices.verify#ReCaptcha",
        &params(&[("reCaptchaSecret", "s")]),
    );
    assert!(errors(&context).contains("No reCAPTCHA token"), "{}", errors(&context));
}

fn seed_instance(facade: &EntityFacade, port: u16) {
    seed(
        facade,
        "moqui.server.instance.InstanceHostType",
        &[
            ("hostTypeId", "Docker"),
            ("initService", "org.moqui.impl.InstanceServices.init#InstanceDocker"),
            ("startService", "org.moqui.impl.InstanceServices.start#InstanceDocker"),
            ("stopService", "org.moqui.impl.InstanceServices.stop#InstanceDocker"),
            ("removeService", "org.moqui.impl.InstanceServices.remove#InstanceDocker"),
            ("checkService", "org.moqui.impl.InstanceServices.check#InstanceDocker"),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.InstanceHost",
        &[
            ("instanceHostId", "H1"),
            ("hostTypeId", "Docker"),
            ("hostProtocol", "http"),
            ("hostAddress", "127.0.0.1"),
            ("adminPort", &port.to_string()),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.InstanceImageType",
        &[("imageTypeId", "Moqui"), ("defaultNetworkMode", "bridge")],
    );
    seed(
        facade,
        "moqui.server.instance.InstanceImage",
        &[
            ("instanceImageId", "I1"),
            ("imageTypeId", "Moqui"),
            ("imageName", "moqui/moqui"),
            ("username", "reg"),
            ("password", "pw"),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.AppInstance",
        &[
            ("appInstanceId", "A1"),
            ("instanceImageId", "I1"),
            ("instanceHostId", "H1"),
            ("hostName", "a1.example.test"),
            ("instanceName", "a1"),
            ("jsonConfig", r#"{"HostConfig":{"Memory":100},"Labels":{"x":"y"}}"#),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.AppInstanceEnv",
        &[
            ("appInstanceId", "A1"),
            ("envName", "entity_ds_user"),
            ("envValue", "moqui"),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.AppInstanceHostConfig",
        &[
            ("appInstanceId", "A1"),
            ("hostConfigName", "CpuShares"),
            ("hostConfigValue", "512"),
            ("type", "Number"),
        ],
    );
}

#[test]
fn docker_services_call_the_host_admin_api_and_keep_the_container_id() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["impl/InstanceServices.xml"]);
    let (listener, port) = loopback();
    seed_instance(&facade, port);
    let server = serve(
        listener,
        vec![
            (200, "OK", "{}"),
            (201, "Created", r#"{"Id":"c0ffee","Warnings":[]}"#),
            (204, "No Content", ""),
            (304, "Not Modified", ""),
            (
                200,
                "OK",
                r#"{"Id":"c0ffee","State":{"Running":true,"Status":"running","Error":"","StartedAt":"2026-10-14T10:00:00Z"}}"#,
            ),
            (200, "OK", "\u{1b}[32mstarted\u{1b}[0m"),
            (404, "Not Found", ""),
        ],
    );

    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.init#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(text(&out, "instanceUuid"), "c0ffee");
    assert_eq!(text(&out, "instanceCreated"), "true");
    assert_eq!(
        field(
            &facade,
            "moqui.server.instance.AppInstance",
            ("appInstanceId", "A1"),
            "instanceUuid"
        ),
        "c0ffee"
    );

    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.start#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceStarted"), "true");
    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.stop#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceStopped"), "false");
    assert!(messages(&context).contains("was not running"), "{}", messages(&context));
    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.get#DockerStatus",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceRunning"), "true");
    assert_eq!(text(&out, "statusString"), "running");
    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.get#DockerLog",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceLog"), "started");
    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.remove#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceExists"), "false");
    assert_eq!(errors(&context), "");

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert!(
        requests[0].starts_with("POST /images/create?fromImage=moqui%2Fmoqui&repo=moqui%2Fmoqui "),
        "{}",
        requests[0]
    );
    let auth = requests[0]
        .lines()
        .find_map(|line| {
            line.strip_prefix("x-registry-auth: ")
                .or_else(|| line.strip_prefix("X-Registry-Auth: "))
        })
        .expect("registry auth header");
    let decoded = {
        use base64::Engine;
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(auth.trim()).unwrap()).unwrap()
    };
    assert!(
        decoded.contains(r#""username":"reg""#) && decoded.contains(r#""password":"pw""#),
        "{decoded}"
    );
    let create = &requests[1];
    assert!(create.starts_with("POST /containers/create?name=a1 "), "{create}");
    let body: serde_json::Value = serde_json::from_str(&create[create.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["Image"], "moqui/moqui");
    assert_eq!(body["Env"][0], "entity_ds_user=moqui");
    assert_eq!(body["HostConfig"]["RestartPolicy"]["Name"], "unless-stopped");
    assert_eq!(body["HostConfig"]["NetworkMode"], "bridge");
    assert_eq!(body["HostConfig"]["CpuShares"], 512);
    // the base jsonConfig keeps what the request does not set
    assert_eq!(body["HostConfig"]["Memory"], 100);
    assert_eq!(body["Labels"]["x"], "y");
    assert!(
        requests[2].starts_with("POST /containers/c0ffee/start "),
        "{}",
        requests[2]
    );
    assert!(
        requests[3].starts_with("POST /containers/c0ffee/stop "),
        "{}",
        requests[3]
    );
    assert!(
        requests[4].starts_with("GET /containers/c0ffee/json "),
        "{}",
        requests[4]
    );
    assert!(
        requests[5].starts_with("GET /containers/c0ffee/logs?stdout=true&tail=50 "),
        "{}",
        requests[5]
    );
    assert!(requests[6].starts_with("DELETE /containers/c0ffee "), "{}", requests[6]);
}

/// A conf connection that records its statements and answers counts from a table of results.
struct RecordingDriver {
    statements: Arc<Mutex<Vec<String>>>,
    counts: Arc<Mutex<Vec<i64>>>,
}

struct RecordingConn {
    statements: Arc<Mutex<Vec<String>>>,
    counts: Arc<Mutex<Vec<i64>>>,
}

impl DbDriver for RecordingDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(RecordingConn {
            statements: Arc::clone(&self.statements),
            counts: Arc::clone(&self.counts),
        }))
    }
}

impl DbConnection for RecordingConn {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<u64, Error> {
        self.statements.lock().unwrap().push(sql.to_string());
        Ok(1)
    }
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }
    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.statements.lock().unwrap().push(sql.to_string());
        let mut counts = self.counts.lock().unwrap();
        let count = if counts.is_empty() { 0 } else { counts.remove(0) };
        Ok(vec![DbRow {
            columns: vec![("row_count".into(), Value::I64(count))],
        }])
    }
    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        Err(Error::new("no streams"))
    }
    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }
    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn stream_open(&self) -> bool {
        false
    }
    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn database_services_run_their_admin_sql_on_the_database_host_conf() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["impl/InstanceServices.xml"]);
    let statements = Arc::new(Mutex::new(Vec::new()));
    // create: no database, no user; then check: both there
    let counts = Arc::new(Mutex::new(vec![0, 0, 1, 1]));
    let seen = Arc::new(Mutex::new(Vec::<ConfDb>::new()));
    {
        let (statements, counts, seen) = (Arc::clone(&statements), Arc::clone(&counts), Arc::clone(&seen));
        crate::driver::install_conf_driver_factory(Arc::new(move |conf: &ConfDb| {
            seen.lock().unwrap().push(conf.clone());
            Ok(Arc::new(RecordingDriver {
                statements: Arc::clone(&statements),
                counts: Arc::clone(&counts),
            }) as Arc<dyn DbDriver>)
        }));
    }
    seed(
        &facade,
        "moqui.server.instance.DatabaseType",
        &[
            ("databaseTypeId", "Postgres"),
            ("confName", "postgres"),
            (
                "createService",
                "org.moqui.impl.InstanceServices.create#DatabasePostgres",
            ),
            ("checkService", "org.moqui.impl.InstanceServices.check#DatabasePostgres"),
        ],
    );
    seed(
        &facade,
        "moqui.server.instance.DatabaseHost",
        &[
            ("databaseHostId", "D1"),
            ("databaseTypeId", "Postgres"),
            ("hostAddress", "db.example.test"),
            ("hostPort", "5433"),
            ("adminUser", "admin"),
            ("adminPassword", "adminpw"),
        ],
    );
    seed(
        &facade,
        "moqui.server.instance.AppInstance",
        &[
            ("appInstanceId", "A2"),
            ("databaseHostId", "D1"),
            ("hostName", "a2"),
            ("instanceName", "a2"),
        ],
    );
    for (name, value) in [
        ("entity_ds_database", "tenant_a2"),
        ("entity_ds_user", "a2user"),
        ("entity_ds_password", "it's"),
    ] {
        seed(
            &facade,
            "moqui.server.instance.AppInstanceEnv",
            &[("appInstanceId", "A2"), ("envName", name), ("envValue", value)],
        );
    }

    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.create#AppDatabase",
        &params(&[("appInstanceId", "A2")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(text(&out, "databaseCreated"), "true");
    assert_eq!(text(&out, "userCreated"), "true");
    let out = crate::service::sync(
        "org.moqui.impl.InstanceServices.check#AppDatabase",
        &params(&[("appInstanceId", "A2")]),
    )
    .unwrap();
    assert_eq!(text(&out, "dbConnectSuccess"), "true");
    assert_eq!(text(&out, "databaseExists"), "true");
    assert_eq!(text(&out, "dbUserExists"), "true");

    let statements = statements.lock().unwrap().clone();
    assert_eq!(
        statements,
        [
            "SELECT COUNT(*) AS row_count FROM pg_database WHERE pg_database.datname='tenant_a2'",
            "CREATE DATABASE tenant_a2",
            "SELECT COUNT(*) AS row_count FROM pg_catalog.pg_user WHERE usename='a2user'",
            "CREATE USER a2user WITH PASSWORD 'it''s'",
            "GRANT ALL PRIVILEGES ON DATABASE tenant_a2 TO a2user",
            "SELECT COUNT(*) AS row_count FROM pg_database WHERE pg_database.datname='tenant_a2'",
            "SELECT COUNT(*) AS row_count FROM pg_catalog.pg_user WHERE usename='a2user'",
        ]
    );
    let seen = seen.lock().unwrap().clone();
    // create connects to the server's default database, named as the conf; check to the tenant's
    assert_eq!(seen[0].database, "postgres");
    assert_eq!(seen[0].host, "db.example.test");
    assert_eq!(seen[0].port, "5433");
    assert_eq!(seen[0].user, "admin");
    assert_eq!(seen[0].password, "adminpw");
    assert_eq!(seen.last().unwrap().database, "tenant_a2");

    // a name that is not a plain identifier never reaches the SQL
    let mut env = facade.make_value("moqui.server.instance.AppInstanceEnv").unwrap();
    env.set("appInstanceId", Value::string("A2"));
    env.set("envName", Value::string("entity_ds_user"));
    env.set("envValue", Value::string("x; DROP TABLE y"));
    env.update(&facade).unwrap();
    let err = crate::service::sync(
        "org.moqui.impl.InstanceServices.create#AppDatabase",
        &params(&[("appInstanceId", "A2")]),
    );
    assert!(
        err.is_err() || errors(&context).contains("is not a plain identifier"),
        "{}",
        errors(&context)
    );
}

#[cfg(unix)]
#[test]
fn the_token_command_returns_its_first_line_and_a_failure_throws() {
    let (_facade, context) = setup();
    let _entered = context.enter();
    crate::script::run_actions(
        r#"<actions><script>token = ec.resource.runTokenCommand("printf tok\\nsecond")</script></actions>"#,
    )
    .unwrap();
    assert_eq!(
        crate::script::context_value("token")
            .map(|value| to_plain_string(&value))
            .as_deref(),
        Some("tok")
    );
    let err = crate::script::run_actions(r#"<actions><script>ec.resource.runTokenCommand("false")</script></actions>"#)
        .unwrap_err();
    assert!(err.message().contains("token command false failed"), "{err}");
}

/// get#UserWikiSpaces: each space the user may view once, though the space is joined to each user's grant; the user's own
/// grant in it (none from another user's row), and the count of its pages the user may view.
#[test]
fn user_wiki_spaces_are_each_listed_once_with_their_page_count() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["EntityServices.xml", "impl/WikiServices.xml"]);
    for (space, restrict) in [("WOPEN", "N"), ("WSHUT", "Y"), ("WNONE", "Y")] {
        seed(
            &facade,
            "moqui.resource.wiki.WikiSpace",
            &[
                ("wikiSpaceId", space),
                ("description", space),
                ("restrictView", restrict),
            ],
        );
    }
    for (space, user, admin, view) in [
        ("WOPEN", "ada", "N", "Y"),
        ("WOPEN", "bob", "Y", "Y"),
        ("WOPEN", "cy", "N", "N"),
        ("WSHUT", "ada", "Y", "N"),
        ("WNONE", "bob", "N", "Y"),
    ] {
        seed(
            &facade,
            "moqui.resource.wiki.WikiSpaceUser",
            &[
                ("wikiSpaceId", space),
                ("userId", user),
                ("allowAdmin", admin),
                ("allowView", view),
            ],
        );
    }
    for (page, space, restrict) in [
        ("P1", "WOPEN", "N"),
        ("P2", "WOPEN", ""),
        ("P3", "WOPEN", "Y"),
        ("P4", "WSHUT", "N"),
    ] {
        let mut pairs = vec![("wikiPageId", page), ("wikiSpaceId", space), ("pagePath", page)];
        if !restrict.is_empty() {
            pairs.push(("restrictView", restrict));
        }
        seed(&facade, "moqui.resource.wiki.WikiPage", &pairs);
    }
    // the restricted page: Ada may view it, Bob's row says no
    for (user, view) in [("ada", "Y"), ("bob", "N")] {
        seed(
            &facade,
            "moqui.resource.wiki.WikiPageUser",
            &[("wikiPageId", "P3"), ("userId", user), ("allowView", view)],
        );
    }
    let spaces = |user: &str| -> Vec<(String, String, String, String)> {
        let out = crate::service::sync(
            "org.moqui.impl.WikiServices.get#UserWikiSpaces",
            &params(&[("userId", user)]),
        )
        .unwrap_or_else(|err| panic!("{err}; {}", errors(&context)));
        assert_eq!(errors(&context), "");
        let Some(Value::List(list)) = out.get("wikiSpaceAndUserList") else {
            panic!("no list: {out:?}");
        };
        list.iter()
            .map(|item| {
                let Value::Map(map) = item else { panic!("{item:?}") };
                let get = |name: &str| map.get(name).map(to_plain_string).unwrap_or_default();
                (get("wikiSpaceId"), get("userId"), get("allowAdmin"), get("pageCount"))
            })
            .collect()
    };
    let row = |space: &str, user: &str, admin: &str, count: &str| {
        (
            space.to_string(),
            user.to_string(),
            admin.to_string(),
            count.to_string(),
        )
    };
    assert_eq!(
        spaces("ada"),
        [row("WOPEN", "ada", "N", "3"), row("WSHUT", "ada", "Y", "1")]
    );
    assert_eq!(
        spaces("bob"),
        [row("WNONE", "bob", "N", "0"), row("WOPEN", "bob", "Y", "2")]
    );
    // no grant of their own: the open space once, with no user fields
    assert_eq!(spaces("dee"), [row("WOPEN", "", "", "2")]);
}

#[test]
fn wiki_pages_keep_versions_and_move_clone_and_delete() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    // get#WikiPageManualDocumentData implements add#ManualDocumentData.
    install(&["EntityServices.xml", "impl/WikiServices.xml"]);
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = crate::service::sync(&format!("org.moqui.impl.WikiServices.{name}"), &params(pairs))
            .unwrap_or_else(|err| panic!("{name}: {err}; {}", errors(&context)));
        assert_eq!(errors(&context), "", "{name}");
        out
    };

    sync(
        "create#WikiSpace",
        &[("wikiSpaceId", "TST"), ("description", "Test space")],
    );
    assert_eq!(
        field(
            &facade,
            "moqui.resource.wiki.WikiSpace",
            ("wikiSpaceId", "TST"),
            "rootPageLocation"
        ),
        "dbresource://WikiSpace/TST.md"
    );
    let root = facade.read_db_resource("dbresource://WikiSpace/TST.md").unwrap();
    assert!(String::from_utf8_lossy(&root.bytes).contains("Automatic root page for space Test space"));

    let page = sync(
        "update#WikiPage",
        &[("wikiSpaceId", "TST"), ("pageName", "Page1"), ("pageText", "one")],
    );
    assert_eq!(text(&page, "pagePath"), "Page1");
    let page_id = text(&page, "wikiPageId");
    assert!(!page_id.is_empty());
    sync(
        "update#WikiPage",
        &[("wikiSpaceId", "TST"), ("pageName", "Page1"), ("pageText", "two")],
    );
    let info = sync("get#WikiPageInfo", &[("wikiSpaceId", "TST"), ("pagePath", "Page1")]);
    assert_eq!(text(&info, "currentVersionName"), "02");
    assert_eq!(text(&info, "wikiType"), "md");
    assert_eq!(text(&info, "pageLocation"), "dbresource://WikiSpace/TST/Page1.md");
    let first = sync(
        "get#PublishedWikiPageText",
        &[("wikiSpaceId", "TST"), ("pagePath", "Page1"), ("versionName", "01")],
    );
    assert_eq!(text(&first, "pageText"), "one");
    sync(
        "set#PublishedVersion",
        &[("wikiPageId", &page_id), ("publishedVersionName", "01")],
    );
    let published = sync(
        "get#PublishedWikiPageText",
        &[("wikiSpaceId", "TST"), ("pagePath", "Page1")],
    );
    assert_eq!(text(&published, "pageText"), "one");
    let histories = facade
        .find("moqui.resource.wiki.WikiPageHistory")
        .unwrap()
        .eq("wikiPageId", Value::string(&page_id))
        .list()
        .unwrap();
    assert_eq!(histories.len(), 2);

    // a child page lives in the directory named as its parent
    sync(
        "update#WikiPage",
        &[
            ("wikiSpaceId", "TST"),
            ("parentPath", "Page1"),
            ("pageName", "Child"),
            ("pageText", "child"),
        ],
    );
    assert!(
        facade
            .read_db_resource("dbresource://WikiSpace/TST/Page1/Child.md")
            .unwrap()
            .exists
    );
    let children = sync("get#WikiPageChildren", &[("wikiSpaceId", "TST"), ("pagePath", "Page1")]);
    let Some(Value::List(children)) = children.get("childPageInfoList") else {
        panic!("no children: {children:?}");
    };
    assert_eq!(children.len(), 1);

    // an attachment is listed with its page and deleted by name
    facade
        .write_db_resource("dbresource://WikiSpace/TST/Page1/_attachments/note.txt", b"attached")
        .unwrap();
    let info = sync("get#WikiPageInfo", &[("wikiSpaceId", "TST"), ("pagePath", "Page1")]);
    let Some(Value::List(attachments)) = info.get("attachmentList") else {
        panic!("no attachments: {info:?}");
    };
    assert_eq!(attachments.len(), 1);
    let reference = sync(
        "get#WikiPageAttachment",
        &[("wikiSpaceId", "TST"), ("pagePath", "Page1/note.txt")],
    );
    let Some(Value::Map(reference)) = reference.get("attachmentReference") else {
        panic!("no attachment reference: {reference:?}");
    };
    assert_eq!(
        reference.get("location").map(to_plain_string).as_deref(),
        Some("dbresource://WikiSpace/TST/Page1/_attachments/note.txt")
    );
    sync(
        "delete#WikiPageAttachment",
        &[("wikiSpaceId", "TST"), ("pagePath", "Page1"), ("filename", "note.txt")],
    );
    assert!(
        !facade
            .read_db_resource("dbresource://WikiSpace/TST/Page1/_attachments/note.txt")
            .unwrap()
            .exists
    );

    // renaming moves the page file and its directory
    let renamed = sync(
        "update#WikiPage",
        &[
            ("wikiSpaceId", "TST"),
            ("wikiPageId", &page_id),
            ("pageName", "Renamed"),
            ("pageText", "two"),
        ],
    );
    assert_eq!(text(&renamed, "pagePath"), "Renamed");
    assert!(
        facade
            .read_db_resource("dbresource://WikiSpace/TST/Renamed.md")
            .unwrap()
            .exists
    );
    assert!(
        facade
            .read_db_resource("dbresource://WikiSpace/TST/Renamed/Child.md")
            .unwrap()
            .exists
    );
    assert!(
        !facade
            .read_db_resource("dbresource://WikiSpace/TST/Page1.md")
            .unwrap()
            .exists
    );
    assert_eq!(
        field(
            &facade,
            "moqui.resource.wiki.WikiPage",
            ("wikiPageId", &page_id),
            "pagePath"
        ),
        "Renamed"
    );

    // a clone has every page
    sync("clone#WikiSpace", &[("baseWikiSpaceId", "TST"), ("wikiSpaceId", "TS2")]);
    let pages = sync("get#WikiSpacePages", &[("wikiSpaceId", "TS2")]);
    let Some(Value::List(flat)) = pages.get("allChildFileFlatList") else {
        panic!("no pages: {pages:?}");
    };
    let paths: Vec<String> = flat
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => map.get("path").map(to_plain_string),
            _ => None,
        })
        .collect();
    assert_eq!(paths, ["", "Renamed", "Renamed/Child"]);
    let copied = facade
        .read_db_resource("dbresource://WikiSpace/TS2/Renamed/Child.md")
        .unwrap();
    assert_eq!(copied.bytes, b"child".to_vec());

    // a page with a child is kept (an error, so a SECA's work before it rolls back); the child, then the page, are
    // deleted
    let _ = crate::service::sync(
        "org.moqui.impl.WikiServices.delete#WikiPage",
        &params(&[("wikiSpaceId", "TS2"), ("pagePath", "Renamed")]),
    );
    assert!(errors(&context).contains("has pages under it"), "{}", errors(&context));
    context.message().clear();
    let parent = sync(
        "delete#WikiPage",
        &[("wikiSpaceId", "TS2"), ("pagePath", "Renamed/Child")],
    );
    assert_eq!(text(&parent, "pagePath"), "Renamed");
    sync("delete#WikiPage", &[("wikiSpaceId", "TS2"), ("pagePath", "Renamed")]);
    assert!(
        !facade
            .read_db_resource("dbresource://WikiSpace/TS2/Renamed.md")
            .unwrap()
            .exists
    );
    assert!(
        facade
            .read_db_resource("dbresource://WikiSpace/TST/Renamed.md")
            .unwrap()
            .exists
    );
}

/// A wiki space whose pages are `git://` files in a database repository: the wiki services use only the resource
/// API, so page versions are commits; then the GitServices export it, and refuse a fetch of a non-mirror.
#[cfg(feature = "git")]
#[test]
fn a_wiki_space_on_a_git_repository_keeps_commits_as_versions() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    context.factory().resource().install_git().unwrap();
    install(&["EntityServices.xml", "impl/WikiServices.xml", "impl/GitServices.xml"]);
    seed(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", "wiki"), ("repositoryKind", "db")],
    );
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = crate::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    sync(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "GTS"),
            ("description", "Git space"),
            ("rootPageDirectory", "git://wiki"),
        ],
    );
    for text in ["one", "two"] {
        sync(
            "org.moqui.impl.WikiServices.update#WikiPage",
            &[("wikiSpaceId", "GTS"), ("pageName", "Page1"), ("pageText", text)],
        );
    }
    let location = "git://wiki/GTS/Page1.md";
    let history = context.factory().resource().version_history(location).unwrap();
    assert_eq!(history.len(), 2);
    let info = sync(
        "org.moqui.impl.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", "GTS"), ("pagePath", "Page1")],
    );
    assert_eq!(text(&info, "currentVersionName"), history[0].version_name);
    assert_eq!(text(&info, "pageLocation"), location);
    let first = sync(
        "org.moqui.impl.WikiServices.get#PublishedWikiPageText",
        &[
            ("wikiSpaceId", "GTS"),
            ("pagePath", "Page1"),
            ("versionName", &history[1].version_name),
        ],
    );
    assert_eq!(text(&first, "pageText"), "one");

    let target = std::env::temp_dir().join(format!("moqui-wiki-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    let exported = sync(
        "org.moqui.impl.GitServices.export#GitRepository",
        &[("repositoryId", "wiki"), ("targetPath", &target.to_string_lossy())],
    );
    assert_eq!(text(&exported, "exportPath"), target.to_string_lossy());
    assert!(target.join("HEAD").is_file());
    let _ = std::fs::remove_dir_all(&target);

    let out = crate::service::sync(
        "org.moqui.impl.GitServices.fetch#GitRepository",
        &params(&[("repositoryId", "wiki")]),
    )
    .expect("a business failure is a message, not an Err");
    assert!(!out.contains_key("headCommitId"));
    assert!(errors(&context).contains("not a remote mirror"), "{}", errors(&context));
}

/// A wiki space rooted in a subdirectory of a code repository (`git://code/docs`, IronStone Projects' documentation
/// beside the code): its root page and pages are files under `docs/`, each save a commit beside the code's, which stays
/// as it was; history, child pages, a rename, the page list and a delete work as at a repository's root, and a branch
/// rule protecting the default branch does not refuse the saves (`git://` writes pass the rules and are logged).
#[cfg(feature = "git")]
#[test]
fn a_wiki_space_in_a_code_repository_subdirectory_keeps_its_pages_beside_the_code() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    context.factory().resource().install_git().unwrap();
    install(&["EntityServices.xml", "impl/WikiServices.xml", "impl/GitServices.xml"]);
    seed(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", "code"), ("repositoryKind", "db")],
    );
    let resources = context.factory().resource();
    resources
        .write_bytes("git://code/src/main.rs", b"fn main() {}\n")
        .unwrap();
    resources.write_bytes("git://code/README.md", b"# Code\n").unwrap();
    seed(
        &facade,
        "moqui.resource.GitBranchRule",
        &[
            ("branchRuleId", "CODE_MAIN"),
            ("repositoryId", "code"),
            ("refPattern", "main"),
            ("allowForcePush", "N"),
            ("allowDelete", "N"),
            ("requireMergeRequest", "Y"),
        ],
    );
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = crate::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    let made = sync(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("description", "Code documentation"),
            ("rootPageDirectory", "git://code/docs"),
        ],
    );
    assert_eq!(text(&made, "rootPageLocation"), "git://code/docs/CODEDOCS.md");
    assert!(resources.get("git://code/docs/CODEDOCS.md").unwrap().exists());
    let access = sync(
        "org.moqui.impl.WikiServices.get#WikiPageAccess",
        &[("wikiSpaceId", "CODEDOCS")],
    );
    assert_eq!(text(&access, "storage"), "git");
    assert_eq!(text(&access, "repositoryId"), "code");
    assert_eq!(text(&access, "canUpdate"), "true");

    for body in ["# Getting started\n\nOne.\n", "# Getting started\n\nTwo.\n"] {
        sync(
            "org.moqui.impl.WikiServices.update#WikiPage",
            &[
                ("wikiSpaceId", "CODEDOCS"),
                ("pageName", "GettingStarted"),
                ("pageText", body),
                ("versionNote", "Getting started"),
            ],
        );
    }
    // a child page under its parent page (a directory without its page file is not listed)
    sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pageName", "Releases"),
            ("pageText", "# Releases\n"),
        ],
    );
    sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pageName", "1.0"),
            ("parentPath", "Releases"),
            ("pageText", "# 1.0\n"),
        ],
    );
    let page = "git://code/docs/CODEDOCS/GettingStarted.md";
    assert_eq!(resources.location_text(page).unwrap(), "# Getting started\n\nTwo.\n");
    assert!(resources
        .get("git://code/docs/CODEDOCS/Releases/1.0.md")
        .unwrap()
        .exists());
    let history = resources.version_history(page).unwrap();
    assert_eq!(history.len(), 2);
    let info = sync(
        "org.moqui.impl.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", "CODEDOCS"), ("pagePath", "GettingStarted")],
    );
    assert_eq!(text(&info, "pageLocation"), page);
    assert_eq!(text(&info, "currentVersionName"), history[0].version_name);
    // the code beside the pages is as it was
    assert_eq!(
        resources.location_text("git://code/src/main.rs").unwrap(),
        "fn main() {}\n"
    );
    assert_eq!(resources.location_text("git://code/README.md").unwrap(), "# Code\n");

    let pages = sync(
        "org.moqui.impl.WikiServices.get#WikiSpacePages",
        &[("wikiSpaceId", "CODEDOCS")],
    );
    let Some(Value::List(flat)) = pages.get("allChildFileFlatList") else {
        panic!("no pages: {pages:?}");
    };
    let mut paths: Vec<String> = flat
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => map.get("path").map(to_plain_string),
            _ => None,
        })
        .collect();
    paths.sort();
    assert_eq!(paths, ["", "GettingStarted", "Releases", "Releases/1.0"]);

    let renamed = sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pagePath", "GettingStarted"),
            ("pageName", "Start"),
            ("pageText", "# Start\n"),
        ],
    );
    assert_eq!(text(&renamed, "pagePath"), "Start");
    assert!(resources.get("git://code/docs/CODEDOCS/Start.md").unwrap().exists());
    assert!(!resources.get(page).unwrap().exists());
    sync(
        "org.moqui.impl.WikiServices.delete#WikiPage",
        &[("wikiSpaceId", "CODEDOCS"), ("pagePath", "Start")],
    );
    assert!(!resources.get("git://code/docs/CODEDOCS/Start.md").unwrap().exists());
    assert_eq!(
        resources.location_text("git://code/src/main.rs").unwrap(),
        "fn main() {}\n"
    );
}

/// `ensure#UserNotesSpace` (IronStone R22): the user's private notes space on database files, made once, with its owner
/// row and root page, then answered as it is; a quick note writes into it. Another user's only with authorization off.
#[test]
fn a_users_notes_space_is_made_on_first_use_and_kept() {
    let (facade, context) = setup();
    let _entered = context.enter();
    install(&["EntityServices.xml", "impl/WikiServices.xml"]);
    {
        let _authz = context.disable_authz();
        seed(
            &facade,
            "moqui.security.UserAccount",
            &[
                ("userId", "NOTER"),
                ("username", "noter"),
                ("userFullName", "Nora Noter"),
            ],
        );
        seed(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", "OTHER"), ("username", "other")],
        );
        crate::user::internal_login_user("noter").unwrap();
    }
    let ensure = |pairs: &[(&str, &str)]| {
        crate::service::sync("org.moqui.impl.WikiServices.ensure#UserNotesSpace", &params(pairs)).unwrap()
    };
    let first = ensure(&[]);
    assert_eq!(errors(&context), "");
    assert_eq!(text(&first, "wikiSpaceId"), "NOTES_NOTER");
    assert_eq!(
        text(&first, "rootPageLocation"),
        "dbresource://WikiSpace/NOTES_NOTER.md"
    );
    assert_eq!(text(&first, "created"), "true");
    let space = ("wikiSpaceId", "NOTES_NOTER");
    for (name, expected) in [
        ("restrictView", "Y"),
        ("restrictUpdate", "Y"),
        ("description", "Notes of Nora Noter"),
    ] {
        assert_eq!(
            field(&facade, "moqui.resource.wiki.WikiSpace", space, name),
            expected,
            "{name}"
        );
    }
    let owner = facade
        .find("moqui.resource.wiki.WikiSpaceUser")
        .unwrap()
        .eq("wikiSpaceId", Value::string("NOTES_NOTER"))
        .eq("userId", Value::string("NOTER"))
        .one()
        .unwrap()
        .expect("the owner's row");
    for name in ["allowAdmin", "allowView", "allowUpdate"] {
        assert_eq!(owner.get(name).map(to_plain_string).as_deref(), Some("Y"), "{name}");
    }
    let root = facade
        .read_db_resource("dbresource://WikiSpace/NOTES_NOTER.md")
        .unwrap();
    assert!(String::from_utf8_lossy(&root.bytes).contains("# Notes of Nora Noter"));

    // again: the same space, nothing made
    let again = ensure(&[("userId", "NOTER")]);
    assert_eq!(text(&again, "wikiSpaceId"), "NOTES_NOTER");
    assert_eq!(text(&again, "created"), "false");
    assert_eq!(errors(&context), "");
    // the owner may write in it, as QuickNote does
    let note = crate::service::sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &params(&[
            ("wikiSpaceId", "NOTES_NOTER"),
            ("pageName", "Groceries"),
            ("pageText", "milk"),
        ]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(text(&note, "pagePath"), "Groceries");

    // another user's space is not made for a caller with authorization on
    let theirs = crate::service::sync(
        "org.moqui.impl.WikiServices.ensure#UserNotesSpace",
        &params(&[("userId", "OTHER")]),
    )
    .unwrap();
    assert!(errors(&context).contains("Another user's notes space"), "{theirs:?}");
    context.message().clear_errors();
    assert!(facade
        .find("moqui.resource.wiki.WikiSpace")
        .unwrap()
        .eq("wikiSpaceId", Value::string("NOTES_OTHER"))
        .one()
        .unwrap()
        .is_none());
    let _authz = context.disable_authz();
    let made = ensure(&[("userId", "OTHER")]);
    assert_eq!(text(&made, "created"), "true");
}

/// `clean#GitUpdates` removes done and none rows older than the window, and their refs, and leaves the rest.
/// One row whose delete fails is logged and skipped.
#[cfg(feature = "git")]
#[test]
fn old_done_git_updates_are_removed_and_one_failure_does_not_stop_the_rest() {
    let facade = crate::testing::open_with(
        &crate::testing::framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            data_feed: false,
            tape: None,
        },
    );
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), crate::context::framework_dir());
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["impl/GitServices.xml"]);
    crate::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca id="FailGitUpdate" entity="moqui.resource.GitUpdate" on-delete="true" run-before="true">
            <condition><expression>updateId == 'FAIL'</expression></condition>
            <actions><script>throw new Error("forced")</script></actions>
        </eeca></eecas>"#,
    )
    .unwrap();
    let ago = |days: i64| {
        let millis = jiff::Timestamp::now().as_millisecond() - days * 86_400_000;
        moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
    };
    let row = |id: &str, status: &str, when: Value| {
        let mut update = facade.make_value("moqui.resource.GitUpdate").unwrap();
        update.set("updateId", Value::string(id));
        update.set("ecaStatus", Value::string(status));
        update.set("updateDate", when);
        update.set("source", Value::string("write"));
        update.create(&facade).unwrap();
    };
    let git_ref = |id: &str| {
        let mut reference = facade.make_value("moqui.resource.GitUpdateRef").unwrap();
        reference.set("updateId", Value::string(id));
        reference.set("refName", Value::string("refs/heads/main"));
        reference.create(&facade).unwrap();
    };
    row("OLD_DONE", "done", ago(200));
    git_ref("OLD_DONE");
    row("OLD_NONE", "none", ago(200));
    row("OLD_PENDING", "pending", ago(200));
    row("OLD_FAILED", "failed", ago(200));
    row("OLD_RUNNING", "running", ago(200));
    row("OLD_RECORDING", "recording", ago(200));
    row("NEW_DONE", "done", ago(1));
    git_ref("NEW_DONE");
    row("FAIL", "done", ago(200));
    let out = crate::service::sync("org.moqui.impl.GitServices.clean#GitUpdates", &params(&[])).unwrap();
    assert_eq!(errors(&context), "", "{out:?}");
    assert_eq!(text(&out, "recordsRemoved"), "2", "{out:?}");
    let present = |id: &str| {
        facade
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("updateId", Value::string(id))
            .one()
            .unwrap()
            .is_some()
    };
    let refs = |id: &str| {
        facade
            .find("moqui.resource.GitUpdateRef")
            .unwrap()
            .eq("updateId", Value::string(id))
            .list()
            .unwrap()
            .len()
    };
    assert!(!present("OLD_DONE") && refs("OLD_DONE") == 0);
    assert!(!present("OLD_NONE"));
    for kept in [
        "OLD_PENDING",
        "OLD_FAILED",
        "OLD_RUNNING",
        "OLD_RECORDING",
        "NEW_DONE",
        "FAIL",
    ] {
        assert!(present(kept), "{kept}");
    }
    assert_eq!(refs("NEW_DONE"), 1);
}
