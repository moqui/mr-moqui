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

use super::*;

#[test]
fn a_tap_takes_the_lines_at_its_level_and_above() {
    // The taps are the process's: the other tests' lines reach them too, so only these lines count.
    let warnings = tap_at(64, Level::WARN);
    let all = tap(64);
    feed_taps(Level::INFO, "tap test: an info line");
    feed_taps(Level::WARN, "tap test: a warning");
    feed_taps(Level::ERROR, "tap test: an error");
    let mine = |lines: std::sync::mpsc::TryIter<'_, String>| {
        lines.filter(|line| line.starts_with("tap test: ")).collect::<Vec<_>>()
    };
    assert_eq!(mine(warnings.try_iter()), ["tap test: a warning", "tap test: an error"]);
    assert_eq!(mine(all.try_iter()).len(), 3);
}

#[test]
fn an_app_log_carries_the_user_the_visitor_and_the_thrown_message() {
    install();
    let bare = format!("log-bare-{}", std::process::id());
    tracing::error!("{bare}");
    let entry = take_app_log(&bare).expect("the subscriber did not queue the line");
    assert!(
        entry.get("user_id").is_none() && entry.get("visitor_id").is_none(),
        "{entry:?}"
    );

    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    moqui_core::user::set_current_log_actor("user-1", "visitor-1");
    let marked = format!("log-actor-{}", std::process::id());
    tracing::error!(error = "disk full", "{marked}");
    let entry = take_app_log(&marked).expect("the subscriber did not queue the line");
    assert_eq!(entry.get("user_id").and_then(Value::as_str), Some("user-1"));
    assert_eq!(entry.get("visitor_id").and_then(Value::as_str), Some("visitor-1"));
    let thrown = match entry.get("thrown") {
        Some(Value::Map(map)) => map,
        other => panic!("thrown missing: {other:?}"),
    };
    assert_eq!(thrown.get("message").and_then(Value::as_str), Some("disk full"));
    assert!(entry.get("mdc").is_none(), "{entry:?}");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-logging-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rolled(dir: &Path, base: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(&format!("{base}.")) && name != &format!("{base}.log"))
        .collect();
    names.sort();
    names
}

#[test]
fn a_file_rolls_by_size_and_keeps_the_newest() {
    let dir = scratch("size");
    let mut log = Rolling::open(&dir, "moqui", 100, 2).unwrap();
    for index in 0..20 {
        log.write(&format!("line {index:02} {}", "x".repeat(30)));
        // distinct modified times, so the oldest is well defined
        std::thread::sleep(Duration::from_millis(15));
    }
    let names = rolled(&dir, "moqui");
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names
            .iter()
            .all(|name| name.ends_with(".log") && name.contains(&today())),
        "{names:?}"
    );
    let current = std::fs::read_to_string(dir.join("moqui.log")).unwrap();
    assert!(current.contains("line 19"), "{current}");
    // the current file and the rolled ones are this user's alone, whatever the umask (SEC-DAT-06)
    #[cfg(unix)]
    for name in names.iter().map(String::as_str).chain(["moqui.log"]) {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{name}");
    }
    assert!(!std::fs::read_to_string(dir.join(&names[0]))
        .unwrap()
        .contains("line 00"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_from_an_earlier_day_rolls_before_the_first_write() {
    let dir = scratch("day");
    let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
    log.write("first");
    log.day = "2000-01-01".to_string();
    log.write("second");
    let names = rolled(&dir, "moqui");
    assert_eq!(names, vec!["moqui.2000-01-01.1.log".to_string()]);
    assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "second\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_cannot_be_reopened_is_tried_again() {
    let dir = scratch("reopen");
    let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
    log.retry_every = Duration::from_millis(50);
    log.write("first");
    // The roll's move and reopen both fail while the directory is gone.
    std::fs::remove_dir_all(&dir).unwrap();
    log.day = "2000-01-01".to_string();
    log.write("lost");
    assert!(log.file.is_none());
    std::fs::create_dir_all(&dir).unwrap();
    log.write("too soon");
    assert!(log.file.is_none());
    std::thread::sleep(Duration::from_millis(60));
    log.write("back");
    assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "back\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_failed_roll_keeps_writing_the_current_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("keep");
    let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
    log.retry_every = Duration::from_millis(50);
    log.write("first");
    // A directory that takes no new names: the move to the rolled name fails.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let locked = std::fs::write(dir.join("probe"), "").is_err();
    if locked {
        log.day = "2000-01-01".to_string();
        log.write("second");
        assert!(rolled(&dir, "moqui").is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("moqui.log")).unwrap(),
            "first\nsecond\n"
        );
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if locked {
        std::thread::sleep(Duration::from_millis(60));
        log.write("third");
        assert_eq!(rolled(&dir, "moqui"), vec!["moqui.2000-01-01.1.log".to_string()]);
        assert_eq!(
            std::fs::read_to_string(dir.join("moqui.2000-01-01.1.log")).unwrap(),
            "first\nsecond\n"
        );
        assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "third\n");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn warnings_and_errors_go_to_the_error_file_too() {
    let dir = scratch("errors");
    let mut sinks = FileSinks {
        all: Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap(),
        errors: Rolling::open(&dir, "moqui-error", 1_000_000, 5).unwrap(),
    };
    sinks.write(Level::INFO, "info line");
    sinks.write(Level::WARN, "warn line");
    sinks.write(Level::ERROR, "error line");
    assert_eq!(
        std::fs::read_to_string(dir.join("moqui.log")).unwrap().lines().count(),
        3
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("moqui-error.log")).unwrap(),
        "warn line\nerror line\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_log_dir_is_a_runtime_location_a_path_or_empty() {
    let runtime = Path::new("/srv/moqui/runtime");
    assert_eq!(
        file_dir(runtime, "runtime://log"),
        Some(PathBuf::from("/srv/moqui/runtime/log"))
    );
    assert_eq!(
        file_dir(runtime, "/var/log/moqui"),
        Some(PathBuf::from("/var/log/moqui"))
    );
    assert_eq!(
        file_dir(runtime, "logs"),
        Some(PathBuf::from("/srv/moqui/runtime/logs"))
    );
    assert_eq!(file_dir(runtime, "  "), None);
}

const STAMP: &str = "2026-10-15 01:48:49.933";

fn text<'a>(level: Level, target: &'a str, message: &'a str, extra: &'a [(String, String)]) -> LogText<'a> {
    LogText {
        stamp: STAMP,
        level,
        thread: "moqui-http",
        target,
        message,
        extra,
        error: None,
    }
}

#[test]
fn a_field_named_error_is_on_the_file_and_console_lines() {
    // The cause of a failure was kept only for telemetry's thrown, so the log showed the message and no cause.
    install();
    let lines = tap(64);
    let marked = format!("log-error-field-{}", std::process::id());
    let cause = std::io::Error::other("disk full");
    tracing::error!(error = %cause, table = "EMAIL_MESSAGE", "{marked}");
    let line = lines
        .try_iter()
        .find(|line| line.contains(&marked))
        .expect("the tap did not see the line");
    assert!(
        line.ends_with(&format!("{marked} table=EMAIL_MESSAGE error=disk full")),
        "{line}"
    );
    let _ = take_app_log(&marked);

    let event = LogText {
        error: Some("disk full"),
        ..text(Level::ERROR, "moqui_entity::entity::meta", "no table", &[])
    };
    assert!(plain_line(&event).ends_with("no table error=disk full"));
    assert!(console_line(&event, false).ends_with("no table error=disk full"));
}

#[test]
fn the_file_line_stays_plain() {
    let message = "Setting property instance_purpose from default with value [dev]";
    let line = plain_line(&text(Level::INFO, "moqui_core::context::factory", message, &[]));
    assert_eq!(
        line,
        "2026-10-15 01:48:49.933 INFO  [moqui-http] moqui_core::context::factory - Setting property instance_purpose from default with value [dev]"
    );
    assert!(!line.contains('\u{1b}'));
    let extra = [("name".to_string(), "value".to_string())];
    let with_extra = plain_line(&text(Level::WARN, "moqui_host::db::postgres", "hello", &extra));
    assert_eq!(
        with_extra,
        "2026-10-15 01:48:49.933 WARN  [moqui-http] moqui_host::db::postgres - hello name=value"
    );
    assert!(!with_extra.contains('\u{1b}'));
}

#[test]
fn the_console_line_is_structured_and_the_level_is_colored() {
    let message = "Setting property instance_purpose from default with value [dev]";
    let plain = console_line(&text(Level::INFO, "moqui_core::context::factory", message, &[]), false);
    assert_eq!(
        plain,
        "2026-10-15 01:48:49.933  INFO   [moqui-http] core::context::factory  Setting property instance_purpose from default with value [dev]"
    );
    assert!(!plain.contains('\u{1b}'));

    let cases = [
        (Level::TRACE, "35", "TRACE"),
        (Level::DEBUG, "1;35", "DEBUG"),
        (Level::INFO, "1;32", "INFO"),
        (Level::WARN, "1;33", "WARN"),
        (Level::ERROR, "1;31", "ERROR"),
    ];
    for (level, sgr, word) in cases {
        let line = console_line(&text(level, "moqui_core::context::factory", "hello", &[]), true);
        assert!(
            line.contains(&format!("\u{1b}[{sgr}m{word}\u{1b}[0m")),
            "{word} in {line:?}"
        );
        let at = line.find("hello").unwrap();
        let last = line[..at].rfind('\u{1b}').unwrap();
        assert!(
            line[last..at].starts_with("\u{1b}[0m"),
            "{word} message span in {line:?}"
        );
    }
    let extra = [("name".to_string(), "value".to_string())];
    let named = console_line(
        &text(Level::INFO, "moqui_core::context::factory", "hello", &extra),
        true,
    );
    assert!(named.contains(" \u{1b}[2mname\u{1b}[0m=value"), "{named:?}");
}

#[test]
fn the_console_shortens_only_the_moqui_crate_prefix() {
    let message = "hello";
    for (full, short) in [
        ("moqui_core::context::factory", "core::context::factory"),
        ("moqui_host::db::postgres", "host::db::postgres"),
        ("moqui_lib::env", "lib::env"),
        ("moqui_db::sql", "db::sql"),
        ("moqui_core", "core"),
    ] {
        let shown = console_line(&text(Level::INFO, full, message, &[]), false);
        assert!(shown.contains(short), "{full} -> {shown}");
        assert!(!shown.contains(full), "{full} stayed in {shown}");
        let stored = plain_line(&text(Level::INFO, full, message, &[]));
        assert!(stored.contains(full), "{stored}");
    }
    for keep in ["moqui::main", "other::target"] {
        let shown = console_line(&text(Level::INFO, keep, message, &[]), false);
        assert!(shown.contains(keep), "{shown}");
    }
}

#[test]
fn a_second_line_indents_on_the_console_and_stays_flush_in_the_file() {
    let message = "first\nsecond";
    let stored = plain_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]));
    assert!(stored.contains("first\nsecond"), "{stored:?}");
    assert!(!stored.contains('|'));

    let shown = console_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]), false);
    let (first, second) = shown.split_once('\n').unwrap();
    let col = first.find("first").unwrap();
    assert_eq!(&second[col..], "second");
    assert_eq!(&second[col - 2..col], "| ");
    assert!(second[..col - 2].chars().all(|ch| ch == ' '));
    assert!(!shown.contains('\u{1b}'));

    let colored = console_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]), true);
    assert!(colored.contains("\u{1b}[2m|\u{1b}[0m second"), "{colored:?}");
    let stripped = strip_ansi(&colored);
    let (first, second) = stripped.split_once('\n').unwrap();
    let col = first.find("first").unwrap();
    assert_eq!(&second[col..], "second");
}

#[test]
fn color_follows_the_mode_the_terminal_and_no_color() {
    assert!(color_enabled("auto", false, true));
    assert!(!color_enabled("auto", false, false));
    assert!(!color_enabled("auto", true, true));
    assert!(color_enabled("", false, true));
    assert!(color_enabled("always", false, false));
    assert!(!color_enabled("never", false, true));
    assert!(color_enabled("always", true, false));
    assert!(!color_enabled(" NEVER ", false, true));
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('\u{1b}') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find('m').map_or(after.len(), |at| at + 1);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn a_target_rule_lets_its_debug_lines_through_until_it_is_reverted() {
    use moqui_core::live_settings::{self, catalog, Change};
    install();
    let lines = tap(4096);
    let target = "moqui_settings_test";
    let mine = |lines: &std::sync::mpsc::Receiver<String>, part: &str| {
        lines.try_iter().filter(|line| line.contains(part)).count()
    };
    tracing::debug!(target: "moqui_settings_test", "settings-before");
    assert_eq!(mine(&lines, "settings-before"), 0, "DEBUG is off at the default level");

    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let factory = context.factory();
    let key = catalog::target_key(target);
    let change = Change {
        key: &key,
        value: "debug",
        minutes: 15,
        reason: "a logging test",
        user: "admin",
    };
    live_settings::set(factory, &change).unwrap();
    tracing::debug!(target: "moqui_settings_test", "settings-during");
    tracing::trace!(target: "moqui_settings_test", "settings-trace");
    tracing::debug!(target: "moqui_settings_other", "settings-elsewhere");
    tracing::info!(target: "rsipstack::transport", "settings-sip");
    assert_eq!(mine(&lines, "settings-during"), 1, "the rule's target takes DEBUG");
    assert_eq!(mine(&lines, "settings-trace"), 0);
    assert_eq!(mine(&lines, "settings-elsewhere"), 0, "other targets stay at the global level");
    assert_eq!(mine(&lines, "settings-sip"), 0, "rsipstack is held at WARN");

    live_settings::revert(factory, &key, "admin").unwrap();
    tracing::debug!(target: "moqui_settings_test", "settings-after");
    assert_eq!(mine(&lines, "settings-after"), 0, "the revert stops them");
}

/// The node's own telemetry store, as a test sees it: each document exported, with its index.
#[cfg(feature = "telemetry")]
#[derive(Default)]
struct Kept(Mutex<Vec<(String, Document)>>);

#[cfg(feature = "telemetry")]
impl moqui_core::telemetry::Exporter for Kept {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, moqui_lib::error::Error> {
        let count = documents.len();
        lock(&self.0).extend(documents.into_iter().map(|document| (index.to_string(), document)));
        Ok(count)
    }
}

/// A collector on a loopback port that answers every request `200 {}`: each request's path, content type and body.
#[cfg(feature = "telemetry")]
fn collector() -> (String, std::sync::mpsc::Receiver<(String, String, Vec<u8>)>) {
    use std::io::Read as _;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8192];
            let (head, length) = loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break (String::new(), 0),
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    request.drain(..end + 4);
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    break (head, length);
                }
            };
            while request.len() < length {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let content_type = head
                .lines()
                .find_map(|line| line.strip_prefix("content-type:"))
                .unwrap_or("")
                .trim()
                .to_string();
            let _ = sender.send((path, content_type, request));
            let _ = std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
            );
        }
    });
    (url, received)
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-16.4.3 and v5.0.0-16.2.4 (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md),
/// in-process. 16.4.3, a Configuration row: with `telemetry@otlp-endpoint` set, the lines the server logs go to that
/// collector, a system apart from the node, over OTLP/HTTP, as well as to the node's own store; with no endpoint (the
/// default) the exporter is the node's store alone. 16.2.4: they arrive in the OpenTelemetry log data model, which
/// collectors and log stores read as it is: a line logged in a context reaches a collector the test plays as a
/// `logRecords` entry of `/v1/logs`, its time in Unix nanoseconds (the line's instant), the OpenTelemetry severity (17,
/// ERROR), the message as its body, and the thread, host, user and exception under the semantic convention names; a
/// request's trace and span ids are added when the line is logged inside one (`sdk_tree_log_correlation_and_delta_metrics`,
/// moqui-core telemetry tests), and a 500's error id is in both the answer and the line
/// (`an_unexpected_error_and_a_backend_tls_failure_are_logged_at_error`). The deployer runs the collector and a store
/// the node's credentials cannot change, reached over https, with `otlp-headers` for its credentials.
#[cfg(feature = "telemetry")]
#[test]
fn a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record() {
    install();
    let none = moqui_core::context::ExecutionContextFactory::test_context();
    let alone = Arc::new(Kept::default());
    let exporter = moqui_core::telemetry::otlp::configured(none.factory(), alone.clone());
    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&exporter), Arc::as_ptr(&alone)),
        "no endpoint: the node's store alone"
    );

    let (url, received) = collector();
    let context = moqui_core::context::ExecutionContextFactory::test_context_with_conf(&format!(
        r#"<moqui-conf><telemetry otlp-endpoint="{url}" otlp-protocol="http/json" otlp-compression="none"/></moqui-conf>"#
    ));
    let factory = Arc::clone(context.factory());
    let telemetry = moqui_core::telemetry::for_factory(&factory);
    let kept = Arc::new(Kept::default());
    let exporter = moqui_core::telemetry::otlp::configured(&factory, kept.clone());
    assert!(!std::ptr::addr_eq(Arc::as_ptr(&exporter), Arc::as_ptr(&kept)));
    telemetry.set_exporter(exporter);
    let marker = format!("otlp-proof-{}", std::process::id());
    let before = now_millis();
    {
        let _entered = context.enter();
        moqui_core::user::set_current_log_actor("user-otlp", "visitor-otlp");
        tracing::error!(error = "disk full", "{marker}");
    }
    let after = now_millis();
    telemetry.flush();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let record = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let (path, content_type, body) = received.recv_timeout(left).expect("the collector got the line");
        assert_eq!((path.as_str(), content_type.as_str()), ("/v1/logs", "application/json"));
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let found = request["resourceLogs"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|resource| resource["scopeLogs"].as_array().into_iter().flatten())
            .flat_map(|scope| scope["logRecords"].as_array().into_iter().flatten())
            .find(|record| record["body"]["stringValue"] == marker.as_str())
            .cloned();
        if let Some(record) = found {
            break record;
        }
    };
    let nanos: i64 = record["timeUnixNano"].as_str().unwrap().parse().unwrap();
    assert!((before..=after).contains(&(nanos / 1_000_000)), "{record}");
    assert_eq!(record["severityNumber"], 17);
    assert_eq!(record["severityText"], "ERROR");
    let attribute = |key: &str| {
        record["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attribute| attribute["key"] == key)
            .map(|attribute| {
                attribute["value"]["stringValue"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
    };
    assert_eq!(attribute("user.id").as_deref(), Some("user-otlp"), "{record}");
    assert_eq!(attribute("exception.message").as_deref(), Some("disk full"), "{record}");
    assert_eq!(attribute("host.name").as_deref(), Some(host_name()), "{record}");
    assert!(attribute("thread.name").is_some(), "{record}");
    assert!(
        lock(&kept.0).iter().any(|(index, document)| index == "otel_logs"
            && document.get("body").and_then(Value::as_str) == Some(marker.as_str())),
        "the node's store has it too"
    );
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-16.2.3 (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md),
/// in-process: a log line goes only to the destinations Monitoring.md and Attack-Surface-Process name. In `logging@dir`
/// the files are `moqui.log` and, for warnings and errors, `moqui-error.log` (rolled to `moqui.<day>.<n>.log` and
/// `moqui-error.<day>.<n>.log`), and nothing else is written there; telemetry goes to the node's own store, and to an
/// OTLP collector only when `telemetry@otlp-endpoint` names one
/// (`a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record`). The other destinations, read in
/// `LineSubscriber::event`, stay on the node: stderr, the taps of the terminal and SSH sessions, and the dev tools'
/// capture of a run. The inventory itself is 16.1.1's documentation.
#[test]
fn log_lines_are_written_only_to_the_named_files() {
    let dir = scratch("destinations").join("log");
    moqui_lib::private_fs::create_dir_all(&dir).unwrap();
    let mut sinks = FileSinks {
        all: Rolling::open(&dir, "moqui", 200, 3).unwrap(),
        errors: Rolling::open(&dir, "moqui-error", 200, 3).unwrap(),
    };
    for index in 0..30 {
        for level in [Level::TRACE, Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR] {
            sinks.write(level, &format!("destination line {index} {level} {}", "x".repeat(20)));
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert!(names.contains(&"moqui.log".to_string()) && names.contains(&"moqui-error.log".to_string()));
    let day = today();
    for name in &names {
        let rolled = ["moqui.", "moqui-error."].iter().any(|base| {
            name.strip_prefix(base)
                .and_then(|rest| rest.strip_suffix(".log"))
                .and_then(|rest| rest.strip_prefix(&format!("{day}.")))
                .is_some_and(|n| n.parse::<u32>().is_ok())
        });
        assert!(
            name == "moqui.log" || name == "moqui-error.log" || rolled,
            "{name} in {names:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-16.2.2 (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md),
/// in-process, of a Configuration row: the timestamps of log events are UTC. A line logged now has the stamp
/// (`yyyy-MM-dd HH:mm:ss.SSS`) of its instant in UTC, written from a `jiff::Timestamp`, which has no zone, never the
/// host's local time; the record the line becomes in telemetry carries the same instant as epoch milliseconds. On a
/// host whose zone is UTC the stamp check cannot tell the two apart; the code read says which. The stamp has no zone
/// marker, so a reader of the files must know it is UTC (Monitoring.md). The deployer keeps the host's clock
/// synchronized (NTP), which the framework does not check.
#[test]
fn a_log_lines_stamp_is_its_instant_in_utc() {
    install();
    let lines = tap(4096);
    let marker = format!("utc-proof-{}", std::process::id());
    tracing::warn!("{marker}");
    let line = lines
        .try_iter()
        .find(|line| line.contains(&marker))
        .expect("the tap got the line");
    let entry = take_app_log(&marker).expect("the subscriber queued the line");
    let millis = match entry.get("@timestamp") {
        Some(Value::I64(millis)) => *millis,
        other => panic!("@timestamp: {other:?}"),
    };
    let utc = jiff::Timestamp::from_millisecond(millis)
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    assert!(line.starts_with(&format!("{utc} WARN")), "{line} at {utc}");
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-16.4.2 (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md),
/// in-process, of a Configuration row: the log files are the server user's alone, whatever the umask. The directory is
/// made 0700 and `moqui.log`, `moqui-error.log` and the files they roll to 0600, as `attach_files` opens them (through
/// `moqui_lib::private_fs`); the audit log of entity changes (`EntityAuditLog`) is written by its own services only,
/// never by the generic entity tools, REST or data loads, while an ordinary entity is. The server user can still change
/// its own files and rows, and the sign-in history (`UserLoginHistory`) and authorization failures
/// (`ArtifactAuthzFailure`) are ordinary entities: a copy that cannot be changed is the deployer's, by the OTLP export
/// to a store apart (16.4.3) or a shipper that appends the files elsewhere.
#[cfg(unix)]
#[test]
fn the_log_files_are_the_server_users_alone() {
    use std::os::unix::fs::PermissionsExt as _;

    let base = scratch("private");
    let dir = base.join("runtime/log");
    moqui_lib::private_fs::create_dir_all(&dir).unwrap();
    let mut sinks = FileSinks {
        all: Rolling::open(&dir, "moqui", 150, 3).unwrap(),
        errors: Rolling::open(&dir, "moqui-error", 150, 3).unwrap(),
    };
    for index in 0..20 {
        sinks.write(Level::WARN, &format!("private line {index} {}", "x".repeat(40)));
    }
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(dir.parent().unwrap()), 0o700, "each directory it made");
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(mode(&path), 0o600, "{}", path.display());
        files += 1;
    }
    assert!(files > 2, "rolled files too: {files}");
    let _ = std::fs::remove_dir_all(base);

    use moqui_core::entity::sensitive::is_generic_write_refused;
    assert!(is_generic_write_refused("moqui.entity.EntityAuditLog"));
    assert!(!is_generic_write_refused("moqui.example.Example"));
}
