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

//! What `moqui test` and `moqui test-client` print and write: a line per feature as it finishes, the failures in full
//! at the end, a summary, and JUnit XML per spec for CI.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use moqui_devkit::testkit::{CaseResult, Failure, Outcome, SpecResult};
use moqui_lib::error::Error;

/// One spec's result, with the component it belongs to.
pub struct ComponentSpec {
    pub component: String,
    pub result: SpecResult,
}

fn seconds(duration: Duration) -> String {
    format!("{:.2}s", duration.as_secs_f64())
}

/// The lines of a case's output (a load table) under its case line, indented.
pub fn output_lines(case: &CaseResult) -> Vec<String> {
    case.output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| format!("    {line}"))
        .collect()
}

/// The line for one feature as it finishes.
pub fn case_line(spec: &str, case: &CaseResult) -> String {
    let (status, note) = match &case.outcome {
        Outcome::Passed => ("PASSED", String::new()),
        Outcome::Failed(failure) => (
            "FAILED",
            failure
                .location
                .as_ref()
                .map(|location| format!("  {location}"))
                .unwrap_or_default(),
        ),
        Outcome::Ignored(reason) => ("IGNORED", format!("  {reason}")),
        Outcome::Skipped(reason) => ("SKIPPED", format!("  {reason}")),
    };
    format!("{spec} > {}  {status} ({}){note}", case.name, seconds(case.duration))
}

fn failure_text(failure: &Failure) -> String {
    let mut out = String::new();
    let _ = write!(out, "[{} in {}", failure.kind, failure.block);
    if let Some(location) = &failure.location {
        let _ = write!(out, " at {location}");
    }
    out.push_str("]\n");
    out.push_str(&failure.message);
    out.push('\n');
    if failure.kind == "error" && !failure.stack.is_empty() {
        for line in failure.stack.lines().filter(|line| !line.trim().is_empty()).take(12) {
            let _ = writeln!(out, "  {}", line.trim());
        }
    }
    for detail in &failure.details {
        let _ = writeln!(out, "  {detail}");
    }
    for path in &failure.attachments {
        let _ = writeln!(out, "  attachment: {path}");
    }
    out
}

fn indent(text: &str) -> String {
    text.lines().map(|line| format!("    {line}\n")).collect()
}

/// A regression spec that runs longer than this fails the run (docs/Build-Test.md, "The regression budget"): it belongs
/// in the acceptance suite, or is deleted. An acceptance spec has no limit.
pub(crate) const SPEC_LIMIT: Duration = Duration::from_secs(15);

/// A component over its budget by more than this share fails the run; over the budget alone, it is a warning.
const BUDGET_MARGIN: f64 = 1.5;

/// Whether a spec result is of the acceptance suite: its display name is `<component>/test/acceptance/...`.
pub(crate) fn acceptance_result(spec: &ComponentSpec) -> bool {
    super::is_acceptance_display(&spec.result.display)
}

/// The budget a component's `test/budget` gives `key` (`server` for `moqui test`, `client` for `moqui test-client`),
/// in seconds of feature time: lines `server = 195` and `client = 30`, `#` for a comment.
pub fn read_budget(component_dir: &Path, key: &str) -> Option<f64> {
    let text = std::fs::read_to_string(component_dir.join("test/budget")).ok()?;
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .and_then(|(_, value)| value.trim().parse::<f64>().ok())
}

/// The budget check of a run of the regression suite: each regression spec over [`SPEC_LIMIT`] fails, and each
/// component's feature time (the serial sum of its cases, so workers do not hide growth) is set against its budget,
/// with a warning over it and a failure over [`BUDGET_MARGIN`] times it. `budget` gives a component's budget, if it
/// has one. Returns the text for the end of the run and the failures. Acceptance results are left out.
pub fn budget_check(specs: &[ComponentSpec], budget: &dyn Fn(&str) -> Option<f64>) -> (String, usize) {
    let mut out = String::new();
    let mut failures = 0;
    let mut slow: Vec<&ComponentSpec> = specs
        .iter()
        .filter(|spec| !acceptance_result(spec) && spec.result.duration > SPEC_LIMIT)
        .collect();
    if !slow.is_empty() {
        slow.sort_by_key(|spec| std::cmp::Reverse(spec.result.duration));
        out.push_str("\nFAILED: regression specs over 15 seconds (move each to test/acceptance/ or delete it):\n");
        for spec in slow {
            failures += 1;
            let _ = writeln!(out, "  {} ({})", spec.result.display, seconds(spec.result.duration));
        }
    }
    let mut components: Vec<(&str, f64)> = Vec::new();
    for spec in specs.iter().filter(|spec| !acceptance_result(spec)) {
        let time: f64 = spec.result.cases.iter().map(|case| case.duration.as_secs_f64()).sum();
        match components.iter_mut().find(|(name, _)| *name == spec.component) {
            Some((_, sum)) => *sum += time,
            None => components.push((spec.component.as_str(), time)),
        }
    }
    let mut lines = String::new();
    for (name, time) in &components {
        match budget(name) {
            Some(limit) if *time > limit * BUDGET_MARGIN => {
                failures += 1;
                let _ = writeln!(
                    lines,
                    "  {name}: {time:.1}s, FAILED: over 1.5 times its budget of {limit:.0}s (test/budget)"
                );
            }
            Some(limit) if *time > limit => {
                let _ = writeln!(
                    lines,
                    "  {name}: {time:.1}s, over its budget of {limit:.0}s (test/budget): cut duplicates or move scenarios to test/acceptance/"
                );
            }
            Some(limit) => {
                let _ = writeln!(lines, "  {name}: {time:.1}s of {limit:.0}s");
            }
            None => {
                let _ = writeln!(lines, "  {name}: {time:.1}s (no test/budget)");
            }
        }
    }
    if !lines.is_empty() {
        out.push_str("\nFeature time by component (serial):\n");
        out.push_str(&lines);
    }
    (out, failures)
}

/// The failures in full, then the summary line. Returns the text and how many
/// features and specs failed.
pub fn summary(specs: &[ComponentSpec], elapsed: Duration) -> (String, usize) {
    let mut out = String::new();
    let (mut passed, mut failed, mut ignored, mut skipped, mut spec_failures) = (0, 0, 0, 0, 0);
    let mut details = String::new();
    for spec in specs {
        let name = &spec.result.name;
        if let Some(failure) = &spec.result.setup_failure {
            spec_failures += 1;
            let _ = write!(
                details,
                "\nFAILED {}/{name} setup\n{}",
                spec.component,
                indent(&failure_text(failure))
            );
        }
        for case in &spec.result.cases {
            match &case.outcome {
                Outcome::Passed => passed += 1,
                Outcome::Ignored(_) => ignored += 1,
                Outcome::Skipped(_) => skipped += 1,
                Outcome::Failed(failure) => {
                    failed += 1;
                    let _ = write!(
                        details,
                        "\nFAILED {name} > {}\n{}",
                        case.name,
                        indent(&failure_text(failure))
                    );
                }
            }
        }
        if let Some(failure) = &spec.result.cleanup_failure {
            spec_failures += 1;
            let _ = write!(
                details,
                "\nFAILED {}/{name} cleanup\n{}",
                spec.component,
                indent(&failure_text(failure))
            );
        }
    }
    if !details.is_empty() {
        out.push_str("\nFailures:\n");
        out.push_str(&details);
    }
    let _ = writeln!(
        out,
        "\n{} features: {passed} passed, {failed} failed, {ignored} ignored, {skipped} skipped; {} specs{} in {}",
        passed + failed + ignored + skipped,
        specs.len(),
        if spec_failures > 0 {
            format!(", {spec_failures} spec setup or cleanup failures")
        } else {
            String::new()
        },
        seconds(elapsed)
    );
    (out, failed + spec_failures)
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no other control characters.
            ch if (ch as u32) < 0x20 && ch != '\n' && ch != '\r' && ch != '\t' => {}
            ch => out.push(ch),
        }
    }
    out
}

/// One JUnit `testsuite` document for a spec. `infix` follows the component in the class name: `.client` for a client
/// spec.
pub fn junit_xml(spec: &ComponentSpec, infix: &str) -> String {
    let result = &spec.result;
    let class = format!("{}{infix}.{}", spec.component, result.name);
    let mut cases = String::new();
    let (mut failures, mut errors, mut skipped) = (0, 0, 0);
    let mut write_failure = |cases: &mut String, failure: &Failure| {
        let tag = if failure.kind == "assertion" {
            "failure"
        } else {
            "error"
        };
        if tag == "failure" {
            failures += 1;
        } else {
            errors += 1;
        }
        let first = failure.message.lines().next().unwrap_or("");
        let _ = write!(
            cases,
            "<{tag} message=\"{}\" type=\"{}\">{}</{tag}>",
            xml_escape(first),
            xml_escape(&failure.kind),
            xml_escape(&failure_text(failure))
        );
    };
    // One system-out per case: its output (a load table), then the attachments in the attachments plugin's form,
    // which Jenkins and GitLab read.
    let system_out = |cases: &mut String, output: &str, attachments: &[String]| {
        if output.is_empty() && attachments.is_empty() {
            return;
        }
        cases.push_str("<system-out>");
        if !output.is_empty() {
            cases.push_str(&xml_escape(output));
            if !output.ends_with('\n') && !attachments.is_empty() {
                cases.push('\n');
            }
        }
        for path in attachments {
            let _ = writeln!(cases, "[[ATTACHMENT|{}]]", xml_escape(path));
        }
        cases.push_str("</system-out>");
    };
    let mut count = result.cases.len();
    for (name, failure) in [
        ("setupSpec", &result.setup_failure),
        ("cleanupSpec", &result.cleanup_failure),
    ] {
        if let Some(failure) = failure {
            count += 1;
            let _ = write!(
                cases,
                "  <testcase name=\"{name}\" classname=\"{}\" time=\"0\">",
                xml_escape(&class)
            );
            write_failure(&mut cases, failure);
            system_out(&mut cases, "", &failure.attachments);
            cases.push_str("</testcase>\n");
        }
    }
    for case in &result.cases {
        let _ = write!(
            cases,
            "  <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\">",
            xml_escape(&case.name),
            xml_escape(&class),
            case.duration.as_secs_f64()
        );
        let attachments: &[String] = match &case.outcome {
            Outcome::Passed => &[],
            Outcome::Failed(failure) => {
                write_failure(&mut cases, failure);
                &failure.attachments
            }
            Outcome::Ignored(reason) | Outcome::Skipped(reason) => {
                skipped += 1;
                let _ = write!(cases, "<skipped message=\"{}\"/>", xml_escape(reason));
                &[]
            }
        };
        system_out(&mut cases, &case.output, attachments);
        cases.push_str("</testcase>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"{}\" tests=\"{count}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{:.3}\">\n  <properties><property name=\"file\" value=\"{}\"/></properties>\n{cases}</testsuite>\n",
        xml_escape(&class),
        result.duration.as_secs_f64(),
        xml_escape(&result.display)
    )
}

/// Write `TEST-<component><infix>.<Spec>.xml` for each spec into `dir`.
pub fn write_junit(dir: &Path, specs: &[ComponentSpec], infix: &str) -> Result<(), Error> {
    std::fs::create_dir_all(dir)
        .map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    for spec in specs {
        let path = dir.join(format!("TEST-{}{infix}.{}.xml", spec.component, spec.result.name));
        std::fs::write(&path, junit_xml(spec, infix))
            .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ComponentSpec {
        ComponentSpec {
            component: "comp-a".to_string(),
            result: SpecResult {
                name: "OrderFlow".to_string(),
                display: "comp-a/test/OrderFlow.spec.js".to_string(),
                cases: vec![
                    CaseResult {
                        name: "place <order>".to_string(),
                        outcome: Outcome::Passed,
                        duration: Duration::from_millis(1240),
                        output: "4 users, 10.0 s: 2,412 requests".to_string(),
                    },
                    CaseResult {
                        name: "ship".to_string(),
                        outcome: Outcome::Failed(Failure {
                            kind: "assertion".to_string(),
                            block: "then".to_string(),
                            message: "Condition not satisfied:\n\na == 1\n|\n2".to_string(),
                            location: Some("comp-a/test/OrderFlow.spec.js:12".to_string()),
                            stack: String::new(),
                            details: vec!["data check: Field x did not match".to_string()],
                            attachments: vec!["out/failed-ship-1.png".to_string()],
                        }),
                        duration: Duration::from_millis(30),
                        output: String::new(),
                    },
                    CaseResult {
                        name: "later".to_string(),
                        outcome: Outcome::Ignored("not yet".to_string()),
                        duration: Duration::ZERO,
                        output: String::new(),
                    },
                ],
                setup_failure: None,
                cleanup_failure: None,
                duration: Duration::from_secs(2),
            },
        }
    }

    #[test]
    fn the_budget_check_fails_a_long_spec_and_a_component_far_over_its_budget() {
        // sample(): 1.27 s of case time, 2 s of spec time
        let mut long = sample();
        long.result.duration = Duration::from_secs(16);
        let mut accepted = sample();
        accepted.result.name = "Walk".to_string();
        accepted.result.display = "comp-a/test/acceptance/Walk.spec.js".to_string();
        accepted.result.duration = Duration::from_secs(50);
        let (text, failures) = budget_check(&[long, accepted], &|_| None);
        assert_eq!(failures, 1, "{text}");
        assert!(
            text.contains("FAILED: regression specs over 15 seconds (move each to test/acceptance/ or delete it):\n  comp-a/test/OrderFlow.spec.js (16.00s)\n"),
            "{text}"
        );
        assert!(!text.contains("Walk.spec.js"), "{text}");
        assert!(text.contains("  comp-a: 1.3s (no test/budget)"), "{text}");
        // within, over (a warning) and over 1.5 times (a failure)
        let spec = || vec![sample()];
        assert_eq!(
            budget_check(&spec(), &|_| Some(2.0)),
            (
                "\nFeature time by component (serial):\n  comp-a: 1.3s of 2s\n".to_string(),
                0
            )
        );
        let (over, failures) = budget_check(&spec(), &|_| Some(1.0));
        assert!(over.contains("over its budget of 1s") && failures == 0, "{over}");
        let (far, failures) = budget_check(&spec(), &|_| Some(0.5));
        assert!(
            far.contains("FAILED: over 1.5 times its budget") && failures == 1,
            "{far}"
        );
    }

    #[test]
    fn a_budget_file_names_each_part() {
        let dir = std::env::temp_dir().join(format!("moqui-budget-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::write(
            dir.join("test/budget"),
            "# seconds of feature time\nserver = 195\nclient = 30 # walks\n",
        )
        .unwrap();
        assert_eq!(read_budget(&dir, "server"), Some(195.0));
        assert_eq!(read_budget(&dir, "client"), Some(30.0));
        assert_eq!(read_budget(&dir, "other"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lines_summary_and_junit() {
        let spec = sample();
        assert_eq!(
            case_line("OrderFlow", &spec.result.cases[0]),
            "OrderFlow > place <order>  PASSED (1.24s)"
        );
        assert_eq!(
            case_line("OrderFlow", &spec.result.cases[1]),
            "OrderFlow > ship  FAILED (0.03s)  comp-a/test/OrderFlow.spec.js:12"
        );
        let (text, failed) = summary(std::slice::from_ref(&spec), Duration::from_secs(3));
        assert_eq!(failed, 1);
        assert!(text.contains("FAILED OrderFlow > ship\n    [assertion in then at comp-a/test/OrderFlow.spec.js:12]\n    Condition not satisfied:"), "{text}");
        assert!(text.contains("      data check: Field x did not match"), "{text}");
        assert!(
            text.ends_with("3 features: 1 passed, 1 failed, 1 ignored, 0 skipped; 1 specs in 3.00s\n"),
            "{text}"
        );
        assert!(text.contains("      attachment: out/failed-ship-1.png"), "{text}");
        let xml = junit_xml(&spec, "");
        assert!(
            xml.contains("<testsuite name=\"comp-a.OrderFlow\" tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\""),
            "{xml}"
        );
        assert!(xml.contains("<testcase name=\"place &lt;order&gt;\""), "{xml}");
        assert!(
            xml.contains("<failure message=\"Condition not satisfied:\" type=\"assertion\">"),
            "{xml}"
        );
        assert!(
            xml.contains("<system-out>[[ATTACHMENT|out/failed-ship-1.png]]\n</system-out></testcase>"),
            "{xml}"
        );
        assert!(junit_xml(&spec, ".client").contains("<testsuite name=\"comp-a.client.OrderFlow\""));
        assert!(
            xml.contains("time=\"1.240\"><system-out>4 users, 10.0 s: 2,412 requests</system-out></testcase>"),
            "{xml}"
        );
        assert_eq!(
            output_lines(&spec.result.cases[0]),
            vec!["    4 users, 10.0 s: 2,412 requests".to_string()]
        );
    }
}
