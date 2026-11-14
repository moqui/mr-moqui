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

//! JUnit XML, as `moqui test --junit` and most test tools write it: the tests of a report and the ones that failed,
//! for a job's `GitRunTestFailure` rows (docs/interfaces/Pipelines.md "Reports").

use moqui_lib::node::MNode;

/// A test that failed or was an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Failure {
    pub suite: String,
    pub test: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub message: String,
    pub duration_millis: Option<i64>,
}

/// What a report holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub tests: usize,
    pub failures: Vec<Failure>,
}

/// The longest message kept of one failure.
const MESSAGE_MAX: usize = 4000;

/// The first `path:line` in `text`: a word with a dot or a slash in it, a colon and digits (`test/Order.spec.js:41`).
fn location_in(text: &str) -> Option<(String, i64)> {
    let trim: &[char] = &['(', ')', '[', ']', ',', ';', '"', '\''];
    text.split_whitespace().find_map(|word| {
        let word = word.trim_matches(trim);
        let mut parts = word.split(':');
        let path = parts.next()?;
        let line: i64 = parts.next()?.parse().ok()?;
        (path.len() > 2 && (path.contains('.') || path.contains('/')) && !path.contains("//"))
            .then(|| (path.to_string(), line))
    })
}

fn case(suite: &str, node: &MNode, report: &mut Report) {
    report.tests += 1;
    let Some(failed) = node.first_named("failure").or_else(|| node.first_named("error")) else {
        return;
    };
    let text = failed.text().unwrap_or_default();
    let mut message = failed.attribute("message").unwrap_or_default();
    if message.trim().is_empty() {
        message = text.lines().next().unwrap_or_default().to_string();
    } else if !text.trim().is_empty() && !text.trim().starts_with(message.trim()) {
        message = format!("{}\n{}", message.trim(), text.trim());
    }
    let message: String = message.chars().take(MESSAGE_MAX).collect();
    let file = node.attribute("file").filter(|file| !file.is_empty());
    let line = node.attribute("line").and_then(|line| line.trim().parse().ok());
    let (file, line) = match (file, line) {
        (Some(file), line) => (Some(file), line),
        (None, _) => match location_in(&text).or_else(|| location_in(&message)) {
            Some((file, line)) => (Some(file), Some(line)),
            None => (None, None),
        },
    };
    let duration_millis = node
        .attribute("time")
        .and_then(|time| time.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| {
            #[allow(clippy::cast_possible_truncation)]
            let millis = (seconds * 1000.0).round() as i64;
            millis
        });
    let class = node.attribute("classname").filter(|class| !class.is_empty());
    report.failures.push(Failure {
        suite: if suite.is_empty() {
            class.unwrap_or_default()
        } else {
            suite.to_string()
        },
        test: node.attribute("name").unwrap_or_default(),
        file,
        line,
        message,
        duration_millis,
    });
}

fn suite(node: &MNode, report: &mut Report) {
    let name = node.attribute("name").unwrap_or_default();
    for child in node.children() {
        match child.name().as_str() {
            "testcase" => case(&name, &child, report),
            "testsuite" => suite(&child, report),
            _ => {}
        }
    }
}

/// Read a JUnit XML report: a `testsuites` or a `testsuite` root.
///
/// # Errors
///
/// When it is not XML, or has another root.
pub fn parse(text: &str) -> Result<Report, String> {
    let root = MNode::parse_text("junit.xml", text).map_err(|err| err.message().to_string())?;
    let mut report = Report::default();
    match root.name().as_str() {
        "testsuite" => suite(&root, &mut report),
        "testsuites" => {
            for child in root.children_named("testsuite") {
                suite(&child, &mut report);
            }
        }
        other => {
            return Err(format!(
                "the root element is <{other}>, not <testsuites> or <testsuite>"
            ))
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "junit_tests.rs"]
mod tests;
