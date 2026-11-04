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

//! The content group's entities (`DbResource*`, `ContentBlob*`, `GitObject`, `GitRef`, `GitLfsObject`) are read and
//! written only by the resource implementations and the framework's maintenance, so they can be on a database of their
//! own (ResourceEntities.xml, docs/Run-Deploy.md "Content database"). Everything else goes through the resource facade
//! (`ec.resource` in a script, `ResourceFacade` in Rust). Tests, and data files (which the data loader writes), may
//! name them.

use std::path::{Path, PathBuf};

/// The content group's entities.
const CONTENT_ENTITIES: &[&str] = &[
    "DbResource",
    "DbResourceFile",
    "DbResourceFileHistory",
    "ContentBlob",
    "ContentBlobChunk",
    "ContentBlobUpload",
    "GitObject",
    "GitRef",
    "GitLfsObject",
];

/// The Rust sources that may use the content entities, or the `dbresource://` implementation's methods: that
/// implementation, the resource facade that calls it, the blob store, the maintenance services, and the git storage and
/// hosting. A path ending in `/` covers the directory.
const RUST_ALLOWED: &[&str] = &[
    "crates/moqui-entity/src/entity/dbresource.rs",
    "crates/moqui-entity/src/entity/dbresource/",
    "crates/moqui-entity/src/resource/blob.rs",
    "crates/moqui-core/src/context/resource.rs",
    "crates/moqui-core/src/context/resource/",
    "crates/moqui-core/src/service/content.rs",
    "crates/moqui-git/src/db.rs",
    "crates/moqui-git/src/lib.rs",
    "crates/moqui-git/src/lfs_files.rs",
    "crates/moqui-git/src/gc.rs",
    "crates/moqui-git/src/gc/",
    "crates/moqui-git/src/server/",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The files under `dir` with one of `extensions`, leaving out vendored and generated trees and the directories
/// `skip` names.
fn files(dir: &Path, extensions: &[&str], skip: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "node_modules" | "libs" | ".git") && !skip.contains(&name.as_str()) {
                files(&path, extensions, skip, out);
            }
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.contains(&ext))
        {
            out.push(path);
        }
    }
}

fn relative(path: &Path) -> String {
    let root = std::fs::canonicalize(root()).unwrap();
    let path = std::fs::canonicalize(path).unwrap();
    path.strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Whether `text` has `name` at `at` as a whole name: not followed by a letter or digit (so `DbResource` is not found
/// in `DbResourceFile`).
fn whole_at(text: &str, at: usize, name: &str) -> bool {
    !text[at + name.len()..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
}

/// The content entity `text` names in full (`moqui.resource.DbResource`), if any.
fn full_name_in(text: &str) -> Option<String> {
    CONTENT_ENTITIES.iter().find_map(|entity| {
        let name = format!("moqui.resource.{entity}");
        text.match_indices(&name)
            .any(|(at, _)| whole_at(text, at, &name))
            .then_some(name)
    })
}

/// A call of a `dbresource://` implementation method (`.write_db_resource(`, `.make_db_directory(`) in `text`.
fn db_resource_call_in(text: &str) -> Option<String> {
    text.match_indices('.').find_map(|(at, _)| {
        let rest = &text[at + 1..];
        let end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))?;
        let name = &rest[..end];
        let called = rest[end..].starts_with('(');
        (called && (name.contains("db_resource") || name == "make_db_directory")).then(|| name.to_string())
    })
}

/// A test file or directory, which may name the entities.
fn is_test(relative: &str) -> bool {
    relative.ends_with("_tests.rs")
        || relative.ends_with("/tests.rs")
        || relative.ends_with("/testing.rs")
        || relative.contains("/tests/")
        || relative.contains("/testkit/")
}

#[test]
fn only_the_resource_implementations_use_the_content_entities_in_rust() {
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root().join("crates")).unwrap().filter_map(Result::ok) {
        files(&entry.path().join("src"), &["rs"], &[], &mut sources);
    }
    assert!(sources.len() > 100, "the crates' sources were not found");
    let mut found = Vec::new();
    for path in sources {
        let relative = relative(&path);
        if is_test(&relative)
            || RUST_ALLOWED
                .iter()
                .any(|allowed| relative == *allowed || (allowed.ends_with('/') && relative.starts_with(allowed)))
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        if let Some(name) = full_name_in(&text) {
            found.push(format!("{relative} names {name}"));
        }
        if let Some(call) = db_resource_call_in(&text) {
            found.push(format!("{relative} calls {call}"));
        }
    }
    assert!(
        found.is_empty(),
        "use ResourceFacade (factory.resource()) for these, not the content entities or the dbresource:// \
         implementation:\n{}",
        found.join("\n")
    );
}

/// The content entity a service, screen or script of a component reads or writes: an `entity-name` attribute, an
/// `entity-auto` service's noun, or a name in quotes in a script.
fn entity_use_in(text: &str, script: bool) -> Option<String> {
    CONTENT_ENTITIES.iter().find_map(|entity| {
        let full = format!("moqui.resource.{entity}");
        let names = [full.as_str(), entity];
        let used = names.iter().any(|name| {
            let quoted = |quote: char| text.contains(&format!("{quote}{name}{quote}"));
            if script {
                quoted('"') || quoted('\'')
            } else {
                text.contains(&format!("entity-name=\"{name}\""))
                    || text.split("<service ").skip(1).any(|service| {
                        let head = service.split('>').next().unwrap_or_default();
                        head.contains("type=\"entity-auto\"") && head.contains(&format!("noun=\"{name}\""))
                    })
            }
        });
        used.then_some(full)
    })
}

#[test]
fn no_service_screen_or_script_uses_the_content_entities() {
    let runtime = root().join("runtime");
    let mut sources = Vec::new();
    for dir in ["base-component", "component", "dev-component"] {
        files(
            &runtime.join(dir),
            &["xml", "js", "mjs", "qvue", "ftl"],
            &["data", "entity", "test", "plan", "docs"],
            &mut sources,
        );
    }
    assert!(sources.len() > 100, "the components' files were not found");
    let mut found = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let script = path.extension().is_some_and(|ext| ext != "xml");
        if let Some(name) = entity_use_in(&text, script) {
            found.push(format!("{} uses {name}", relative(&path)));
        }
    }
    assert!(
        found.is_empty(),
        "use ec.resource for these, not the content entities:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scans_find_what_they_look_for() {
    assert_eq!(
        full_name_in(r#"find("moqui.resource.DbResourceFile")"#).as_deref(),
        Some("moqui.resource.DbResourceFile")
    );
    assert_eq!(full_name_in("moqui.resource.DbResourceTag"), None);
    assert_eq!(
        db_resource_call_in("facade.write_db_resource(&location, bytes)").as_deref(),
        Some("write_db_resource")
    );
    assert_eq!(
        db_resource_call_in("facade.make_db_directory(x)").as_deref(),
        Some("make_db_directory")
    );
    assert_eq!(db_resource_call_in("fn read_db_resource(&self)"), None);
    assert!(entity_use_in(
        r#"<entity-find entity-name="moqui.resource.GitObject" list="x"/>"#,
        false
    )
    .is_some());
    assert!(entity_use_in(
        r#"<service verb="create" noun="DbResource" type="entity-auto"/>"#,
        false
    )
    .is_some());
    assert!(entity_use_in(
        r#"<service verb="create" noun="GitRef" type="rust" location="x"/>"#,
        false
    )
    .is_none());
    assert!(entity_use_in("ec.entity.find('ContentBlob')", true).is_some());
    assert!(entity_use_in("// ContentBlob chunks", true).is_none());
}
