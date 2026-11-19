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

use std::time::Duration;

use moqui_devkit::testkit::{CaseResult, Failure, SpecResult};

use super::*;

#[test]
fn a_proof_is_filed_by_the_id_its_name_starts_with() {
    assert_eq!(
        requirement_of("v5.0.0-7.2.4: a sign-in issues a new session id"),
        Some("v5.0.0-7.2.4")
    );
    assert_eq!(requirement_of("v5.0.0-1.10.12:x"), Some("v5.0.0-1.10.12"));
    for name in [
        "A07 a sign-in issues a new session id",
        "v5.0.0-7.2: two parts",
        "v5.0-7.2.4: a short version",
        "v5.0.0-7.2.x: not a number",
        "5.0.0-7.2.4: no v",
        "v5.0.0-7.2.4 no colon",
        "(every feature)",
    ] {
        assert_eq!(requirement_of(name), None, "{name}");
    }
    let mut ids = vec!["v5.0.0-1.10.1", "v5.0.0-1.9.1", "v5.0.0-10.1.1", "v5.0.0-2.1.1"];
    ids.sort_by_key(|id| id_key(id));
    assert_eq!(
        ids,
        vec!["v5.0.0-1.9.1", "v5.0.0-1.10.1", "v5.0.0-2.1.1", "v5.0.0-10.1.1"]
    );
}

#[test]
fn matrix_rows_are_read_by_their_header() {
    let text = "\
# V3 Web Frontend Security

| Column | Holds |
| --- | --- |
| Requirement | The id |

## V3.4 Browser Security Mechanism Headers

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.4.1 | 1 | Strict transport on every answer | Framework | `headers.rs` with a \\| pipe | SecurityHeaders \"A05\" |
| v5.0.0-3.4.2 | 1 | Origins allowed by a list | Configuration | `webapp@allow-origins` | - |
| V3.4.3 | 2 | not an id of ours | Gap | - | - |

| Status | Meaning |
| --- | --- |
| Framework | Met by default |
";
    assert_eq!(
        matrix_rows(text),
        vec![
            Row {
                id: "v5.0.0-3.4.1".to_string(),
                level: "1".to_string(),
                topic: "Strict transport on every answer".to_string(),
                status: "Framework".to_string(),
            },
            Row {
                id: "v5.0.0-3.4.2".to_string(),
                level: "1".to_string(),
                topic: "Origins allowed by a list".to_string(),
                status: "Configuration".to_string(),
            },
        ]
    );
    assert_eq!(
        cells("| a | b \\| c | `d` |"),
        vec!["a".to_string(), "b \\| c".to_string(), "`d`".to_string()]
    );
}

fn case(name: &str, outcome: Outcome) -> CaseResult {
    CaseResult {
        name: name.to_string(),
        outcome,
        duration: Duration::from_millis(20),
        output: String::new(),
    }
}

fn spec(component: &str, name: &str, cases: Vec<CaseResult>) -> ComponentSpec {
    ComponentSpec {
        component: component.to_string(),
        result: SpecResult {
            name: name.to_string(),
            display: format!("{component}/test/proofs/{name}.spec.js"),
            cases,
            setup_failure: None,
            cleanup_failure: None,
            duration: Duration::from_millis(60),
        },
    }
}

fn failure(message: &str) -> Failure {
    Failure {
        kind: "assertion".to_string(),
        block: "then".to_string(),
        message: message.to_string(),
        location: None,
        stack: String::new(),
        details: Vec::new(),
        attachments: Vec::new(),
    }
}

/// Two components, one inheriting from the other, with a matrix each: the results join the rows, an inherited row
/// takes the other component's proofs, and a passing feature filed under no requirement fails the run.
#[test]
fn the_summary_joins_the_results_to_the_matrix() {
    let dir = std::env::temp_dir().join(format!("moqui-proofs-summary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let framework = dir.join("framework");
    let app = dir.join("app");
    std::fs::create_dir_all(framework.join("test/proofs")).unwrap();
    std::fs::create_dir_all(framework.join("docs")).unwrap();
    std::fs::create_dir_all(app.join("test/proofs")).unwrap();
    std::fs::write(
        framework.join("test/proofs/suite.js"),
        "suite(['V3Web'], { server: { components: ['framework'] }, matrix: 'docs' })\n",
    )
    .unwrap();
    std::fs::write(
        framework.join("docs/V3.md"),
        "| Requirement | L | Topic | Status | Evidence | Proof |\n| --- | --- | --- | --- | --- | --- |\n\
         | v5.0.0-3.4.1 | 1 | Strict transport | Framework | x | y |\n\
         | v5.0.0-3.4.2 | 1 | Allowed origins | Configuration | x | - |\n\
         | v5.0.0-3.3.1 | 1 | Cookie attributes | Framework | x | y |\n",
    )
    .unwrap();
    std::fs::write(
        app.join("test/proofs/suite.js"),
        "suite(['Shop'], { matrix: ['Matrix.md', 'Gone.md'] })\n",
    )
    .unwrap();
    std::fs::write(
        app.join("Matrix.md"),
        "| Requirement | L | Topic | Status | Evidence | Proof |\n| --- | --- | --- | --- | --- | --- |\n\
         | v5.0.0-3.4.1 | 1 | Strict transport | Inherited | framework | - |\n\
         | v5.0.0-2.1.1 | 1 | Business rules documented | PASS | x | - |\n",
    )
    .unwrap();
    let components = vec![
        TestComponent {
            name: "framework".to_string(),
            dir: framework.clone(),
            specs: Vec::new(),
        },
        TestComponent {
            name: "app".to_string(),
            dir: app.clone(),
            specs: Vec::new(),
        },
    ];
    let results = vec![
        spec(
            "framework",
            "V3Web",
            vec![
                case("v5.0.0-3.4.1: every answer has HSTS", Outcome::Passed),
                case(
                    "v5.0.0-3.4.1: and on an error page",
                    Outcome::Skipped("needs a fixture".to_string()),
                ),
                case(
                    "v5.0.0-3.3.1: the session cookie is HttpOnly",
                    Outcome::Failed(failure("no HttpOnly")),
                ),
                case("v5.0.0-3.9.9: a mistyped id", Outcome::Passed),
                case("a feature with no id", Outcome::Passed),
            ],
        ),
        spec(
            "app",
            "Shop",
            vec![case(
                "v5.0.0-2.1.1: the rules are written down",
                Outcome::Skipped("deployment".to_string()),
            )],
        ),
    ];
    let out = dir.join("out");
    let run = Run {
        server: "https://test.example.com",
        profile: "deployment",
    };
    let written = write_summary(&out, &components, &results, &run).unwrap();
    assert_eq!(written.unfiled, 1);
    let text = written.lines.join("\n");
    assert!(
        text.contains(
            "Proofs of framework: 3 requirements with proofs: 1 passed, 1 partial, 1 failed, 0 skipped; 3 requirements in its matrix"
        ),
        "{text}"
    );
    assert!(text.contains("v5.0.0-3.9.9 is not in the component's matrix"), "{text}");
    assert!(
        text.contains("the matrix Gone.md its suite.js names is not there"),
        "{text}"
    );
    assert!(text.contains("  framework/V3Web > a feature with no id"), "{text}");

    let summary: Value = serde_json::from_str(&std::fs::read_to_string(out.join("proofs.json")).unwrap()).unwrap();
    assert_eq!(summary["profile"], "deployment");
    assert_eq!(summary["server"], "https://test.example.com");
    let framework_rows = &summary["components"][0]["requirements"];
    let ids: Vec<&str> = framework_rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["v5.0.0-3.3.1", "v5.0.0-3.4.1", "v5.0.0-3.4.2", "v5.0.0-3.9.9"]
    );
    assert_eq!(framework_rows[0]["result"], "failed");
    assert_eq!(framework_rows[0]["proofs"][0]["reason"], "no HttpOnly");
    assert_eq!(framework_rows[1]["result"], "partial");
    assert_eq!(framework_rows[1]["level"], 1);
    assert_eq!(framework_rows[2]["result"], "none");
    assert_eq!(framework_rows[3]["status"], Value::Null);
    let app_rows = &summary["components"][1]["requirements"];
    assert_eq!(app_rows[0]["id"], "v5.0.0-2.1.1");
    assert_eq!(app_rows[0]["result"], "skipped");
    // the inherited row takes the framework's proofs of it
    assert_eq!(app_rows[1]["id"], "v5.0.0-3.4.1");
    assert_eq!(app_rows[1]["result"], "partial");
    assert_eq!(app_rows[1]["via"], json!(["framework"]));
    assert_eq!(summary["unfiled"][0]["feature"], "a feature with no id");

    let markdown = std::fs::read_to_string(out.join("proofs.md")).unwrap();
    assert!(markdown.starts_with("# Security proofs\n"), "{markdown}");
    assert!(
        markdown.contains("| v5.0.0-3.4.1 | 1 | Strict transport | Inherited | partial | in framework: V3Web › every answer has HSTS: passed<br>V3Web › and on an error page: skipped (needs a fixture) |"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| v5.0.0-3.9.9 |  |  | not in the matrix | passed |"),
        "{markdown}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
