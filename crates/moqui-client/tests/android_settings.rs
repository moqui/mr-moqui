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
//! settings decide. The checks after the proofs hold the app's fixes of the security review's findings, each by its
//! register id (`SEC-CLI-*`, plans/Security-Review-Low.md).

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

/// The innermost block (`{ ... }`) of `lexed` around `offset`: the offsets of its `{` and its `}` (the text's end when it
/// is not closed); none at the top level.
fn block_around(lexed: &Lexed, offset: usize) -> Option<(usize, usize)> {
    let bare = lexed.bare.as_bytes();
    let mut depth = 0;
    let mut open = None;
    for at in (0..offset.min(bare.len())).rev() {
        match bare[at] {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                open = Some(at);
                break;
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    let open = open?;
    let mut depth = 0;
    for (at, byte) in bare.iter().enumerate().skip(open + 1) {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some((open, at)),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    Some((open, bare.len()))
}

/// The body, as code, of the first block whose header has `header` (`fun onCreate(`, `init {`).
fn body_of<'a>(lexed: &'a Lexed, header: &str) -> Option<&'a str> {
    let at = lexed.code.find(header)?;
    let open = at + lexed.bare[at..].find('{')?;
    let (_, close) = block_around(lexed, open + 1)?;
    Some(&lexed.code[open + 1..close])
}

/// The braces of the function (`fun name(...) {`) around `offset` in `lexed`: the first enclosing block whose header,
/// from the statement before it, has `fun `.
fn function_around(lexed: &Lexed, offset: usize) -> Option<(usize, usize)> {
    let mut at = offset;
    loop {
        let (open, close) = block_around(lexed, at)?;
        let start = lexed.bare[..open].rfind([';', '{', '}']).map_or(0, |start| start + 1);
        if lexed.code[start..open].contains("fun ") {
            return Some((open, close));
        }
        at = open;
    }
}

/// The start tags of the elements `name` (`meta-data`) in `xml`, without their `<name` and `>`.
fn start_tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}");
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        rest = &rest[at + open.len()..];
        // `<activity`, not `<activity-alias`
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            continue;
        }
        let end = rest.find('>').unwrap_or(rest.len());
        tags.push(&rest[..end]);
        rest = &rest[end..];
    }
    tags
}

/// `xml` less its elements `name`, each from its start tag to its end tag (or the empty element).
fn without_elements(xml: &str, name: &str) -> String {
    let (open, close) = (format!("<{name}"), format!("</{name}>"));
    let mut out = String::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            out.push_str(&rest[..at + open.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..at]);
        let tag = after.find('>').map_or(after.len(), |end| end + 1);
        rest = if after[..tag].ends_with("/>") {
            &after[tag..]
        } else {
            after.find(&close).map_or("", |end| &after[end + close.len()..])
        };
    }
    out.push_str(rest);
    out
}

/// The `android:name` and `android:value` of each `<meta-data>` in `xml`.
fn meta_data(xml: &str) -> Vec<(String, String)> {
    start_tags(xml, "meta-data")
        .into_iter()
        .map(|tag| {
            let first = |name: &str| attribute_values(tag, name).into_iter().next().unwrap_or_default();
            (first("android:name"), first("android:value"))
        })
        .collect()
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

/// What the app's code asks for by name of one of its classes at run time.
#[derive(Debug, PartialEq)]
enum Wanted {
    /// the class only
    Class,
    /// a method it names (through JNI, or Kotlin's reflection)
    Method(String),
    /// a native method Rust exports (`Java_org_moqui_mobile_<Class>_<method>`), found by the class's and its own name
    Native(String),
    /// a method whose name is not written beside the class's: every member
    Every,
}

/// A `-keep` rule of a ProGuard file: the class names it names (ProGuard's wildcards, a `!` one against), and the
/// members it keeps (none: the class only).
struct Keep {
    classes: Vec<String>,
    members: Vec<String>,
}

impl Keep {
    /// Whether the rule keeps `class` by its name: its first name that matches decides, a `!` one against.
    fn names(&self, class: &str) -> bool {
        self.classes
            .iter()
            .find_map(|pattern| {
                let (against, pattern) = match pattern.strip_prefix('!') {
                    Some(pattern) => (true, pattern),
                    None => (false, pattern.as_str()),
                };
                class_matches(pattern.as_bytes(), class.as_bytes()).then_some(!against)
            })
            .unwrap_or(false)
    }

    /// Whether the rule keeps `wanted` of a class it names.
    fn keeps(&self, wanted: &Wanted) -> bool {
        let every = |member: &str| member == "*" || member == "<methods>";
        let named = |member: &str, name: &str| {
            member
                .split_once('(')
                .is_some_and(|(head, _)| head.split_whitespace().last() == Some(name))
        };
        match wanted {
            Wanted::Class => true,
            Wanted::Every => self.members.iter().any(|member| every(member)),
            Wanted::Method(name) => self.members.iter().any(|member| every(member) || named(member, name)),
            Wanted::Native(name) => self
                .members
                .iter()
                .any(|member| every(member) || member == "native <methods>" || named(member, name)),
        }
    }
}

/// Whether ProGuard's class name `pattern` (`?` one character but a `.`, `*` any but a `.`, `**` any) matches `name`.
fn class_matches(pattern: &[u8], name: &[u8]) -> bool {
    match pattern {
        [] => name.is_empty(),
        [b'*', b'*', rest @ ..] => (0..=name.len()).any(|at| class_matches(rest, &name[at..])),
        [b'*', rest @ ..] => (0..=name.len())
            .take_while(|at| !name[..*at].contains(&b'.'))
            .any(|at| class_matches(rest, &name[at..])),
        [b'?', rest @ ..] => name.first().is_some_and(|c| *c != b'.') && class_matches(rest, &name[1..]),
        [c, rest @ ..] => name.first() == Some(c) && class_matches(rest, &name[1..]),
    }
}

/// The `-keep` rules of the ProGuard file `text` that keep classes by their names: not `-keepclassmembers` or
/// `-keepnames`, not one with an `allow...` modifier (which lets R8 remove or rename what it names), and not one for
/// the classes that extend or implement another (`-keep class * extends ...`), which names none.
fn keep_rules(text: &str) -> Vec<Keep> {
    let mut directives: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default();
        if line.trim_start().starts_with('-') {
            directives.push(String::new());
        }
        if let Some(last) = directives.last_mut() {
            last.push_str(line);
            last.push('\n');
        }
    }
    let mut rules = Vec::new();
    for directive in &directives {
        let Some(rest) = directive.trim_start().strip_prefix("-keep") else {
            continue;
        };
        if !rest.starts_with([' ', '\t', '\n', ',']) {
            continue;
        }
        let (head, body) = match rest.split_once('{') {
            Some((head, body)) => (head, body.split('}').next().unwrap_or_default()),
            None => (rest, ""),
        };
        let tokens: Vec<&str> = head
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|token| !token.is_empty())
            .collect();
        let Some(kind) = tokens
            .iter()
            .position(|token| matches!(*token, "class" | "interface" | "enum" | "@interface"))
        else {
            continue;
        };
        let names = &tokens[kind + 1..];
        if tokens[..kind].iter().any(|token| token.starts_with("allow"))
            || names.iter().any(|token| *token == "extends" || *token == "implements")
        {
            continue;
        }
        rules.push(Keep {
            classes: names.iter().map(|name| name.to_string()).collect(),
            members: body
                .split(';')
                .map(|member| member.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|member| !member.is_empty())
                .collect(),
        });
    }
    rules
}

/// What the Rust file `path` asks for by name of the app's classes (`org.moqui.mobile.*`): a string that names one
/// (`jni_str!("org.moqui.mobile.KeyVault")`, or `org/moqui/mobile/...`), with the methods named in the
/// `call_static_method`s on the name its statement binds it to (`let class = app_class(..)`) in its block; and the
/// native methods the file exports (`Java_org_moqui_mobile_Native_init`).
fn rust_by_name(path: &Path) -> Vec<(String, Wanted)> {
    let lexed = lex(&read(path));
    let (code, bare) = (&lexed.code, &lexed.bare);
    let mut wanted = Vec::new();
    for prefix in ["org.moqui.mobile.", "org/moqui/mobile/"] {
        for (at, _) in code.match_indices(prefix) {
            // in a string, whose contents bare blanks
            if bare.as_bytes()[at] == code.as_bytes()[at] {
                continue;
            }
            let name: String = code[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '/'))
                .collect();
            let name = name.trim_end_matches(['.', '/']);
            if !name
                .rsplit(['.', '/'])
                .next()
                .is_some_and(|last| last.starts_with(|c: char| c.is_ascii_uppercase()))
            {
                continue;
            }
            let class = format!("org.moqui.mobile.{}", name.replace('/', "."));
            let start = bare[..at].rfind([';', '{', '}']).map_or(0, |start| start + 1);
            let binding = code[start..at]
                .trim_start()
                .strip_prefix("let ")
                .and_then(|rest| rest.split_once('='))
                .map(|(name, _)| name.trim().trim_start_matches("mut ").trim().to_string());
            let mut methods = Vec::new();
            if let (Some(binding), Some((open, close))) = (binding, block_around(&lexed, at)) {
                let call = "call_static_method(";
                for (call_at, _) in code[open..close].match_indices(call) {
                    let args = &code[open + call_at + call.len()..];
                    let Some(rest) = args
                        .trim_start()
                        .strip_prefix('&')
                        .and_then(|rest| rest.strip_prefix(binding.as_str()))
                        .and_then(|rest| rest.trim_start().strip_prefix(','))
                    else {
                        continue;
                    };
                    let method = rest
                        .trim_start()
                        .strip_prefix("jni_str!(\"")
                        .and_then(|rest| rest.split_once('"'))
                        .map(|(method, _)| method.to_string());
                    methods.push(method.map_or(Wanted::Every, Wanted::Method));
                }
            }
            if methods.is_empty() {
                methods.push(Wanted::Class);
            }
            wanted.extend(methods.into_iter().map(|method| (class.clone(), method)));
        }
    }
    let export = "Java_org_moqui_mobile_";
    for (at, _) in code.match_indices(export) {
        if bare.as_bytes()[at] != code.as_bytes()[at] {
            continue;
        }
        let name: String = code[at + export.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if let Some((class, method)) = name.split_once('_') {
            wanted.push((format!("org.moqui.mobile.{class}"), Wanted::Native(method.to_string())));
        }
    }
    wanted
}

/// What the Kotlin file `path` asks for by name of the app's classes through reflection
/// (`Class.forName("org.moqui.mobile.FcmPush")`, `loadClass`): the method asked for on it by its name
/// (`.getMethod("start", ...)`), every member for one asked for by another name, or the class only.
fn kotlin_by_name(path: &Path) -> Vec<(String, Wanted)> {
    let code = lex(&read(path)).code;
    let mut wanted = Vec::new();
    for call in ["Class.forName(", ".loadClass("] {
        for (at, _) in code.match_indices(call) {
            let Some((name, rest)) = code[at + call.len()..]
                .trim_start()
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
            else {
                continue;
            };
            if !name.starts_with("org.moqui.mobile.") {
                continue;
            }
            let after = rest.split_once(')').map_or("", |(_, after)| after).trim_start();
            let asks = [".getMethod(", ".getDeclaredMethod("]
                .iter()
                .find_map(|asks| after.strip_prefix(asks));
            let method = asks
                .and_then(|args| args.trim_start().strip_prefix('"'))
                .and_then(|rest| rest.split_once('"'))
                .map(|(method, _)| method.to_string());
            let what = match (asks, method) {
                (_, Some(method)) => Wanted::Method(method),
                (Some(_), None) => Wanted::Every,
                (None, None) => Wanted::Class,
            };
            wanted.push((name.to_string(), what));
        }
    }
    wanted
}

/// SEC-CLI-127: R8 keeps what the app's code asks for by name at run time, which it cannot see is used. The release
/// build optimizes (`optimization { enable = true }`), so each class of the app (`org.moqui.mobile.*`) that the mobile
/// crate's Rust names in a string (the JNI calls of src/android.rs), whose native methods it exports
/// (`Java_org_moqui_mobile_Native_*`), or that its Kotlin asks for through reflection (`Push.startFcm`'s `FcmPush`)
/// has a `-keep` rule in `proguard-rules.pro` that names it (or a wildcard that covers it) and keeps the methods named
/// with it (`{ *; }` when they are not written there). `ScreenSecurity.set` and `CalendarIntent.insert` had none.
#[test]
fn every_class_and_method_the_mobile_apps_code_asks_for_by_name_has_a_keep_rule() {
    let mobile = root().join("crates/moqui-mobile");
    let mut wanted = Vec::new();
    let rust = files_under(&mobile.join("src"), &|name| {
        name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs")
    });
    for path in &rust {
        wanted.extend(
            rust_by_name(path)
                .into_iter()
                .map(|(class, what)| (class, what, path.clone())),
        );
    }
    let mut kotlin = files_under(&mobile.join("patches/wry/src/android/kotlin"), &|name| {
        name.ends_with(".kt")
    });
    for (_, dir) in source_sets() {
        kotlin.extend(files_under(&dir, &|name| {
            name.ends_with(".kt") || name.ends_with(".java")
        }));
    }
    for path in &kotlin {
        wanted.extend(
            kotlin_by_name(path)
                .into_iter()
                .map(|(class, what)| (class, what, path.clone())),
        );
    }
    // a scan that finds nothing does not pass
    let method = |name: &str| Wanted::Method(name.to_string());
    for (class, what) in [
        ("org.moqui.mobile.ScreenSecurity", method("set")),
        ("org.moqui.mobile.CalendarIntent", method("insert")),
        ("org.moqui.mobile.Push", method("startFcm")),
        ("org.moqui.mobile.FcmPush", method("start")),
        ("org.moqui.mobile.KeyVault", Wanted::Every),
        ("org.moqui.mobile.Native", Wanted::Native("init".to_string())),
    ] {
        assert!(
            wanted.iter().any(|(named, asked, _)| named == class && *asked == what),
            "{class} {what:?} was not found by the scan: {wanted:?}"
        );
    }
    let rules = keep_rules(&read(&android().join("app/proguard-rules.pro")));
    assert!(rules.len() > 10, "the rules were read");
    for (class, what, path) in &wanted {
        assert!(
            rules.iter().any(|rule| rule.names(class) && rule.keeps(what)),
            "{} asks for {class} {what:?} by name, which no -keep rule of proguard-rules.pro keeps",
            path.display()
        );
    }
}

/// SEC-CLI-104 (Gradle's wrapper): the wrapper checks the Gradle distribution it downloads against the SHA-256 it names
/// (`distributionSha256Sum`, 64 hex digits) and runs no other zip; the distribution is Gradle's own, over https. A new
/// `distributionUrl` takes the `.sha256` published beside its zip.
#[test]
fn the_android_apps_gradle_wrapper_checks_its_distribution_by_its_sha256() {
    let properties = read(&android().join("gradle/wrapper/gradle-wrapper.properties"));
    let values = |key: &str| -> Vec<String> {
        properties
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#') && !line.starts_with('!'))
            .filter_map(|line| line.split_once(['=', ':']))
            .filter(|(name, _)| name.trim() == key)
            .map(|(_, value)| value.trim().replace("\\:", ":"))
            .collect()
    };
    let sums = values("distributionSha256Sum");
    assert_eq!(sums.len(), 1, "one distributionSha256Sum: {sums:?}");
    assert!(
        sums[0].len() == 64
            && sums[0]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "distributionSha256Sum is not a SHA-256 in hex: {}",
        sums[0]
    );
    let urls = values("distributionUrl");
    assert_eq!(urls.len(), 1, "one distributionUrl: {urls:?}");
    assert!(
        urls[0].starts_with("https://services.gradle.org/distributions/gradle-") && urls[0].ends_with(".zip"),
        "{}",
        urls[0]
    );
}

/// SEC-CLI-115: a chat notification that carries a text's one-time code (the one with Copy code) is posted
/// `VISIBILITY_SECRET`, so a secure lock screen shows nothing of it, and the function that builds it sets no other
/// visibility and no public version: Android shows a `VISIBILITY_PRIVATE` notification's public version only when the
/// user set the lock screen to hide sensitive content, so under its default the code would show. On a device:
/// `ChatNotificationsTest.aOneTimeCodeIsSecretOnTheLockScreen`.
#[test]
fn a_chat_notification_with_a_one_time_code_is_secret_on_the_lock_screen() {
    let path = android().join("app/src/main/java/org/moqui/mobile/ChatNotifications.kt");
    let lexed = lex(&read(&path));
    let at = lexed
        .code
        .match_indices("copyAction(")
        .map(|(at, _)| at)
        .find(|at| !lexed.code[..*at].ends_with("fun "))
        .expect("Copy code's action");
    let (open, close) = block_around(&lexed, at).expect("the block that adds Copy code");
    assert!(
        lexed.code[open..close].contains("setVisibility(NotificationCompat.VISIBILITY_SECRET)"),
        "{}: the code's notification is not VISIBILITY_SECRET",
        path.display()
    );
    let (open, close) = function_around(&lexed, at).expect("the function that builds it");
    for refused in ["setPublicVersion", "VISIBILITY_PRIVATE", "VISIBILITY_PUBLIC"] {
        assert!(
            !lexed.code[open..close].contains(refused),
            "{}: the code's notification has {refused}",
            path.display()
        );
    }
}

/// SEC-CLI-121: a Firebase build reaches Google only once an account is signed in here. `src/fcm`'s manifest turns
/// FCM's auto-init off (`firebase_messaging_auto_init_enabled` false, and no manifest sets it otherwise), so Firebase
/// makes no token at the start; `Push.start` asks for one (`Push.startFcm`, the one place that names `FcmPush`) only
/// after `Native.accounts` has an account, and a sign-in asks from Rust (`MobileHost::account_signed_in`, `fcm_start`).
#[test]
fn a_firebase_build_asks_fcm_for_a_token_only_once_an_account_is_signed_in() {
    let key = "firebase_messaging_auto_init_enabled";
    let fcm = without_xml_comments(&read(&android().join("app/src/fcm/AndroidManifest.xml")));
    let application = without_elements(&fcm, "service");
    assert!(
        meta_data(&application).contains(&(key.to_string(), "false".to_string())),
        "src/fcm's manifest does not turn FCM's auto-init off: {:?}",
        meta_data(&fcm)
    );
    for (_, dir) in source_sets() {
        let path = dir.join("AndroidManifest.xml");
        if !path.exists() {
            continue;
        }
        for (name, value) in meta_data(&without_xml_comments(&read(&path))) {
            assert!(name != key || value == "false", "{}: {name}={value}", path.display());
        }
    }

    let push = lex(&read(&android().join("app/src/main/java/org/moqui/mobile/Push.kt")));
    let start = body_of(&push, "fun start(").expect("Push.start");
    let accounts = start.find("Native.accounts(").expect("Push.start reads the accounts");
    let asks = start.find("startFcm(").expect("Push.start asks Firebase");
    assert!(accounts < asks, "Push.start asks Firebase before it reads the accounts");
    let start_fcm = body_of(&push, "fun startFcm(").expect("Push.startFcm");
    assert!(start_fcm.contains("org.moqui.mobile.FcmPush"));
    assert_eq!(
        push.code.matches("FcmPush").count(),
        1,
        "FcmPush is named in Push.startFcm only"
    );

    let host = lex(&read(&root().join("crates/moqui-mobile/src/host.rs")));
    let signed_in = body_of(&host, "fn account_signed_in(").expect("MobileHost::account_signed_in");
    assert!(signed_in.contains("fcm_start()"), "a sign-in does not start FCM");
}

/// SEC-CLI-123 (the phone's part): the app's WebView sends Google no metrics, on a phone whose owner allows usage reports
/// too: the main manifest's application (not one of its components) has `android.webkit.WebView.MetricsOptOut` true,
/// and no source set's manifest sets it otherwise.
#[test]
fn the_android_apps_webview_sends_google_no_metrics() {
    let key = "android.webkit.WebView.MetricsOptOut";
    let manifest = without_xml_comments(&read(&android().join("app/src/main/AndroidManifest.xml")));
    let mut application = manifest
        .split_once("<application")
        .map(|(_, rest)| rest.to_string())
        .expect("the main manifest's application element");
    for component in ["activity", "activity-alias", "service", "receiver", "provider"] {
        application = without_elements(&application, component);
    }
    assert!(
        meta_data(&application).contains(&(key.to_string(), "true".to_string())),
        "the application's meta-data: {:?}",
        meta_data(&application)
    );
    for (_, dir) in source_sets() {
        let path = dir.join("AndroidManifest.xml");
        if !path.exists() {
            continue;
        }
        for (name, value) in meta_data(&without_xml_comments(&read(&path))) {
            assert!(name != key || value == "true", "{}: {name}={value}", path.display());
        }
    }
}

/// SEC-CLI-116: below Android 12, which itself drops a touch that passes through an untrusted overlay, the app does
/// (`filterTouchesWhenObscured`), so another app's window cannot pass the user's taps through to a call's Answer, a
/// permission question or a dialog's button: each activity the manifests declare sets it on its decor view in its
/// `onCreate`, after `super.onCreate` (`ObscuredTouches.refuse(window.decorView)`); wry's `RustWebView` on itself in
/// its `init`; and each Kotlin file that builds an `AlertDialog` shows each one through its side's helper
/// (`ObscuredTouches.show`, wry's `RustWebView.refuseObscuredTouches` on the shown dialog's window), whose bodies set
/// it. Nothing sets it false.
#[test]
fn every_activity_webview_and_dialog_of_the_android_app_drops_touches_through_another_apps_window() {
    let on = "filterTouchesWhenObscured = true";
    let app = android().join("app/src/main/java/org/moqui/mobile");
    let wry = root().join("crates/moqui-mobile/patches/wry/src/android/kotlin");
    let helper = lex(&read(&app.join("ObscuredTouches.kt")));
    assert!(body_of(&helper, "fun refuse(").is_some_and(|body| body.contains(on)));
    assert!(body_of(&helper, "fun show(").is_some_and(|body| body.contains("refuse(")));
    let view = lex(&read(&wry.join("RustWebView.kt")));
    assert!(body_of(&view, "fun refuseObscuredTouches(").is_some_and(|body| body.contains(on)));
    assert!(
        body_of(&view, "init {").is_some_and(|body| body.contains("refuseObscuredTouches(this)")),
        "RustWebView's init does not refuse obscured touches"
    );

    let mut kotlin = Vec::new();
    let mut activities = Vec::new();
    for (_, dir) in source_sets() {
        kotlin.extend(files_under(&dir, &|name| name.ends_with(".kt")));
        let manifest = dir.join("AndroidManifest.xml");
        if manifest.exists() {
            for tag in start_tags(&without_xml_comments(&read(&manifest)), "activity") {
                activities.extend(attribute_values(tag, "android:name"));
            }
        }
    }
    assert!(activities.len() >= 2, "the activities were found: {activities:?}");
    for activity in &activities {
        let class = activity.rsplit('.').next().unwrap_or_default();
        let path = kotlin
            .iter()
            .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(class))
            .unwrap_or_else(|| panic!("no Kotlin file of {activity}"));
        let lexed = lex(&read(path));
        let created = body_of(&lexed, "fun onCreate(")
            .and_then(|body| body.split_once("super.onCreate("))
            .map(|(_, after)| after)
            .unwrap_or_else(|| panic!("{}: no onCreate calling super.onCreate", path.display()));
        assert!(
            created.contains("ObscuredTouches.refuse(window.decorView)")
                || created.contains("window.decorView.filterTouchesWhenObscured = true"),
            "{}: onCreate does not refuse obscured touches on its decor view after super.onCreate",
            path.display()
        );
    }

    kotlin.extend(files_under(&wry, &|name| name.ends_with(".kt")));
    let mut dialogs = 0;
    for path in &kotlin {
        let code = lex(&read(path)).code;
        for refused in [
            "filterTouchesWhenObscured = false",
            "setFilterTouchesWhenObscured(false)",
        ] {
            assert!(!code.contains(refused), "{} has {refused}", path.display());
        }
        let built = code.matches("AlertDialog.Builder(").count() + code.matches("MaterialAlertDialogBuilder(").count();
        let shown =
            code.matches("ObscuredTouches.show(").count() + code.matches("RustWebView.refuseObscuredTouches(").count();
        assert!(
            shown >= built,
            "{}: {built} dialogs built, {shown} shown refusing obscured touches",
            path.display()
        );
        dialogs += built;
    }
    assert!(dialogs >= 7, "the dialogs were found: {dialogs}");
}

/// SEC-CLI-120 (the `notify` half): a page's notification never asks for the notification permission, since any server
/// page may call `notify`: no Rust of the app calls the notification plugin's `request_permission`, and `notify` reads
/// the permission's state and fails without it ("Notifications are off for this app"). Android's question comes with
/// its reason after a sign-in (`MainActivity.askForCallsOnce`).
#[test]
fn no_page_notification_asks_for_the_notification_permission() {
    let src = root().join("crates/moqui-mobile/src");
    let rust = files_under(&src, &|name| {
        name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs")
    });
    assert!(rust.len() > 10, "the Rust was found: {rust:?}");
    for path in &rust {
        assert!(
            !lex(&read(path)).code.contains("request_permission"),
            "{} asks for a permission",
            path.display()
        );
    }
    let commands = lex(&read(&src.join("commands.rs")));
    let notify = body_of(&commands, "fn notify(").expect("the notify command");
    assert!(
        notify.contains("permission_state()") && notify.contains("Notifications are off for this app"),
        "notify does not fail without the permission"
    );
}

/// The offset of the bracket that closes the one at `open` (`(` or `{`) in `lexed`, its strings and comments left out;
/// the text's end when it is not closed.
fn closing(lexed: &Lexed, open: usize) -> usize {
    let bare = lexed.bare.as_bytes();
    let (opens, closes) = match bare[open] {
        b'(' => (b'(', b')'),
        b'{' => (b'{', b'}'),
        other => panic!("not an opening bracket: {}", other as char),
    };
    let mut depth = 0;
    for (at, byte) in bare.iter().enumerate().skip(open) {
        if *byte == opens {
            depth += 1;
        } else if *byte == closes {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    bare.len()
}

/// The text of the string resource `name` in the resources `xml` (`<string name="...">text</string>`).
fn string_resource(xml: &str, name: &str) -> Option<String> {
    let open = format!("<string name=\"{name}\">");
    let (_, rest) = xml.split_once(&open)?;
    rest.split_once("</string>").map(|(text, _)| text.to_string())
}

/// SEC-CLI-89 and SEC-CLI-110: a camera capture from a page's file input, photo or video, stays in the app: the
/// FileProvider's paths (`res/xml/file_paths.xml`, and any other of a source set a release build merges) have no
/// `external-*` or `root-path` entry and none of a whole folder (`path="."`), only the cache's `captures/`; wry's
/// `RustWebChromeClient` writes no external files folder (`getExternalFilesDir`), its photo and its video capture
/// each give the camera app an `EXTRA_OUTPUT` made by `createCaptureFile` in the cache's `captures/`, delete it when
/// nothing was taken, and the video's no longer fails (`!!`) on a camera app that returns no data; the folder is
/// emptied as the next webview's chrome client starts (its `init`).
#[test]
fn a_camera_capture_from_a_page_stays_in_the_apps_cache() {
    let mut paths = Vec::new();
    for (name, dir) in source_sets() {
        if name == "debug" {
            continue;
        }
        for path in files_under(&dir.join("res"), &|file| file.ends_with(".xml")) {
            if without_xml_comments(&read(&path)).contains("<paths") {
                paths.push(path);
            }
        }
    }
    let main = android().join("app/src/main/res/xml/file_paths.xml");
    assert!(paths.contains(&main), "the FileProvider's paths were found: {paths:?}");
    for path in &paths {
        let xml = without_xml_comments(&read(path));
        for refused in ["<root-path", "<external-"] {
            assert!(!xml.contains(refused), "{}: a {refused} entry", path.display());
        }
        for value in attribute_values(&xml, "path") {
            let folder = value.trim().trim_start_matches("./").trim_matches('/');
            assert!(
                !folder.is_empty() && folder != ".",
                "{}: path=\"{value}\" gives a whole folder",
                path.display()
            );
        }
    }
    let main_xml = without_xml_comments(&read(&main));
    let entries = start_tags(&main_xml, "cache-path");
    assert_eq!(entries.len(), 1, "{}: one cache-path", main.display());
    assert_eq!(attribute_values(entries[0], "path"), ["captures/"]);

    let path = root().join("crates/moqui-mobile/patches/wry/src/android/kotlin/RustWebChromeClient.kt");
    let chrome = lex(&read(&path));
    for refused in [
        "getExternalFilesDir",
        "getExternalStoragePublicDirectory",
        "getExternalCacheDir",
    ] {
        assert!(!chrome.code.contains(refused), "{}: {refused}", path.display());
    }
    assert!(
        chrome.code.contains("const val CAPTURES = \"captures\""),
        "{}: the captures folder",
        path.display()
    );
    let create = body_of(&chrome, "fun createCaptureFile(").expect("createCaptureFile");
    assert!(
        create.contains("File(activity.cacheDir, CAPTURES)"),
        "a capture is not made in the cache's captures/"
    );
    for intent in ["MediaStore.ACTION_IMAGE_CAPTURE", "MediaStore.ACTION_VIDEO_CAPTURE"] {
        let at = chrome
            .code
            .find(intent)
            .unwrap_or_else(|| panic!("{}: no {intent}", path.display()));
        let (open, close) = function_around(&chrome, at).expect("the capture's function");
        let capture = &chrome.code[open..close];
        for wanted in ["createCaptureFile(", "putExtra(MediaStore.EXTRA_OUTPUT", ".delete()"] {
            assert!(
                capture.contains(wanted),
                "{}: the {intent} capture has no {wanted}",
                path.display()
            );
        }
        assert!(
            !capture.contains("!!"),
            "{}: the {intent} capture fails on no data",
            path.display()
        );
    }
    assert!(
        body_of(&chrome, "init {").is_some_and(|init| init.contains("sweepCaptures()")),
        "{}: the chrome client's init does not empty the captures",
        path.display()
    );
    assert!(
        body_of(&chrome, "fun sweepCaptures(").is_some_and(|sweep| sweep.contains("File(activity.cacheDir, CAPTURES)")),
        "{}: sweepCaptures does not empty the cache's captures/",
        path.display()
    );
}

/// SEC-CLI-119: a page's alert, confirm and prompt are the app's own dialogs, so each names who asks: wry's
/// `RustWebChromeClient` sets each one's title (`dialogTitle`), which is the server's host for the app origin, set by
/// Rust as it opens a server's pages (`open_remote`, through `RustWebChromeClient.pageServer`) and cleared as they close
/// (`close_remotes`); `onJsPrompt` refuses (`result.cancel()`) on the app's two origins before it builds a dialog, so a
/// page of one account's server cannot ask as the app for another account's password; and a page's `notify` is titled
/// by `notification_title` (commands_tests.rs).
#[test]
fn a_pages_dialogs_name_who_asks_and_a_prompt_is_refused_on_the_apps_origins() {
    let path = root().join("crates/moqui-mobile/patches/wry/src/android/kotlin/RustWebChromeClient.kt");
    let chrome = lex(&read(&path));
    for origin in [
        "const val SHELL_ORIGIN = \"http://moqui-shell.localhost\"",
        "const val APP_ORIGIN = \"http://moqui-app.localhost\"",
    ] {
        assert!(chrome.code.contains(origin), "{}: no {origin}", path.display());
    }
    for handler in ["fun onJsAlert(", "fun onJsConfirm(", "fun onJsPrompt("] {
        let body = body_of(&chrome, handler).unwrap_or_else(|| panic!("{}: no {handler}", path.display()));
        assert!(
            body.contains(".setTitle(dialogTitle(url))"),
            "{}: {handler} shows a dialog with no title of who asks",
            path.display()
        );
    }
    let title = body_of(&chrome, "fun dialogTitle(").expect("dialogTitle");
    assert!(
        title.contains("SHELL_ORIGIN ->") && title.contains("APP_ORIGIN -> appServer"),
        "{title}"
    );

    let header = chrome.code.find("fun onJsPrompt(").expect("onJsPrompt");
    let open = header + chrome.bare[header..].find('{').expect("onJsPrompt's body");
    let close = closing(&chrome, open);
    let built = chrome.code[open..close]
        .find("AlertDialog.Builder(")
        .expect("the prompt's dialog")
        + open;
    let refused = chrome.code[open..built].match_indices("if (").any(|(at, _)| {
        let condition = open + at + "if ".len();
        let end = closing(&chrome, condition);
        let then = end + chrome.bare[end..].find('{').unwrap_or(0);
        let block = &chrome.code[then..closing(&chrome, then).min(built)];
        let condition = &chrome.code[condition..end];
        condition.contains("SHELL_ORIGIN")
            && condition.contains("APP_ORIGIN")
            && block.contains("result.cancel()")
            && block.contains("return true")
    });
    assert!(
        refused,
        "{}: onJsPrompt does not cancel on the app's origins before its dialog",
        path.display()
    );

    let mobile = root().join("crates/moqui-mobile/src");
    let android = lex(&read(&mobile.join("android.rs")));
    assert!(
        body_of(&android, "fn page_server(").is_some_and(|body| body.contains("jni_str!(\"pageServer\")")),
        "page_server does not tell RustWebChromeClient.pageServer"
    );
    let host = lex(&read(&mobile.join("host.rs")));
    let open_remote = body_of(&host, "fn open_remote(").expect("open_remote");
    let told = open_remote
        .find("tell_page_server(server")
        .expect("open_remote tells the server");
    let shown = open_remote.find("self.navigate(").expect("open_remote shows the page");
    assert!(told < shown, "open_remote shows the page before it tells its server");
    assert!(
        body_of(&host, "fn close_remotes(").is_some_and(|body| body.contains("tell_page_server(None)")),
        "close_remotes does not clear the pages' server"
    );
    assert!(
        body_of(&host, "fn tell_page_server(").is_some_and(|body| body.contains("crate::android::page_server(")),
        "tell_page_server does not tell the chrome client"
    );
    let commands = lex(&read(&mobile.join("commands.rs")));
    let notify = body_of(&commands, "fn notify(").expect("the notify command");
    assert!(
        notify.contains("notification_title("),
        "notify's title does not name the page's server"
    );
}

/// SEC-CLI-120 (the microphone half): the app asks for the microphone after a sign-in only behind a dialog that says
/// why: in `MainActivity.askForCallsOnce`, every `requestPermissions` is in the dialog's buttons and listeners (within
/// its one `ObscuredTouches.show(...)`), and each title and reason it picks when the microphone is missing
/// (`microphone` in the arm's condition) names the microphone and a call answered on a locked phone.
#[test]
fn the_android_app_asks_for_the_microphone_only_after_a_dialog_that_says_why() {
    let main = android().join("app/src/main");
    let path = main.join("java/org/moqui/mobile/MainActivity.kt");
    let activity = lex(&read(&path));
    let header = activity.code.find("fun askForCallsOnce(").expect("askForCallsOnce");
    let open = header + activity.bare[header..].find('{').expect("askForCallsOnce's body");
    let close = closing(&activity, open);
    let body = &activity.code[open..close];
    assert!(
        body.contains("Manifest.permission.RECORD_AUDIO"),
        "askForCallsOnce asks for the microphone"
    );
    let shows: Vec<usize> = body
        .match_indices("ObscuredTouches.show(")
        .map(|(at, _)| open + at)
        .collect();
    assert_eq!(shows.len(), 1, "{}: one dialog in askForCallsOnce", path.display());
    let dialog_open = shows[0] + "ObscuredTouches.show".len();
    let dialog_close = closing(&activity, dialog_open);
    let asks: Vec<usize> = body
        .match_indices("requestPermissions(")
        .map(|(at, _)| open + at)
        .collect();
    assert!(!asks.is_empty(), "askForCallsOnce asks for nothing");
    for at in asks {
        assert!(
            (dialog_open..dialog_close).contains(&at),
            "{}: askForCallsOnce asks for permissions outside its dialog's buttons (byte {at})",
            path.display()
        );
    }
    let dialog = &activity.code[dialog_open..dialog_close];
    assert!(
        dialog.contains(".setTitle(heading)") && dialog.contains(".setMessage(reason)"),
        "{dialog}"
    );

    let chosen = body
        .find("val (heading, reason) = when {")
        .expect("the dialog's title and reason, by what is missing");
    let when_open = open + chosen + body[chosen..].find('{').unwrap();
    let arms = &activity.code[when_open + 1..closing(&activity, when_open)];
    let strings = read(&main.join("res/values/strings.xml"));
    let mut microphone_arms = 0;
    for arm in arms.lines().filter(|line| line.contains("->")) {
        let (condition, value) = arm.split_once("->").unwrap();
        let names: Vec<&str> = value
            .split("R.string.")
            .skip(1)
            .map(|rest| {
                rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(names.len(), 2, "a title and a reason: {arm}");
        let asks_microphone = condition
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '!')
            .any(|word| word == "microphone");
        if !asks_microphone {
            continue;
        }
        microphone_arms += 1;
        let reason = string_resource(&strings, names[1]).unwrap_or_else(|| panic!("no string {}", names[1]));
        assert!(
            reason.contains("microphone") && reason.contains("locked phone"),
            "{}: the reason shown for the microphone does not say why: {reason}",
            names[1]
        );
        assert!(string_resource(&strings, names[0]).is_some(), "no string {}", names[0]);
    }
    assert!(
        microphone_arms >= 2,
        "the arms that ask for the microphone were found: {arms}"
    );
}
