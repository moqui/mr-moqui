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

//! Static checks of the Android app's settings that this crate's traffic and the app's WebView rely on, the proofs of
//! OWASP MASVS v2.1.0 controls of a Configuration status (docs/security/proofs/clients/README.md). The app
//! (`crates/moqui-mobile`) is outside the workspace, so its files are read here by path from the repository root, and
//! `cargo test --workspace` (`./moq test-framework`) checks them on every push: this crate is the one workspace member
//! the app carries, and on Android its requests are checked by the platform's trust manager, whose trust anchors these
//! settings decide.

use std::path::{Path, PathBuf};

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The app's Gradle project.
fn android() -> PathBuf {
    root().join("crates/moqui-mobile/gen/android")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The source sets of the app module (`app/src/<name>`), with their names, but for the unit and device tests'.
fn source_sets() -> Vec<(String, PathBuf)> {
    let mut sets: Vec<(String, PathBuf)> = std::fs::read_dir(android().join("app/src"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), path))
        .filter(|(name, _)| name != "test" && name != "androidTest")
        .collect();
    sets.sort();
    assert!(sets.iter().any(|(name, _)| name == "main"), "{sets:?}");
    sets
}

/// The files under `dir` (recursively) whose name `wanted` takes, but in a `build` directory.
fn files_under(dir: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "build" {
                found.extend(files_under(&path, wanted));
            }
        } else if wanted(&name) {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// `text` with its comments blanked (`code`), and with its string and character literals' contents blanked too
/// (`bare`), each byte for a byte so offsets agree, for Kotlin and Rust: `//`, `/* */`, `"..."`, `"""..."""`,
/// `r#"..."#` and `'x'` (a Rust lifetime is left as code).
struct Lexed {
    code: String,
    bare: String,
}

fn lex(text: &str) -> Lexed {
    let bytes = text.as_bytes();
    let mut code = bytes.to_vec();
    let mut bare = bytes.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let find = |from: usize, end: &[u8]| {
        bytes[from..]
            .windows(end.len())
            .position(|window| window == end)
            .map_or(bytes.len(), |at| from + at)
    };
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"//") {
            let end = find(at, b"\n");
            blank(&mut code, at, end);
            blank(&mut bare, at, end);
            at = end;
        } else if rest.starts_with(b"/*") {
            let end = (find(at + 2, b"*/") + 2).min(bytes.len());
            blank(&mut code, at, end);
            blank(&mut bare, at, end);
            at = end;
        } else if rest.starts_with(b"\"\"\"") {
            let end = find(at + 3, b"\"\"\"");
            blank(&mut bare, at + 3, end);
            at = (end + 3).min(bytes.len());
        } else if rest[0] == b'r' && (rest.starts_with(b"r\"") || rest.starts_with(b"r#")) {
            let hashes = rest[1..].iter().take_while(|byte| **byte == b'#').count();
            if rest.get(1 + hashes) == Some(&b'"') {
                let mut close = vec![b'"'];
                close.resize(1 + hashes, b'#');
                let start = at + 2 + hashes;
                let end = find(start, &close);
                blank(&mut bare, start, end);
                at = (end + close.len()).min(bytes.len());
            } else {
                at += 1;
            }
        } else if rest[0] == b'"' {
            let mut end = at + 1;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            blank(&mut bare, at + 1, end);
            at = end + 1;
        } else if rest[0] == b'\'' {
            // 'x' or '\x...': a character literal; else a Rust lifetime
            let end = if rest.get(1) == Some(&b'\\') {
                bytes[at + 2..]
                    .iter()
                    .position(|byte| *byte == b'\'')
                    .map(|to| at + 2 + to)
            } else {
                let width = text[at + 1..].chars().next().map_or(1, char::len_utf8);
                (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 1 + width)
            };
            match end {
                Some(end) => {
                    blank(&mut bare, at + 1, end);
                    at = end + 1;
                }
                None => at += 1,
            }
        } else {
            at += 1;
        }
    }
    Lexed {
        code: String::from_utf8(code).unwrap(),
        bare: String::from_utf8(bare).unwrap(),
    }
}

/// The name of the innermost block (`name {`) around `offset` in `lexed`, as written (`getByName("debug")`), or empty at
/// the top level.
fn block_of(lexed: &Lexed, offset: usize) -> String {
    let mut depth = 0;
    for (at, byte) in lexed.bare.as_bytes()[..offset].iter().enumerate().rev() {
        match byte {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                let line = lexed.code[..at]
                    .rfind(['\n', '{', '}', ';'])
                    .map_or(0, |start| start + 1);
                return lexed.code[line..at].trim().to_string();
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    String::new()
}

/// `text` with its XML comments blanked.
fn without_xml_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("<!--") {
        out.push_str(before);
        rest = after.split_once("-->").map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}

/// The values of the attribute `name` (`android:usesCleartextTraffic`) wherever it is set in `xml`.
fn attribute_values(xml: &str, name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(name) {
        rest = &rest[at + name.len()..];
        let after = rest.trim_start();
        let Some(after) = after.strip_prefix('=') else { continue };
        let after = after.trim_start();
        let Some(quote) = after.chars().next().filter(|quote| *quote == '"' || *quote == '\'') else {
            continue;
        };
        let value = after[1..].split(quote).next().unwrap_or_default();
        values.push(value.to_string());
    }
    values
}

/// The security proof of OWASP MASVS v2.1.0 MASVS-NETWORK-1 (docs/security/proofs/clients/README.md), static, of the
/// mobile app's Configuration row: no build but the `debug` build type lets the WebView's own loads go in clear text.
/// The application's `android:usesCleartextTraffic` is the manifest placeholder (or `false`), which the Gradle build
/// sets `false` in `defaultConfig` and `true` only in the `debug` build type; no other build type, flavor or source
/// set's manifest that a release build merges (`fcm`, the git-ignored `tauri.build.gradle.kts` when present) turns it
/// on. The client's own requests and sockets refuse clear text by `base_url` (`plain_http_is_for_this_machine_only`).
#[test]
fn only_the_debug_build_lets_the_android_apps_webview_load_clear_text() {
    let manifest = without_xml_comments(&read(&android().join("app/src/main/AndroidManifest.xml")));
    let application = manifest
        .split_once("<application")
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(tag, _)| tag)
        .expect("the main manifest's application element");
    let set = attribute_values(application, "android:usesCleartextTraffic");
    assert!(
        set == ["${usesCleartextTraffic}"] || set == ["false"],
        "the application's usesCleartextTraffic: {set:?}"
    );
    assert_eq!(
        attribute_values(&manifest, "android:usesCleartextTraffic").len(),
        1,
        "set once, on the application"
    );
    for (name, dir) in source_sets() {
        let path = dir.join("AndroidManifest.xml");
        if name == "main" || name == "debug" || !path.exists() {
            continue;
        }
        let values = attribute_values(&without_xml_comments(&read(&path)), "android:usesCleartextTraffic");
        assert!(
            values.iter().all(|value| value == "false"),
            "{}: {values:?}",
            path.display()
        );
    }

    let gradle_path = android().join("app/build.gradle.kts");
    let gradle = lex(&read(&gradle_path));
    let placeholder = "manifestPlaceholders[\"usesCleartextTraffic\"]";
    let mut by_block = Vec::new();
    for (at, _) in gradle.code.match_indices("usesCleartextTraffic") {
        let start = at.saturating_sub("manifestPlaceholders[\"".len());
        assert!(
            gradle.code[start..].starts_with(placeholder),
            "usesCleartextTraffic set some other way at byte {at} of {}",
            gradle_path.display()
        );
        let value = gradle.code[start + placeholder.len()..]
            .trim_start()
            .strip_prefix('=')
            .and_then(|rest| rest.trim_start().strip_prefix('"'))
            .and_then(|rest| rest.split_once('"'))
            .map(|(value, _)| value.to_string())
            .unwrap_or_else(|| panic!("the placeholder at byte {at} is not set to a string"));
        by_block.push((block_of(&gradle, at), value));
    }
    assert!(
        by_block.contains(&("defaultConfig".to_string(), "false".to_string())),
        "defaultConfig sets it false: {by_block:?}"
    );
    for (block, value) in &by_block {
        assert!(
            value == "false" || block == "getByName(\"debug\")",
            "{block} sets usesCleartextTraffic to {value}"
        );
    }
    let generated = android().join("app/tauri.build.gradle.kts");
    if generated.exists() {
        assert!(!lex(&read(&generated)).code.contains("usesCleartextTraffic"));
    }
}

/// The security proof of OWASP MASVS v2.1.0 MASVS-NETWORK-1 (docs/security/proofs/clients/README.md), static, of what
/// the mobile app's Configuration row takes from the platform: the WebView and the trust manager rustls-platform-verifier
/// asks (this crate's requests and sockets on Android) trust the system's authorities only. That is Android's default
/// configuration for an app that targets API 24 or later, so `targetSdk` is at least 24, and no network security
/// configuration of a source set a release build merges trusts the user's authorities (`<certificates src="user">`) or
/// permits clear text; a manifest's `android:networkSecurityConfig` names one of them. Today there is none.
#[test]
fn the_android_app_trusts_no_user_authority_and_permits_no_clear_text_by_its_network_configuration() {
    let gradle = lex(&read(&android().join("app/build.gradle.kts")));
    let target = gradle
        .code
        .match_indices("targetSdk")
        .filter_map(|(at, _)| {
            let value = gradle.code[at + "targetSdk".len()..].trim_start().strip_prefix('=')?;
            Some((
                at,
                value
                    .trim_start()
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<u32>()
                    .ok()?,
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(target.len(), 1, "one targetSdk: {target:?}");
    assert_eq!(block_of(&gradle, target[0].0), "defaultConfig");
    assert!(
        target[0].1 >= 24,
        "targetSdk {} trusts the user's authorities by default",
        target[0].1
    );

    let released: Vec<PathBuf> = source_sets()
        .into_iter()
        .filter(|(name, _)| name != "debug")
        .map(|(_, dir)| dir)
        .collect();
    let mut configs = Vec::new();
    for dir in &released {
        for path in files_under(&dir.join("res"), &|file| file.ends_with(".xml")) {
            let xml = without_xml_comments(&read(&path));
            if !xml.contains("<network-security-config") {
                continue;
            }
            for (attribute, refused) in [("src", "user"), ("cleartextTrafficPermitted", "true")] {
                assert!(
                    !attribute_values(&xml, attribute).iter().any(|value| value == refused),
                    "{}: {attribute}=\"{refused}\"",
                    path.display()
                );
            }
            configs.push(format!("@xml/{}", path.file_stem().unwrap().to_string_lossy()));
        }
    }
    for dir in &released {
        let manifest = dir.join("AndroidManifest.xml");
        if !manifest.exists() {
            continue;
        }
        for named in attribute_values(&without_xml_comments(&read(&manifest)), "android:networkSecurityConfig") {
            assert!(
                configs.contains(&named),
                "{} names {named}, not one checked here",
                manifest.display()
            );
        }
    }
    // the matrix's evidence is that there is none: a new one is read, and the row with it
    assert!(configs.is_empty(), "a network security configuration: {configs:?}");
}

/// The security proof of OWASP MASVS v2.1.0 MASVS-NETWORK-1 (docs/security/proofs/clients/README.md), static: no code
/// the mobile app ships turns a certificate or name check off. In the Kotlin (the app's and the patched wry's) no
/// WebView `onReceivedSslError` handler (so the WebView's default, cancel, holds), no `SslErrorHandler` and no trust
/// manager or host name verifier of its own; in the Rust of the client crates and the app (`moqui-client`,
/// `moqui-client-tauri`, `moqui-mobile`; tests left out) no reqwest `danger_*` option and no rustls `dangerous()`
/// verifier, so every request and socket keeps rustls-platform-verifier's check.
#[test]
fn no_code_of_the_mobile_app_turns_a_certificate_or_name_check_off() {
    let mobile = root().join("crates/moqui-mobile");
    let mut kotlin = files_under(&mobile.join("patches/wry/src/android/kotlin"), &|name| {
        name.ends_with(".kt")
    });
    for (_, dir) in source_sets() {
        kotlin.extend(files_under(&dir, &|name| {
            name.ends_with(".kt") || name.ends_with(".java")
        }));
    }
    assert!(kotlin.len() > 20, "the Kotlin was found: {kotlin:?}");
    for path in &kotlin {
        let code = lex(&read(path)).code;
        for refused in [
            "onReceivedSslError",
            "SslErrorHandler",
            "X509TrustManager",
            "HostnameVerifier",
            "TrustAllCerts",
        ] {
            assert!(!code.contains(refused), "{} has {refused}", path.display());
        }
    }

    let mut rust = Vec::new();
    for dir in [
        "crates/moqui-client/src",
        "crates/moqui-client-tauri/src",
        "crates/moqui-mobile/src",
    ] {
        rust.extend(files_under(&root().join(dir), &|name| {
            name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs")
        }));
    }
    assert!(rust.len() > 30, "the Rust was found: {rust:?}");
    for path in &rust {
        let within = path.strip_prefix(root()).unwrap_or(path);
        if within.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let code = lex(&read(path)).code;
        for refused in [
            "danger_accept_invalid",
            "tls_danger",
            ".dangerous()",
            "danger::",
            "NoVerifier",
        ] {
            assert!(!code.contains(refused), "{} has {refused}", path.display());
        }
    }
}
