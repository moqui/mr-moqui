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

//! The server's OpenUI Lang (plans/LLM-A2A-Assist.md 7.4, D6): lang-core in QuickJS parses, the checks name the skill
//! section of each problem and quote its rule, a patch merges once, a Mutation evaluates from the stored program, and
//! every rule and signature is in the skill's text.

use serde_json::json;

use super::*;

const CHART: &str = r#"root = Stack([CardHeader("Hits"), chart, table])
$type = "AT_SERVICE"
hits = Query("request", {path: "/apps/monitor/Artifacts", query: {artifactType: $type}}, {rows: []})
typeSelect = Select("type", $type, [SelectItem("AT_SERVICE", "Services")])
chart = BarChart(hits.rows.artifactName, [Series("Hits", hits.rows.hitCount)])
table = Table([Col("Service", hits.rows.artifactName), Col("Hits", hits.rows.hitCount)])"#;

fn problems_of(program: &str) -> Vec<Problem> {
    let sources = test_sources();
    let parsed = parse(&sources, program).unwrap();
    check(&parsed, None, None, &NoPaths)
}

fn sections(problems: &[Problem]) -> Vec<&'static str> {
    problems.iter().map(|problem| problem.rule.section).collect()
}

/// The skill file's sections by slug.
fn skill_sections() -> std::collections::BTreeMap<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/framework/skill/openui-lang.md");
    let text = std::fs::read_to_string(path).unwrap();
    let mut out = std::collections::BTreeMap::new();
    let mut current: Option<String> = None;
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        }
        if !fenced {
            if let Some(title) = line.strip_prefix("## ") {
                current = Some(title.trim().to_lowercase().replace(' ', "-"));
                continue;
            }
        }
        if let Some(slug) = &current {
            out.entry(slug.clone())
                .or_insert_with(String::new)
                .push_str(&format!("{line}\n"));
        }
    }
    out
}

#[test]
fn a_chart_program_parses_and_checks_with_no_problems() {
    let sources = test_sources();
    let parsed = parse(&sources, CHART).unwrap();
    assert_eq!(parsed.root.as_deref(), Some("Stack"));
    assert_eq!(parsed.queries, ["hits"]);
    // typeSelect is not reached from root: lang-core reports it and the check names verify
    let problems = check(&parsed, None, None, &NoPaths);
    assert_eq!(sections(&problems), ["verify"], "{problems:?}");
    assert!(problems[0].message.contains("typeSelect"), "{problems:?}");
    let fixed = CHART.replace(
        "root = Stack([CardHeader(\"Hits\"), chart, table])",
        "root = Stack([CardHeader(\"Hits\"), typeSelect, chart, table])",
    );
    assert_eq!(problems_of(&fixed), Vec::new());
    let checked = prepare(&sources, None, Some(&format!("```\n{fixed}\n```")), None, &NoPaths)
        .unwrap()
        .unwrap();
    assert_eq!(
        checked.statements,
        ["root", "$type", "hits", "typeSelect", "chart", "table"]
    );
    assert!(!checked.program.contains("```"));
}

#[test]
fn a_query_not_closed_with_two_braces_names_the_query_section() {
    let program = "root = Stack([table])\n\
        data = Query(\"request\", {path: \"/apps/system/Visit/VisitList\", query: {initialRequest: $q}, {rows: []})\n\
        table = Table([Col(\"Visit\", data.rows.visitId)])";
    let problems = problems_of(program);
    assert_eq!(problems[0].rule, QUERY_SHAPE, "{problems:?}");
    assert_eq!(problems[0].line, 2);
    assert!(problems[0].parse);
    let error: serde_json::Value = serde_json::from_str(&error_json(&problems, program, false)).unwrap();
    assert_eq!(error["error"], json!("openui_parse"));
    assert_eq!(error["skill"], json!("openui-lang#query"));
    assert_eq!(error["rule"], json!(QUERY_SHAPE.text));
    assert_eq!(error["line"], json!(2));
    assert!(
        error["snippet"].as_str().unwrap().starts_with("2 | data = Query("),
        "{error}"
    );
    let instruction = error["instruction"].as_str().unwrap();
    assert!(
        instruction.contains("section query")
            && instruction.contains("skill name \"openui-lang#query\"")
            && instruction.contains("Fix line 2"),
        "{instruction}"
    );
}

#[test]
fn each_class_of_error_names_its_section() {
    let cases: &[(&str, &str)] = &[
        ("root = Stack([a], direction: \"row\")\na = TextContent(\"x\")", "syntax"),
        ("root = Stack([t])\n$orders = Query(\"request\", {path: \"/apps/x\"}, {rows: []})\nt = Table([Col(\"A\", orders.rows.a)])", "syntax"),
        ("root = Stack([Stat(\"Total\", \"@Sum(rows.total)\")])", "syntax"),
        ("root = Stack([TextContent('x')])", "syntax"),
        ("root = Stack([Chartt([1])])", "components"),
        ("root = Stack([Card(Table([]))])", "components"),
        ("root = Stack([t])\nt = TextContent(n)\nn = @Map(rows, \"r\", r.x)", "functions"),
        ("root = Stack([TextContent(Number(\"3\"))])", "functions"),
        ("root = Stack([t])\nq = Query(\"request\", {method: \"POST\", path: \"/apps/x\"}, {rows: []})\nt = Table([Col(\"A\", q.rows.a)])", "query"),
        ("root = Stack([t])\nq = Query(\"request\", {path: \"/qapps/x\"}, {rows: []})\nt = Table([Col(\"A\", q.rows.a)])", "query"),
        ("root = Stack([missing])", "verify"),
        ("root = Stack([t])\nt = TextContent(\"x\")\nlost = TextContent(\"y\")", "verify"),
        ("root = Stack([Link(\"Out\", \"https://example.com/\")])", "links"),
    ];
    for (program, section) in cases {
        let problems = problems_of(program);
        assert!(!problems.is_empty(), "no problem for {program}");
        assert!(
            problems.iter().any(|problem| problem.rule.section == *section),
            "{program}: expected {section}, got {:?}",
            problems
                .iter()
                .map(|problem| (problem.rule.section, problem.message.as_str()))
                .collect::<Vec<_>>()
        );
        // every error names a section and quotes a rule of it
        let error: serde_json::Value = serde_json::from_str(&error_json(&problems, program, false)).unwrap();
        assert!(error["skill"].as_str().unwrap().starts_with("openui-lang#"), "{error}");
        assert!(
            RULES.iter().any(|rule| rule.text == error["rule"].as_str().unwrap()),
            "{error}"
        );
    }
}

#[test]
fn a_select_with_its_arguments_out_of_order_is_the_forms_section() {
    let problems = problems_of(
        "root = Stack([f])\nf = Select(\"orgId\", [SelectItem(\"A\", \"Alpha\")], null, {required: true}, $orgId)",
    );
    assert!(
        problems.iter().any(|problem| problem.rule == FORM_VALUE),
        "{problems:?}"
    );
    let found = problems.iter().find(|problem| problem.rule == FORM_VALUE).unwrap();
    assert!(
        found.message.contains("Select(name, value?, items, label?)"),
        "{found:?}"
    );
}

#[test]
fn unknown_names_get_the_nearest_ones() {
    let problems = problems_of("root = Stack([BarChrt([], [])])");
    assert_eq!(problems[0].rule, COMPONENTS);
    assert!(
        problems[0].did_you_mean.contains(&"BarChart".to_string()),
        "{problems:?}"
    );
    let functions = problems_of("root = Stack([TextContent(@Coutn(rows))])");
    let invented = functions.iter().find(|problem| problem.rule == FUNCTIONS).unwrap();
    assert!(invented.did_you_mean.contains(&"@Count".to_string()), "{functions:?}");
}

#[test]
fn a_visible_filter_no_query_reads_is_not_wired() {
    let program = "root = Stack([FormControl(\"Type\", Select(\"type\", $type, [SelectItem(\"A\", \"A\")])), t])\n\
        hits = Query(\"request\", {path: \"/apps/monitor/Artifacts\", query: {artifactType: \"AT_SERVICE\"}}, {rows: []})\n\
        t = Table([Col(\"Name\", hits.rows.artifactName)])";
    let problems = problems_of(program);
    assert_eq!(sections(&problems), ["verify"], "{problems:?}");
    assert_eq!(problems[0].rule, WIRED);
    let wired = program.replace("artifactType: \"AT_SERVICE\"", "artifactType: $type");
    assert_eq!(problems_of(&wired), Vec::new());
    // a control in a Form posts with the form: not a filter
    let form = "root = Stack([f, t])\nf = Form(\"ask\", Button(\"Go\"), [Input(\"note\", $note)])\n\
        hits = Query(\"request\", {path: \"/apps/monitor/Artifacts\"}, {rows: []})\n\
        t = Table([Col(\"Name\", hits.rows.artifactName)])";
    assert_eq!(problems_of(form), Vec::new());
}

#[test]
fn a_mutation_body_that_reads_a_query_is_refused() {
    let program = "root = Stack([t, b])\n\
        rows = Query(\"request\", {path: \"/apps/x\"}, {rows: []})\n\
        save = Mutation(\"request\", {method: \"POST\", path: \"/apps/x/save\", body: {id: rows.rows}})\n\
        t = Table([Col(\"A\", rows.rows.a)])\n\
        b = Button(\"Save\", Action([@Run(save)]))";
    let problems = problems_of(program);
    assert!(
        problems.iter().any(|problem| problem.rule == MUTATION_BODY),
        "{problems:?}"
    );
}

#[test]
fn a_patch_merges_once_and_a_rename_is_the_edit_section() {
    let sources = test_sources();
    let base = CHART.replace(
        "root = Stack([CardHeader(\"Hits\"), chart, table])",
        "root = Stack([CardHeader(\"Hits\"), typeSelect, chart, table])",
    );
    let patched = prepare(
        &sources,
        Some(&base),
        None,
        Some("chart = LineChart(hits.rows.artifactName, [Series(\"Hits\", hits.rows.hitCount)])\ntable = null\nroot = Stack([CardHeader(\"Hits\"), typeSelect, chart])"),
        &NoPaths,
    )
    .unwrap()
    .unwrap();
    assert!(patched.program.contains("chart = LineChart("), "{}", patched.program);
    assert!(!patched.program.contains("table ="), "{}", patched.program);
    assert!(patched
        .program
        .starts_with("root = Stack([CardHeader(\"Hits\"), typeSelect, chart])"));
    // a new name for the chart while the old one is dropped
    let renamed = prepare(
        &sources,
        Some(&base),
        None,
        Some("lineChart = LineChart(hits.rows.artifactName, [Series(\"Hits\", hits.rows.hitCount)])\nchart = null\nroot = Stack([CardHeader(\"Hits\"), typeSelect, lineChart, table])"),
        &NoPaths,
    )
    .unwrap();
    // lineChart is a LineChart and chart a BarChart: not the same component, so no rename; a BarChart under a new name is
    let renamed_bar = prepare(
        &sources,
        Some(&base),
        None,
        Some("hitChart = BarChart(hits.rows.artifactName, [Series(\"Hits\", hits.rows.hitCount)])\nroot = Stack([CardHeader(\"Hits\"), typeSelect, hitChart, table])"),
        &NoPaths,
    )
    .unwrap();
    assert!(renamed.is_ok(), "{renamed:?}");
    let problems = renamed_bar.unwrap_err();
    assert_eq!(problems[0].rule, RENAME, "{problems:?}");
    // a patch that does not parse alone is refused before the merge
    let broken = prepare(
        &sources,
        Some(&base),
        None,
        Some("chart = BarChart(hits.rows.x, [Series(\"H\", hits.rows.y)]"),
        &NoPaths,
    )
    .unwrap()
    .unwrap_err();
    assert!(broken[0].parse, "{broken:?}");
}

#[test]
fn a_mutation_is_evaluated_from_the_program_with_the_posted_state() {
    let sources = test_sources();
    let program = "root = Stack([b])\n$description = \"\"\n\
        save = Mutation(\"request\", {method: \"POST\", path: \"/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup\", body: {userGroupId: \"ADMIN\", description: $description}})\n\
        b = Button(\"Save\", Action([@Run(save)]))";
    let (tool, args) = evaluate_mutation(&sources, program, "save", &json!({"description": "Renamed"}))
        .unwrap()
        .unwrap();
    assert_eq!(tool, "request");
    assert_eq!(args["body"], json!({"userGroupId": "ADMIN", "description": "Renamed"}));
    assert_eq!(
        args["path"],
        json!("/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup")
    );
    assert_eq!(evaluate_mutation(&sources, program, "other", &json!({})).unwrap(), None);
}

#[test]
fn every_rule_is_quoted_in_its_section_and_every_signature_is_in_the_skill() {
    let sections = skill_sections();
    for rule in RULES {
        let text = sections
            .get(rule.section)
            .unwrap_or_else(|| panic!("the skill has no section {}", rule.section));
        assert!(
            text.contains(rule.text),
            "section {} does not quote: {}",
            rule.section,
            rule.text
        );
    }
    let components = parse(&test_sources(), "root = Stack([])").unwrap().components;
    assert_eq!(components.len(), 46);
    let where_signatures: String = ["components", "forms", "links"]
        .iter()
        .map(|slug| sections[*slug].clone())
        .collect();
    for name in components.keys() {
        let signature = signature(&components, name).unwrap();
        assert!(
            where_signatures.contains(&signature),
            "the skill does not give {signature}"
        );
    }
    // the sections a typical load reads stay small (plan 7.5)
    for (slug, text) in &sections {
        assert!(
            text.chars().count() < 5_000,
            "section {slug} is {} chars",
            text.chars().count()
        );
    }
}

#[test]
fn the_bundle_is_the_patched_one() {
    let sources = test_sources();
    for banned in [
        "posthog",
        "cloudfront",
        "ci-info",
        "child_process",
        "execfile",
        "getbuiltinmodule",
        "sendbeacon",
        "xmlhttprequest",
        "fetch(",
    ] {
        assert!(!sources.bundle.to_lowercase().contains(banned), "{banned}");
    }
    // the failed-query patch is in: a failed fetch is remembered
    assert!(
        sources.bundle.contains(".failed"),
        "the failed-query patch is not in the bundle"
    );
}

#[test]
fn a_query_takes_only_its_own_options_and_no_reserved_name() {
    let program = "root = Stack([t])\n\
        q = Query(\"request\", {path: \"/apps/x\", \"__mutation\": \"del\"}, {rows: []}, 5)\n\
        t = Table([Col(\"A\", q.rows.a)])";
    let problems = problems_of(program);
    assert!(
        problems
            .iter()
            .any(|problem| problem.rule == QUERY_SHAPE && problem.message.contains("__mutation")),
        "{problems:?}"
    );
    let nested = "root = Stack([t])\nq = Query(\"request\", {path: \"/apps/x\", query: {__mutation: \"del\"}}, {rows: []})\nt = Table([Col(\"A\", q.rows.a)])";
    assert!(
        problems_of(nested)
            .iter()
            .any(|problem| problem.message.contains("reserved")),
        "{:?}",
        problems_of(nested)
    );
    let extra = "root = Stack([t])\nq = Query(\"request\", {path: \"/apps/x\", body: {a: 1}}, {rows: []})\nt = Table([Col(\"A\", q.rows.a)])";
    assert!(problems_of(extra)
        .iter()
        .any(|problem| problem.message.contains("body is not an option of a Query")));
}
