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
    }
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
