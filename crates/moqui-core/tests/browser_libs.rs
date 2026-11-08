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

//! The vendored browser libraries under `webroot/libs`: the manifest matches the files, and the Vue
//! compiler bundle runs in the script engine, quickjs-oxide (Spike 4 of `plans/archive/QvtServerStatic.md`).

use std::path::PathBuf;
use std::time::Instant;

use moqui_core::script::standalone::Standalone;
use sha2::{Digest, Sha256};

fn libs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen/webroot/libs")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[test]
fn every_file_in_the_manifest_exists_with_its_recorded_sha256() {
    let dir = libs_dir();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json")).unwrap();
    let mut listed: Vec<String> = vec!["MANIFEST.json".into(), "refresh.mjs".into()];
    let mut check = |path: &str, sha: &str, size: Option<u64>| {
        let bytes = std::fs::read(dir.join(path)).unwrap_or_else(|err| panic!("{path}: {err}"));
        assert_eq!(hex(&Sha256::digest(&bytes)), sha, "sha256 of {path}");
        if let Some(size) = size {
            assert_eq!(bytes.len() as u64, size, "size of {path}");
        }
        listed.push(path.to_string());
    };
    let notices = &manifest["notices"];
    check(
        notices["path"].as_str().unwrap(),
        notices["sha256"].as_str().unwrap(),
        None,
    );
    let libraries = manifest["libraries"].as_array().unwrap();
    assert!(libraries.len() >= 14);
    for lib in libraries {
        let id = lib["id"].as_str().unwrap();
        assert!(!lib["license"].as_str().unwrap().is_empty(), "{id} has no license");
        if id != "license-texts" {
            assert!(lib["tarballIntegrity"].as_str().unwrap().starts_with("sha512-"), "{id}");
            assert!(lib["source"].as_str().unwrap().starts_with("https://"), "{id}");
        }
        for file in lib["files"].as_array().unwrap() {
            check(
                file["path"].as_str().unwrap(),
                file["sha256"].as_str().unwrap(),
                file["size"].as_u64(),
            );
        }
        // the reviewed patches a library was built with (libs/refresh.mjs records them)
        for patch in lib["patches"].as_array().into_iter().flatten() {
            check(
                patch["path"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{id}: a patch has no path")),
                patch["sha256"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{id}: a patch has no sha256")),
                None,
            );
        }
    }
    // Nothing else sits in the tree: no source maps, tests, or docs.
    let mut found = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    listed.sort();
    assert_eq!(
        found, listed,
        "files in libs/ that are not in MANIFEST.json, or the reverse"
    );
}

#[test]
fn the_manifest_licenses_are_the_reviewed_set() {
    let dir = libs_dir();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("MANIFEST.json")).unwrap()).unwrap();
    for lib in manifest["libraries"].as_array().unwrap() {
        let id = lib["id"].as_str().unwrap();
        let license = lib["license"].as_str().unwrap();
        let reviewed = match id {
            "fontawesome" => "OFL-1.1 AND MIT",
            "atkinson-next" | "atkinson-mono" => "OFL-1.1",
            "tom-select" | "material-icons" | "material-symbols" | "pdfjs" => "Apache-2.0",
            "dompurify" => "MPL-2.0 OR Apache-2.0",
            "highlightjs" => "BSD-3-Clause",
            "naturalearth" => "ISC",
            "qz-tray" => "LGPL-2.1-only",
            "vue-compiler-dom" => "MIT AND BSD-2-Clause",
            "matrix-sdk-crypto" => {
                "Apache-2.0 AND MIT AND BSD-3-Clause AND ISC AND Zlib AND Unicode-3.0 AND BSL-1.0 AND MPL-2.0"
            }
            "license-texts" => "n/a",
            _ => "MIT",
        };
        assert_eq!(
            license, reviewed,
            "{id}: license changed, review plans/archive/License-Review.md"
        );
    }
}

fn compiler_script() -> String {
    std::fs::read_to_string(libs_dir().join("vue/compiler-dom.cjs.js")).expect("compiler-dom.cjs.js")
}

/// Run `body` with the compiler bundle loaded in a runtime of its own.
fn with_compiler<R>(body: impl FnOnce(&Standalone) -> R) -> R {
    let js = Standalone::start(256 * 1024 * 1024, || false);
    let started = Instant::now();
    js.run(&compiler_script()).unwrap_or_else(|err| panic!("load: {err}"));
    eprintln!("compiler-dom bundle loaded in {:?}", started.elapsed());
    body(&js)
}

fn compile(js: &Standalone, template: &str) -> String {
    let source = format!(
        "VueCompilerDOM.compile({}, {{prefixIdentifiers: true, mode: 'function'}}).code",
        serde_json::to_string(template).unwrap()
    );
    js.text(&source).unwrap_or_else(|err| panic!("compile: {err}"))
}

#[test]
fn the_vue_compiler_bundle_compiles_with_prefix_identifiers() {
    with_compiler(|js| {
        let code = compile(js, r#"<div :a="b">{{c}}</div>"#);
        eprintln!("generated:\n{code}");
        assert!(code.contains("_ctx.b"), "{code}");
        assert!(code.contains("_toDisplayString(_ctx.c)"), "{code}");
        assert!(!code.contains("with ("), "prefixIdentifiers output has no with: {code}");
        assert!(!code.contains("new Function"));

        // A larger template: v-if, v-for, v-model, an event handler, a slot, a component.
        let code = compile(
            js,
            r#"<q-form @submit.prevent="save(row)"><q-input v-model="form.name" label="Name"/>
            <div v-if="rows.length"><q-item v-for="(row, i) in rows" :key="row.id" @click="pick(i)">{{ row.name | 0 }}</q-item></div>
            <template v-slot:after><q-btn label="Go"/></template></q-form>"#,
        );
        assert!(code.contains("_ctx.rows"), "{code}");
        assert!(code.contains("_resolveComponent(\"q-form\")"), "{code}");
        assert!(!code.contains("with ("));

        // The code is a body that takes Vue and returns the render function.
        let kind: String = js
            .text(&format!(
                "typeof (new Function('Vue', {}))({{}})",
                serde_json::to_string(&code).unwrap()
            ).replace("({})", "({toDisplayString(){},resolveComponent(){},openBlock(){},createBlock(){},createVNode(){},createElementBlock(){},createElementVNode(){},createCommentVNode(){},createTextVNode(){},withCtx(){},withModifiers(){},Fragment:1,renderList(){}})"))
            .unwrap_or_else(|err| panic!("evaluate: {err}"));
        assert_eq!(kind, "function");
    });
}

#[test]
fn a_bad_expression_is_reported_through_on_error() {
    with_compiler(|js| {
        let message: String = js
            .text(
                "var errors = []; VueCompilerDOM.compile('<div :a=\"b c\"></div>', {prefixIdentifiers: true, mode: 'function', onError: function (e) { errors.push(String(e.code) + ' ' + e.message); }}); errors.join('|')",
            )
            .unwrap_or_else(|err| panic!("compile: {err}"));
        assert!(!message.is_empty(), "an invalid expression must reach onError");
    });
}
