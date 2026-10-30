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

/// The failures in full, then the summary line. Returns the text and how many features and specs failed.
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
