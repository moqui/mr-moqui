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

//! The browser of `moqui test-client` (plans/archive/ClientTestHarness.md 5.5, 12) against the seeded server: the fixture spec
//! `tests/fixtures/test-component/browser-client/Browser.spec.js` over the gallery screens of `/qapps` and the login
//! form, with goto, roles and names, fill, click, a dialog, settle, a provoked console error that fails its feature
//! and leaves its screenshot and text, allowPageProblems, allowProblems and takeProblems, a screenshot and a PDF. Opt-in
//! as `gate3_browser` is, because it needs Chromium (`--chromium`, `MOQUI_CHROMIUM`, `runtime/chromium`, or chromium,
//! chromium-browser or google-chrome on the `PATH`):
//!
//! ```sh
//! cargo test -p moqui-host --test client_browser
//! ```
//!
//! It also runs `--video` over an offline spec, `Video.spec.js`, when ffmpeg and ffprobe are on the `PATH`.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "test-client"))]

mod common;

// the example component loads with the repository runtime; link its tool factory as the moqui binary does
use example as _;
// the dev-aide component, linked into a dev instance
#[cfg(feature = "devtools")]
use dev_aide as _;

use std::path::PathBuf;

use common::{start, temp_dir, DirGuard};
use moqui_host::test_client::{parse_client_args, run};

/// The report lines of a run, and whether it passed.
fn run_client(args: &[&str]) -> (String, Result<(), String>) {
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let parsed = parse_client_args(&owned).unwrap();
    let mut lines = Vec::new();
    let result = run(&parsed, &mut |line| lines.push(line.to_string())).map_err(|err| err.to_string());
    (lines.join("\n"), result)
}

fn line_of<'a>(text: &'a str, start: &str) -> &'a str {
    text.lines()
        .find(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line {start}\n{text}"))
}

#[test]
fn a_browser_spec_drives_the_gallery_and_its_problems_fail_a_feature() {
    let gate = start("client-browser");
    let server = format!("http://{}", gate.server.address());
    let component = temp_dir("moqui-client-browser");
    let _component = DirGuard(component.clone());
    std::fs::create_dir_all(component.join("test/client")).unwrap();
    std::fs::write(
        component.join("component.xml"),
        "<component name=\"moqui-test-browser\" version=\"1.0.0\"/>\n",
    )
    .unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-component/browser-client/Browser.spec.js");
    std::fs::copy(&fixture, component.join("test/client/Browser.spec.js")).unwrap();
    let out = component.join("out");
    let junit = component.join("junit");
    let spec = component.join("test/client/Browser.spec.js");
    let (text, result) = run_client(&[
        spec.to_str().unwrap(),
        "--server",
        &server,
        "--out",
        out.to_str().unwrap(),
        "--junit",
        junit.to_str().unwrap(),
    ]);
    // the one failure is the provoked console error
    assert_eq!(result, Err("1 test failure(s)".to_string()), "{text}");
    for passed in [
        "Browser > the login form signs in with fill, click and waitForURL  PASSED",
        "Browser > a gallery dialog opens, takes a value and closes on Escape  PASSED",
        "Browser > a sign-out ends the kept sign-in and the next page signs in again  PASSED",
        "Browser > roles and accessible names on a fixture  PASSED",
        "Browser > allowPageProblems lets a console error pass  PASSED",
        "Browser > allowProblems and takeProblems keep a page problem from failing the feature  PASSED",
        "Browser > a screenshot, a PDF, the viewport and the color scheme  PASSED",
    ] {
        line_of(&text, passed);
    }
    line_of(&text, "Browser > a provoked console error fails the feature  FAILED");
    assert!(text.contains("the feature left 1 page problem(s)"), "{text}");
    assert!(text.contains("console.error: provoked by the spec"), "{text}");
    // the failure's screenshot and text, listed in the report and in JUnit
    let shot = out.join("failed-a-provoked-console-error-fails-the-feature-1.png");
    let dump = out.join("failed-a-provoked-console-error-fails-the-feature-1.txt");
    assert!(text.contains(&format!("attachment: {}", shot.display())), "{text}");
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));
    let dump_text = std::fs::read_to_string(&dump).unwrap();
    assert!(dump_text.contains("console.error: provoked by the spec"), "{dump_text}");
    assert!(dump_text.contains("Comparison operators"), "{dump_text}");
    let xml = std::fs::read_to_string(junit.join("TEST-moqui-test-browser.client.Browser.xml")).unwrap();
    assert!(xml.contains(&format!("[[ATTACHMENT|{}]]", shot.display())), "{xml}");
    // what the passing features wrote
    for (name, magic) in [
        ("gallery-dialog.png", &b"\x89PNG"[..]),
        ("gallery-full.png", b"\x89PNG"),
        ("gallery.pdf", b"%PDF"),
    ] {
        assert!(std::fs::read(out.join(name)).unwrap().starts_with(magic), "{name}");
    }
    // the browser and its profile are gone after the spec
    let profiles: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("moqui-test-chromium-{}-", std::process::id()))
        })
        .collect();
    assert!(profiles.is_empty(), "{profiles:?}");
}

/// `--video` with `--param`: an offline spec of two features, one skipped by the param, leaves a webm that ffprobe
/// reads with a duration, and a timeline of both features. Without Chromium, ffmpeg or ffprobe it says so and passes.
#[test]
fn video_records_a_spec_and_its_timeline() {
    let chromium = std::env::var("MOQUI_CHROMIUM").unwrap_or_default();
    if let Err(reason) = moqui_host::chromium::find_test_browser(None, &chromium) {
        eprintln!("skipped: {reason}");
        return;
    }
    for tool in ["ffmpeg", "ffprobe"] {
        if std::process::Command::new(tool).arg("-version").output().is_err() {
            eprintln!("skipped: no {tool} on the PATH");
            return;
        }
    }
    let component = temp_dir("moqui-client-video");
    let _component = DirGuard(component.clone());
    std::fs::create_dir_all(component.join("test/client")).unwrap();
    std::fs::write(
        component.join("component.xml"),
        "<component name=\"moqui-test-video\" version=\"1.0.0\"/>\n",
    )
    .unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-component/browser-client/Video.spec.js");
    std::fs::copy(&fixture, component.join("test/client/Video.spec.js")).unwrap();
    let out = component.join("out");
    let spec = component.join("test/client/Video.spec.js");
    let (text, result) = run_client(&[
        spec.to_str().unwrap(),
        "--offline",
        "--param",
        "phase=before",
        "--video",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(result, Ok(()), "{text}");
    line_of(&text, "Video > a page changes and then stays the same  PASSED");
    line_of(&text, "Video > a feature the before phase skips  SKIPPED");
    let webm = out.join("video/Video.webm");
    assert!(text.contains(&format!("Video video: {}", webm.display())), "{text}");
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(&webm)
        .output()
        .unwrap();
    let duration: f64 = String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap_or(0.0);
    assert!(duration > 0.0, "{}", String::from_utf8_lossy(&probe.stderr));
    let timeline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("video/Video.timeline.json")).unwrap()).unwrap();
    let entries = timeline.as_array().unwrap();
    assert_eq!(entries.len(), 2, "{timeline}");
    assert_eq!(entries[0]["feature"], "a page changes and then stays the same");
    assert_eq!(entries[0]["status"], "passed");
    assert_eq!(entries[0]["reason"], serde_json::Value::Null);
    assert_eq!(entries[1]["status"], "skipped");
    assert_eq!(entries[1]["reason"], "phase is before");
    let (start, end) = (
        entries[0]["start"].as_f64().unwrap(),
        entries[0]["end"].as_f64().unwrap(),
    );
    // the first feature sleeps a second; the video lasts to the end of the spec
    assert!(end - start >= 1.0, "{timeline}");
    assert!(
        duration + 0.1 >= entries[1]["end"].as_f64().unwrap(),
        "{duration} {timeline}"
    );
    // the frames are removed once encoded
    let left: Vec<_> = std::fs::read_dir(out.join("video"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
}
