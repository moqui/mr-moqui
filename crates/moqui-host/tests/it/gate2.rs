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

//! Plain HTTP gate. Seed, demo user, entity and service REST, login cookie, restart.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moqui_core::entity::EntityFacade;
use moqui_core::user::hash_password;
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::entity_facade::open_entity_facade;
use moqui_host::{load_data, start_server, LoadFlags};
use moqui_lib::object::Value;

#[test]
#[ignore = "slow: cargo test-slow"]
fn empty_db_startup_persists_audit_artifact_stack() {
    if !crate::common::own_process() {
        return;
    }
    use moqui_core::driver::DbConnection;
    use moqui_host::db::embedded::StartOptions;

    let _serial = crate::common::serial();
    let _properties = crate::common::PropertyReset::of(&["cluster_https_port"]);
    let runtime = temp_dir("moqui-audit-rt");
    let cluster_dir = temp_dir("moqui-audit-pg");
    let _runtime_guard = DirGuard(runtime.clone());
    let _cluster_guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    std::fs::write(
        runtime.join("conf/MoquiDevConf.xml"),
        r#"<moqui-conf><tools empty-db-load="seed,seed-initial,install,demo"/></moqui-conf>"#,
    )
    .unwrap();
    let base =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component")).unwrap();
    std::os::unix::fs::symlink(&base, runtime.join("base-component")).unwrap();
    let framework = base.join("framework");
    moqui_lib::env::set_property("cluster_https_port", "0");

    // No pre-seeding: startup itself must load the checked-in WebrootSecurityDemoData user.
    let server = start_server(
        &runtime,
        &framework,
        &cluster_dir,
        SocketAddr::from(([127, 0, 0, 1], 0)),
    )
    .unwrap();
    drop(server);

    // Reopen the stopped database and query SQL directly: this must be persisted provenance,
    // not a value computed by an entity read or a stack supplied by the test.
    let mut options = StartOptions::new(20);
    options.install_dir = Some(moqui_host::db::embedded::install_dir(&runtime));
    let (_cluster, driver) = EmbeddedCluster::start_with(&cluster_dir, &options).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    let audits = conn
        .query_once(
            "SELECT new_value_text, artifact_stack FROM entity_audit_log \
             WHERE changed_entity_name = ? AND changed_field_name = ? AND pk_primary_value = ?",
            &[
                Value::string("moqui.security.UserAccount"),
                Value::string("username"),
                Value::string("EX_JOHN_DOE"),
            ],
        )
        .unwrap();
    assert_eq!(audits.len(), 1, "startup demo username audit count");
    let row = &audits[0];
    assert_eq!(row.get("new_value_text").and_then(Value::as_str), Some("john.doe"));
    let stack = row.get("artifact_stack").and_then(Value::as_str);
    println!("persisted empty-db username audit artifact_stack: {stack:?}");
    assert_eq!(stack, Some("moqui.security.UserAccount, loadDataEmptyDb"));
}

#[test]
#[ignore = "slow: cargo test-slow"]
fn rest_enum_types_login_and_session_survive_restart() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    let runtime = temp_dir("moqui-gate2-rt");
    let cluster_dir = temp_dir("moqui-gate2-pg");
    let _runtime_guard = DirGuard(runtime.clone());
    let _cluster_guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let base =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component")).unwrap();
    std::os::unix::fs::symlink(&base, runtime.join("base-component")).unwrap();

    let seed = load_data(&runtime, &framework, &cluster_dir, &LoadFlags::default()).unwrap();
    assert!(seed.enumeration > 0, "enumeration {}", seed.enumeration);

    let demo = base.join("webroot/data/WebrootSecurityDemoData.xml");
    let demo_flags = LoadFlags {
        location: Some(demo.display().to_string()),
        data_types: Some(vec!["demo".to_string()]),
        ..LoadFlags::default()
    };
    let demo_report = load_data(&runtime, &framework, &cluster_dir, &demo_flags).unwrap();
    assert!(demo_report.records > 0, "demo records {}", demo_report.records);

    let stored = hash_password("secret").unwrap();
    let grant = runtime.join("enum-grant.xml");
    std::fs::write(
        &grant,
        format!(
            r#"<entity-facade-xml type="seed">
            <moqui.security.ArtifactGroup artifactGroupId="ENUM_TYPE" description="EnumerationType"/>
            <moqui.security.ArtifactGroupMember artifactGroupId="ENUM_TYPE"
                artifactName="moqui.basic.EnumerationType" artifactTypeEnumId="AT_ENTITY"
                nameIsPattern="N" inheritAuthz="Y"/>
            <moqui.security.ArtifactAuthz artifactAuthzId="ENUM_TYPE" userGroupId="ADMIN"
                artifactGroupId="ENUM_TYPE" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
            <moqui.security.UserAccount userId="PLAIN" username="plain" currentPassword="{}"
                passwordHashType="argon2"/>
            </entity-facade-xml>"#,
            xml_attr(&stored)
        ),
    )
    .unwrap();
    let grant_flags = LoadFlags {
        location: Some(grant.display().to_string()),
        ..LoadFlags::default()
    };
    let grant_report = load_data(&runtime, &framework, &cluster_dir, &grant_flags).unwrap();
    assert!(grant_report.records > 0, "grant records {}", grant_report.records);

    // 0 binds an ephemeral cluster port, so a test run beside another server does not need 8444 free.
    moqui_lib::env::set_property("cluster_https_port", "0");
    let bind = SocketAddr::from(([127, 0, 0, 1], 0));
    let server = start_server(&runtime, &framework, &cluster_dir, bind).unwrap();
    let address = server.address();

    let (health_status, _, health_body) = exchange(address, &http_request("GET", "/health", &[], ""));
    assert_eq!(health_status, 200, "{health_body}");
    assert_eq!(health_body, "{\"ready\":true}");

    let (metrics_status, _, metrics) = exchange(address, &http_request("GET", "/metrics", &[], ""));
    assert_eq!(metrics_status, 200, "{metrics}");
    assert!(metrics.contains("checked_out "), "{metrics}");
    assert!(metrics.contains("request_queue "), "{metrics}");
    assert!(metrics.contains("statements "), "{metrics}");

    let (anon_status, _, anon_body) = exchange(address, &http_request("GET", S1, &[], ""));
    assert_eq!(anon_status, 401, "{anon_body}");

    let (plain_status, _, plain_body) = exchange(
        address,
        &http_request("GET", S1, &[("Authorization", &basic("plain", "secret"))], ""),
    );
    assert_eq!(plain_status, 403, "{plain_body}");

    let john = basic("john.doe", "moqui");
    let (s1_status, _, s1_body) = exchange(address, &http_request("GET", S1, &[("Authorization", &john)], ""));
    assert_eq!(s1_status, 200, "{s1_body}");
    assert!(
        s1_body.contains("DataSourceType") || s1_body.contains("BooleanYN"),
        "{s1_body}"
    );

    let (e1_status, _, e1_body) = exchange(address, &http_request("GET", E1, &[("Authorization", &john)], ""));
    assert_eq!(e1_status, 200, "{e1_body}");
    assert!(
        e1_body.contains("DataSourceType") || e1_body.contains("BooleanYN"),
        "{e1_body}"
    );

    let form = "username=john.doe&password=moqui";
    let (login_status, login_headers, login_body) = exchange(
        address,
        &http_request(
            "POST",
            "/rest/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            form,
        ),
    );
    assert_eq!(login_status, 200, "{login_body}");
    assert!(login_body.contains("loggedIn"), "{login_body}");
    assert!(login_body.contains("true"), "{login_body}");
    let cookie = session_cookie(&login_headers);
    let cookie_header = format!("moqui.session={cookie}");

    let (cookie_status, _, cookie_body) =
        exchange(address, &http_request("GET", S1, &[("Cookie", &cookie_header)], ""));
    assert_eq!(cookie_status, 200, "{cookie_body}");
    assert!(
        cookie_body.contains("DataSourceType") || cookie_body.contains("BooleanYN"),
        "{cookie_body}"
    );
    drop(server);

    assert_eq!(password_hash_type(&framework, &cluster_dir).as_deref(), Some("argon2"));

    let server = start_server(&runtime, &framework, &cluster_dir, bind).unwrap();
    let (again_status, _, again_body) = exchange(
        server.address(),
        &http_request("GET", S1, &[("Cookie", &cookie_header)], ""),
    );
    assert_eq!(again_status, 200, "{again_body}");
    assert!(
        again_body.contains("DataSourceType") || again_body.contains("BooleanYN"),
        "{again_body}"
    );
}

const S1: &str = "/rest/s1/moqui/basic/enumTypes";
const E1: &str = "/rest/e1/moqui.basic.EnumerationType";

fn password_hash_type(framework: &Path, cluster_dir: &Path) -> Option<String> {
    let (cluster, driver) = EmbeddedCluster::start(cluster_dir, 20).unwrap();
    let driver: std::sync::Arc<dyn moqui_core::driver::DbDriver> = std::sync::Arc::new(driver);
    let facade = open_entity_facade(
        framework,
        driver,
        moqui_core::transaction::PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    let hash_type = hash_type_of(&facade, "john.doe");
    drop(facade);
    drop(cluster);
    hash_type
}

fn hash_type_of(facade: &EntityFacade, username: &str) -> Option<String> {
    facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("username", Value::string(username))
        .one()
        .unwrap()
        .and_then(|row| row.get("passwordHashType").and_then(Value::as_str).map(str::to_string))
}

fn basic(username: &str, password: &str) -> String {
    format!("Basic {}", base64(&format!("{username}:{password}").into_bytes()))
}

fn xml_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn http_request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (name, value) in headers {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    if !body.is_empty() || method.eq_ignore_ascii_case("POST") {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

fn exchange(address: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(count) => buf.extend_from_slice(&tmp[..count]),
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock => {
                if response_complete(&buf) {
                    break;
                }
                panic!("timed out after {} bytes: {}", buf.len(), String::from_utf8_lossy(&buf));
            }
            Err(err) => panic!("read failed after {} bytes: {err}", buf.len()),
        }
        if response_complete(&buf) {
            break;
        }
    }
    parse_response(&buf)
}

fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = find_header_end(buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..split]);
    let Some(length) = content_length(&head) else {
        return false;
    };
    buf.len() >= split + 4 + length
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> Option<usize> {
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn parse_response(buf: &[u8]) -> (u16, Vec<(String, String)>, String) {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (status, headers, body.to_string())
}

fn session_cookie(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find_map(|(name, value)| {
            if !name.eq_ignore_ascii_case("set-cookie") {
                return None;
            }
            let pair = value.split(';').next().unwrap_or("").trim();
            let (key, cookie) = pair.split_once('=')?;
            if key.trim() == "moqui.session" {
                Some(cookie.trim().to_string())
            } else {
                None
            }
        })
        .expect("Set-Cookie moqui.session")
}

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8) | (bytes[index + 2] as u32);
        push_base64(&mut out, chunk, 4);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        push_base64(&mut out, (bytes[index] as u32) << 16, 2);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
        push_base64(&mut out, chunk, 3);
        out.push('=');
    }
    out
}

fn push_base64(out: &mut String, chunk: u32, count: usize) {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let shifts = [18, 12, 6, 0];
    for shift in shifts.into_iter().take(count) {
        let piece = ((chunk >> shift) & 63) as usize;
        out.push(ALPHABET[piece] as char);
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
