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

//! The framework services that were Groovy and are JavaScript now (`WikiServices`, the git update cleanup of
//! `InstanceServices`) on `git://` repositories, and the hosted-instance Docker services of `InstanceServices`, which
//! call the Docker Engine API through this crate's `call#DockerApi`. Their other tests are moqui-core's
//! `service/script_ports_tests.rs`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::service::ServicePools;

fn setup() -> (EntityFacade, ExecutionContext) {
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), moqui_core::context::framework_dir());
    context.factory().set_entity(facade.clone());
    (facade, context)
}

fn install(files: &[&str]) {
    let dir = moqui_core::context::framework_dir().join("service/org/moqui");
    let paths: Vec<_> = files.iter().map(|file| dir.join(file)).collect();
    let failures = moqui_core::service::lint(&paths).unwrap();
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

fn seed(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

/// A wiki space whose pages are `git://` files in a database repository: the wiki services use only the resource
/// API, so page versions are commits; then the GitServices export it, and refuse a fetch of a non-mirror.
#[test]
fn a_wiki_space_on_a_git_repository_keeps_commits_as_versions() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::install(context.factory()).unwrap();
    install(&["EntityServices.xml", "resource/WikiServices.xml", "git/GitServices.xml"]);
    seed(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", "wiki"), ("repositoryKind", "db")],
    );
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = moqui_core::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    sync(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "GTS"),
            ("description", "Git space"),
            ("rootPageDirectory", "git://wiki"),
        ],
    );
    for text in ["one", "two"] {
        sync(
            "org.moqui.resource.WikiServices.update#WikiPage",
            &[("wikiSpaceId", "GTS"), ("pageName", "Page1"), ("pageText", text)],
        );
    }
    let location = "git://wiki/GTS/Page1.md";
    let history = context.factory().resource().version_history(location).unwrap();
    assert_eq!(history.len(), 2);
    let info = sync(
        "org.moqui.resource.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", "GTS"), ("pagePath", "Page1")],
    );
    assert_eq!(text(&info, "currentVersionName"), history[0].version_name);
    assert_eq!(text(&info, "pageLocation"), location);
    let first = sync(
        "org.moqui.resource.WikiServices.get#PublishedWikiPageText",
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
        "org.moqui.git.GitServices.export#GitRepository",
        &[("repositoryId", "wiki"), ("targetPath", &target.to_string_lossy())],
    );
    assert_eq!(text(&exported, "exportPath"), target.to_string_lossy());
    assert!(target.join("HEAD").is_file());
    let _ = std::fs::remove_dir_all(&target);

    let out = moqui_core::service::sync(
        "org.moqui.git.GitServices.fetch#GitRepository",
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
#[test]
fn a_wiki_space_in_a_code_repository_subdirectory_keeps_its_pages_beside_the_code() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::install(context.factory()).unwrap();
    install(&["EntityServices.xml", "resource/WikiServices.xml", "git/GitServices.xml"]);
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
        let out = moqui_core::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    let made = sync(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("description", "Code documentation"),
            ("rootPageDirectory", "git://code/docs"),
        ],
    );
    assert_eq!(text(&made, "rootPageLocation"), "git://code/docs/CODEDOCS.md");
    assert!(resources.get("git://code/docs/CODEDOCS.md").unwrap().exists());
    let access = sync(
        "org.moqui.resource.WikiServices.get#WikiPageAccess",
        &[("wikiSpaceId", "CODEDOCS")],
    );
    assert_eq!(text(&access, "storage"), "git");
    assert_eq!(text(&access, "repositoryId"), "code");
    assert_eq!(text(&access, "canUpdate"), "true");

    for body in ["# Getting started\n\nOne.\n", "# Getting started\n\nTwo.\n"] {
        sync(
            "org.moqui.resource.WikiServices.update#WikiPage",
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
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pageName", "Releases"),
            ("pageText", "# Releases\n"),
        ],
    );
    sync(
        "org.moqui.resource.WikiServices.update#WikiPage",
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
        "org.moqui.resource.WikiServices.get#WikiPageInfo",
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
        "org.moqui.resource.WikiServices.get#WikiSpacePages",
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
        "org.moqui.resource.WikiServices.update#WikiPage",
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
        "org.moqui.resource.WikiServices.delete#WikiPage",
        &[("wikiSpaceId", "CODEDOCS"), ("pagePath", "Start")],
    );
    assert!(!resources.get("git://code/docs/CODEDOCS/Start.md").unwrap().exists());
    assert_eq!(
        resources.location_text("git://code/src/main.rs").unwrap(),
        "fn main() {}\n"
    );
}

/// `clean#GitUpdates` removes done and none rows older than the window, and their refs, and leaves the rest.
/// One row whose delete fails is logged and skipped.
#[test]
fn old_done_git_updates_are_removed_and_one_failure_does_not_stop_the_rest() {
    let facade = moqui_core::testing::open_with(
        &moqui_core::testing::framework_dir(),
        moqui_core::testing::Options {
            entity_eca: true,
            data_feed: false,
            tape: None,
        },
    );
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), moqui_core::context::framework_dir());
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["git/GitServices.xml"]);
    moqui_core::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca id="FailGitUpdate" entity="moqui.resource.GitUpdate" on-delete="true" run-before="true">
            <condition><expression>updateId == 'FAIL'</expression></condition>
            <actions><script>throw new Error("forced")</script></actions>
        </eeca></eecas>"#,
    )
    .unwrap();
    let ago = |days: i64| {
        let millis = jiff::Timestamp::now().as_millisecond() - days * 86_400_000;
        moqui_lib::object::basic_convert(&Value::I64(millis), "Timestamp").value
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
    let out = moqui_core::service::sync("org.moqui.git.GitServices.clean#GitUpdates", &params(&[])).unwrap();
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

fn messages(context: &ExecutionContext) -> String {
    context.message().messages().join("\n")
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

fn seed_instance(facade: &EntityFacade, address: &str, port: u16) {
    seed(
        facade,
        "moqui.server.instance.InstanceHostType",
        &[
            ("hostTypeId", "Docker"),
            ("initService", "org.moqui.dev.InstanceServices.init#InstanceDocker"),
            ("startService", "org.moqui.dev.InstanceServices.start#InstanceDocker"),
            ("stopService", "org.moqui.dev.InstanceServices.stop#InstanceDocker"),
            ("removeService", "org.moqui.dev.InstanceServices.remove#InstanceDocker"),
            ("checkService", "org.moqui.dev.InstanceServices.check#InstanceDocker"),
        ],
    );
    seed(
        facade,
        "moqui.server.instance.InstanceHost",
        &[
            ("instanceHostId", "H1"),
            ("hostTypeId", "Docker"),
            ("hostProtocol", "http"),
            ("hostAddress", address),
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

/// The hosted-instance services send their Docker Engine API calls through `call#DockerApi`, here to a host on this
/// machine over plain http: the image pull with its registry login, the container (whose id is kept), then start,
/// stop, status, log and remove.
#[test]
fn docker_services_call_the_host_admin_api_and_keep_the_container_id() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["dev/InstanceServices.xml"]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    seed_instance(&facade, "127.0.0.1", port);
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

    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.init#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(text(&out, "instanceUuid"), "c0ffee");
    assert_eq!(text(&out, "instanceCreated"), "true");
    assert_eq!(text(&out, "connectSuccess"), "true");
    assert_eq!(
        field(
            &facade,
            "moqui.server.instance.AppInstance",
            ("appInstanceId", "A1"),
            "instanceUuid"
        ),
        "c0ffee"
    );

    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.start#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceStarted"), "true");
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.stop#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceStopped"), "false");
    assert!(messages(&context).contains("was not running"), "{}", messages(&context));
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.get#DockerStatus",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceRunning"), "true");
    assert_eq!(text(&out, "statusString"), "running");
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.get#DockerLog",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceLog"), "started");
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.remove#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap();
    assert_eq!(text(&out, "instanceExists"), "false");
    assert_eq!(errors(&context), "");

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert!(
        requests[0].starts_with("POST /v1.40/images/create?fromImage=moqui%2Fmoqui&repo=moqui%2Fmoqui "),
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
    assert!(create.starts_with("POST /v1.40/containers/create?name=a1 "), "{create}");
    assert!(
        create.to_ascii_lowercase().contains("content-type: application/json"),
        "{create}"
    );
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
        requests[2].starts_with("POST /v1.40/containers/c0ffee/start "),
        "{}",
        requests[2]
    );
    assert!(
        requests[3].starts_with("POST /v1.40/containers/c0ffee/stop "),
        "{}",
        requests[3]
    );
    assert!(
        requests[4].starts_with("GET /v1.40/containers/c0ffee/json "),
        "{}",
        requests[4]
    );
    assert!(
        requests[5].starts_with("GET /v1.40/containers/c0ffee/logs?stdout=true&tail=50 "),
        "{}",
        requests[5]
    );
    assert!(
        requests[6].starts_with("DELETE /v1.40/containers/c0ffee "),
        "{}",
        requests[6]
    );
}

/// SEC-OUT-08: no Docker call goes over plain http to a host that is not this machine. The image pull (with its
/// registry login), start and status are refused before anything is sent, and say so.
#[test]
fn no_docker_call_goes_over_plain_http_to_another_host() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["dev/InstanceServices.xml"]);
    seed_instance(&facade, "192.0.2.10", 2375);
    let started = Instant::now();
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.init#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap_or_default();
    assert!(errors(&context).contains("plain http"), "{}", errors(&context));
    assert_ne!(text(&out, "instanceCreated"), "true");
    assert_eq!(text(&out, "connectSuccess"), "false");
    context.message().clear();
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.start#InstanceDocker",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap_or_default();
    assert!(errors(&context).contains("plain http"), "{}", errors(&context));
    assert_ne!(text(&out, "instanceStarted"), "true");
    context.message().clear();
    let out = moqui_core::service::sync(
        "org.moqui.dev.InstanceServices.get#DockerStatus",
        &params(&[("appInstanceId", "A1")]),
    )
    .unwrap_or_default();
    assert!(errors(&context).contains("plain http"), "{}", errors(&context));
    assert_eq!(text(&out, "connectSuccess"), "false");
    // refused before connecting: nothing waited on the unreachable address
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
