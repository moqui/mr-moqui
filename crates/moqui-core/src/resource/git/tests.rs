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

//! `git://` locations through the resource facade, for each repository kind: the same scenario over a bare
//! repository on disk and over `GitObject`/`GitRef` rows, then export, import and a mirror fetch.

use std::path::{Path, PathBuf};

use moqui_lib::object::Value;

use gix::objs::Kind;

use super::store::{GitStore, MemoryStore, RefEdit, RefOutcome};
use super::{fs, ops};
use crate::context::{ExecutionContext, ExecutionContextFactory, ResourceFacade};
use crate::entity::{EntityFacade, EntityValue};

struct Fixture {
    context: ExecutionContext,
    entity: EntityFacade,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(name: &str) -> Fixture {
    let context = ExecutionContextFactory::test_context();
    let entity = crate::testing::open();
    context.factory().set_entity(entity.clone());
    context.factory().resource().install_git().unwrap();
    let dir = std::env::temp_dir().join(format!("moqui-git-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Fixture { context, entity, dir }
}

fn define(entity: &EntityFacade, id: &str, kind: &str, path: Option<&Path>, remote_url: Option<&str>) {
    let mut row = EntityValue::make(entity, super::GIT_REPOSITORY).unwrap();
    row.set("repositoryId", Value::string(id));
    row.set("repositoryKind", Value::string(kind));
    if let Some(path) = path {
        row.set("localPath", Value::string(path.to_string_lossy()));
    }
    if let Some(url) = remote_url {
        row.set("remoteUrl", Value::string(url));
    }
    row.create(entity).unwrap();
}

fn page(edit: usize) -> String {
    (1..=12)
        .map(|line| {
            if line == edit {
                format!("line {line} edited\n")
            } else {
                format!("line {line}\n")
            }
        })
        .collect()
}

/// Writes, reads, lists, versions, a diff, a move and deletes under `git://<id>/`. The first version's id.
fn scenario(resources: &ResourceFacade, id: &str) -> String {
    let a = format!("git://{id}/notes/a.md");
    let b = format!("git://{id}/notes/b.md");
    let root = format!("git://{id}/");
    assert_eq!(
        resources.stat(&root).unwrap().map(|stat| stat.is_file),
        Some(false),
        "empty root is a directory"
    );
    assert!(resources.list(&root).unwrap().is_empty());
    assert_eq!(resources.stat(&a).unwrap(), None);

    resources.write_bytes(&a, page(0).as_bytes()).unwrap();
    resources.write_bytes(&b, b"other\n").unwrap();
    resources.write_bytes(&a, page(5).as_bytes()).unwrap();
    // the same bytes again make no commit
    resources.write_bytes(&a, page(5).as_bytes()).unwrap();
    assert_eq!(resources.get(&a).unwrap().read_text().unwrap(), page(5));
    let stat = resources.stat(&a).unwrap().expect("a.md");
    assert!(stat.is_file);
    assert_eq!(stat.size, page(5).len() as u64);
    assert!(stat.last_modified.is_some());
    let names: Vec<String> = resources
        .list(&root)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["notes"]);
    let notes: Vec<(String, bool)> = resources
        .list(&format!("git://{id}/notes"))
        .unwrap()
        .into_iter()
        .map(|entry| (entry.location, entry.is_file))
        .collect();
    assert_eq!(notes, [(a.clone(), true), (b.clone(), true)]);

    assert!(resources.supports_version(&a));
    let history = resources.version_history(&a).unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    let (second, first) = (history[0].version_name.clone(), history[1].version_name.clone());
    assert_eq!(history[0].previous_version_name.as_deref(), Some(first.as_str()));
    assert_eq!(resources.current_version(&a).unwrap().unwrap().version_name, second);
    assert_eq!(resources.root_version(&a).unwrap().unwrap().version_name, first);
    let next: Vec<String> = resources
        .next_versions(&a, &first)
        .unwrap()
        .into_iter()
        .map(|version| version.version_name)
        .collect();
    assert_eq!(next, std::slice::from_ref(&second));
    assert_eq!(
        resources.version_text(&a, &first).unwrap().as_deref(),
        Some(page(0).as_str())
    );
    assert_eq!(
        resources.version_text(&a, &first[..10]).unwrap().as_deref(),
        Some(page(0).as_str())
    );
    assert_eq!(resources.version_text(&a, "0000000").unwrap(), None);
    assert_eq!(resources.location_text(&format!("{a}#{first}")).unwrap(), page(0));
    let diff = resources.version_diff(&a, &first, &second).unwrap();
    assert_eq!((diff.stats.insertions, diff.stats.deletions), (1, 1));
    assert_eq!(resources.version_history(&b).unwrap().len(), 1);

    resources.rename(&a, &a).unwrap();
    assert_eq!(
        resources.version_history(&a).unwrap().len(),
        2,
        "a move onto itself makes no commit"
    );
    let moved = format!("git://{id}/archive/a.md");
    resources.rename(&a, &moved).unwrap();
    assert_eq!(resources.stat(&a).unwrap(), None);
    assert_eq!(resources.get(&moved).unwrap().read_text().unwrap(), page(5));
    let err = resources.delete(&format!("git://{id}/notes")).unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    assert!(resources.delete(&moved).unwrap());
    assert!(!resources.delete(&moved).unwrap());
    let names: Vec<String> = resources
        .list(&root)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["notes"], "the emptied archive directory is gone");
    // the deleted file's old versions are still in the history of its path
    assert_eq!(
        resources.version_text(&a, &first).unwrap().as_deref(),
        Some(page(0).as_str())
    );
    first
}

#[test]
fn a_local_repository_is_a_bare_repository_on_disk() {
    let fixture = fixture("local");
    let _entered = fixture.context.enter();
    let path = fixture.dir.join("local.git");
    define(&fixture.entity, "notes", "local", Some(&path), None);
    let resources = fixture.context.factory().resource();
    scenario(resources, "notes");
    assert!(path.join("HEAD").is_file(), "created on first use");
    let repo = gix::open(&path).unwrap();
    assert!(repo.is_bare());
    let head = repo
        .find_reference("refs/heads/main")
        .unwrap()
        .into_fully_peeled_id()
        .unwrap();
    let commit = repo.find_commit(head).unwrap();
    assert_eq!(
        commit.message_raw_sloppy().to_string().lines().next(),
        Some("Delete archive/a.md")
    );
    let row = fixture
        .entity
        .find(super::GIT_REPOSITORY)
        .unwrap()
        .eq("repositoryId", Value::string("notes"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(super::text(row.get("headCommitId")), Some(head.to_string()));
    if let Some(log) = git(&["--git-dir", &path.to_string_lossy(), "log", "--format=%s", "main"]) {
        assert_eq!(log.lines().count(), 5, "{log}");
    }
}

#[test]
fn a_db_repository_exports_and_imports_with_the_same_ids() {
    let fixture = fixture("db");
    let _entered = fixture.context.enter();
    define(&fixture.entity, "wiki", "db", None, None);
    let resources = fixture.context.factory().resource();
    let first = scenario(resources, "wiki");
    let objects = fixture.entity.find(super::db::GIT_OBJECT).unwrap().list().unwrap();
    assert!(objects.len() > 10, "{} objects", objects.len());

    let provider = resources.git().expect("git provider");
    let export = fixture.dir.join("export.git");
    let (path, written) = provider.export("wiki", Some(&export.to_string_lossy())).unwrap();
    assert_eq!(path, export);
    assert!(written > 10);
    let store = fs::FsStore::open(&export).unwrap();
    let db_head = super::db::DbStore::new(fixture.entity.clone(), "wiki", super::db::object_cache())
        .read_ref("refs/heads/main")
        .unwrap();
    assert_eq!(store.read_ref("refs/heads/main").unwrap(), db_head);
    assert_eq!(
        ops::read_at(
            &store,
            &ObjectIdHex::parse(&first),
            &["notes".to_string(), "a.md".to_string()]
        )
        .unwrap(),
        Some(page(0).into_bytes())
    );
    if let Some(fsck) = git(&["--git-dir", &export.to_string_lossy(), "fsck", "--strict"]) {
        assert!(!fsck.contains("error"), "{fsck}");
    }

    define(&fixture.entity, "copy", "db", None, None);
    let (head, copied) = provider.import("copy", Some(&export.to_string_lossy())).unwrap();
    assert_eq!(Some(head.clone()), db_head.map(|id| id.to_string()));
    assert_eq!(copied, written);
    assert_eq!(
        resources.get("git://copy/notes/b.md").unwrap().read_text().unwrap(),
        "other\n"
    );
    define(&fixture.entity, "copy2", "db", None, None);
    let (head2, _) = provider.import("copy2", Some("git://copy")).unwrap();
    assert_eq!(head2, head);
    let err = provider.import("copy2", Some("git://copy2")).unwrap_err();
    assert!(err.to_string().contains("into itself"), "{err}");
}

#[test]
fn a_remote_repository_is_a_read_only_mirror_kept_by_fetch() {
    if git(&["--version"]).is_none() {
        eprintln!("skipped: a file:// fetch runs git-upload-pack, and git is not on the PATH");
        return;
    }
    let fixture = fixture("remote");
    let _entered = fixture.context.enter();
    let source = fixture.dir.join("source.git");
    define(&fixture.entity, "source", "local", Some(&source), None);
    let mirror = fixture.dir.join("mirror.git");
    let url = format!("file://{}", source.display());
    define(&fixture.entity, "mirror", "remote", Some(&mirror), Some(&url));
    let resources = fixture.context.factory().resource();
    let provider = resources.git().expect("git provider");
    resources.write_bytes("git://source/doc.md", b"v1\n").unwrap();

    let err = resources.get("git://mirror/doc.md").unwrap().read_text().unwrap_err();
    assert!(err.to_string().contains("has not been fetched"), "{err}");
    let head = provider.fetch("mirror").unwrap().expect("a head");
    assert_eq!(
        resources.get("git://mirror/doc.md").unwrap().read_text().unwrap(),
        "v1\n"
    );
    let err = resources.write_bytes("git://mirror/doc.md", b"no").unwrap_err();
    assert!(err.to_string().contains("read-only mirror"), "{err}");

    resources.write_bytes("git://source/doc.md", b"v2\n").unwrap();
    let newer = provider.fetch("mirror").unwrap().expect("a head");
    assert_ne!(newer, head);
    assert_eq!(
        resources.get("git://mirror/doc.md").unwrap().read_text().unwrap(),
        "v2\n"
    );
    assert_eq!(resources.version_history("git://mirror/doc.md").unwrap().len(), 2);

    // every branch and tag is mirrored, and one deleted at the source is deleted in the mirror
    let source_store = fs::FsStore::open(&source).unwrap();
    let head_id = gix::ObjectId::from_hex(newer.as_bytes()).unwrap();
    for name in ["refs/heads/feature", "refs/tags/v1.0"] {
        assert!(source_store.compare_and_set_ref(name, None, head_id).unwrap());
    }
    provider.fetch("mirror").unwrap();
    let mirrored = |prefix: &str| fs::FsStore::open(&mirror).unwrap().list_refs(prefix).unwrap();
    assert_eq!(
        mirrored("refs/"),
        [
            ("refs/heads/feature".to_string(), head_id),
            ("refs/heads/main".to_string(), head_id),
            ("refs/tags/v1.0".to_string(), head_id)
        ]
    );
    assert_eq!(
        source_store
            .update_refs(
                &[super::store::RefEdit {
                    name: "refs/heads/feature".into(),
                    expected: Some(head_id),
                    new: None
                }],
                true
            )
            .unwrap(),
        [RefOutcome::Updated]
    );
    provider.fetch("mirror").unwrap();
    assert_eq!(mirrored("refs/heads/"), [("refs/heads/main".to_string(), head_id)]);
    assert_eq!(mirrored("refs/tags/").len(), 1);

    fixture.entity.set_sim_active(true);
    let err = provider.fetch("mirror").unwrap_err();
    fixture.entity.set_sim_active(false);
    assert!(err.to_string().contains("sim mode"), "{err}");

    define(&fixture.entity, "imported", "db", None, Some(&url));
    let (imported, _) = provider.import("imported", None).unwrap();
    assert_eq!(imported, newer);
}

#[test]
fn refs_logs_commits_and_diffs_read_any_revision() {
    let fixture = fixture("browse");
    let _entered = fixture.context.enter();
    define(&fixture.entity, "code", "db", None, None);
    let resources = fixture.context.factory().resource();
    let provider = resources.git().expect("git provider").clone();
    resources.write_bytes("git://code/src/a.rs", b"fn a() {}\n").unwrap();
    resources.write_bytes("git://code/README.md", b"# Code\n").unwrap();
    resources
        .write_bytes("git://code/src/a.rs", b"fn a() {\n    b();\n}\n")
        .unwrap();
    let head = provider.resolve_commit("code", "").unwrap().expect("a head");
    let store = super::db::DbStore::new(fixture.entity.clone(), "code", super::db::object_cache());
    assert!(store.compare_and_set_ref("refs/tags/v1", None, head).unwrap());

    let refs = provider.refs("code", "refs/").unwrap();
    assert_eq!(refs.branch, "main");
    assert_eq!(refs.head, Some(head));
    let names: Vec<(&str, super::browse::RefKind)> =
        refs.refs.iter().map(|info| (info.name.as_str(), info.kind)).collect();
    assert_eq!(
        names,
        [
            ("refs/heads/main", super::browse::RefKind::Branch),
            ("refs/tags/v1", super::browse::RefKind::Tag)
        ]
    );
    assert_eq!(refs.refs[1].commit.as_ref().map(|commit| commit.id), Some(head));

    let log = provider.log("code", "", &[], None, 50).unwrap();
    let summaries: Vec<&str> = log.commits.iter().map(|commit| commit.summary()).collect();
    assert_eq!(summaries, ["Update src/a.rs", "Create README.md", "Create src/a.rs"]);
    assert!(!log.more);
    let page = provider.log("code", "v1", &[], None, 1).unwrap();
    assert_eq!(page.commits.len(), 1);
    assert!(page.more);
    let next = provider
        .log("code", "v1", &[], Some(&page.commits[0].id.to_string()[..10]), 1)
        .unwrap();
    assert_eq!(next.commits[0].summary(), "Create README.md");
    let path = vec!["src".to_string(), "a.rs".to_string()];
    let of_path = provider.log("code", "", &path, None, 50).unwrap();
    assert_eq!(of_path.commits.len(), 2, "only the commits that changed src/a.rs");

    let short = &head.to_string()[..8];
    let (commit, changes) = provider.commit_changes("code", short).unwrap().expect("the head");
    assert_eq!(commit.id, head);
    assert!(commit.author.email.contains('@'), "{:?}", commit.author);
    assert_eq!(commit.committer, commit.author);
    let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs"]);
    let diffs = provider.commit_diffs("code", "HEAD", None).unwrap().unwrap();
    let diff = diffs[0].1.diff.as_ref().expect("a text diff");
    assert_eq!((diff.stats.insertions, diff.stats.deletions), (3, 1));
    assert_eq!(
        provider.blob_at("code", "v1", &["README.md".to_string()]).unwrap(),
        Some(b"# Code\n".to_vec())
    );
    assert_eq!(provider.resolve_commit("code", "no-such-branch").unwrap(), None);
}

#[test]
fn locations_name_their_repository_and_refuse_odd_paths() {
    let fixture = fixture("errors");
    let _entered = fixture.context.enter();
    let resources = fixture.context.factory().resource();
    let err = resources.stat("git://nosuch/x").unwrap_err();
    assert!(
        err.to_string().contains("git repository nosuch is not defined"),
        "{err}"
    );
    assert!(!resources.get("git://nosuch/x").unwrap().exists());
    let err = resources.stat("git://nosuch/../x").unwrap_err();
    assert!(err.to_string().contains("bad path"), "{err}");
    define(&fixture.entity, "odd", "elsewhere", None, None);
    let err = resources.stat("git://odd/x").unwrap_err();
    assert!(err.to_string().contains("must be remote, local or db"), "{err}");
    define(&fixture.entity, "plain", "db", None, None);
    let err = resources.rename("git://plain/a", "git://odd/a").unwrap_err();
    assert!(err.to_string().contains("between git repositories"), "{err}");
}

fn edit(name: &str, expected: Option<gix::ObjectId>, new: Option<gix::ObjectId>) -> RefEdit {
    RefEdit {
        name: name.to_string(),
        expected,
        new,
    }
}

/// Ref updates, ref listing and batch object operations, the same on each kind of store.
fn store_scenario(store: &dyn GitStore) {
    use RefOutcome::{Aborted, Stale, Updated};
    let a = store.write(Kind::Blob, b"a").unwrap();
    let b = store.write(Kind::Blob, b"b").unwrap();
    let c = store.write(Kind::Blob, b"c").unwrap();
    let main = "refs/heads/main";
    let tag = "refs/tags/v1_0";
    assert_eq!(
        store
            .update_refs(&[edit(tag, None, Some(b)), edit(main, None, Some(a))], true)
            .unwrap(),
        [Updated, Updated]
    );
    let all = |prefix: &str| store.list_refs(prefix).unwrap();
    assert_eq!(all("refs/"), [(main.to_string(), a), (tag.to_string(), b)]);
    assert_eq!(all("refs/tags/"), [(tag.to_string(), b)]);
    assert_eq!(
        all("refs/tags/v1_"),
        [(tag.to_string(), b)],
        "an underscore is not a wildcard"
    );
    assert!(all("refs/tags/v1x").is_empty());

    // one stale edit in an atomic update: nothing changes
    let mixed = [edit(main, Some(a), Some(c)), edit(tag, Some(c), Some(a))];
    assert_eq!(store.update_refs(&mixed, true).unwrap(), [Aborted, Stale]);
    assert_eq!(store.read_ref(main).unwrap(), Some(a));
    // the same edits one by one: the good one is made
    assert_eq!(store.update_refs(&mixed, false).unwrap(), [Updated, Stale]);
    assert_eq!(store.read_ref(main).unwrap(), Some(c));
    // a create of a ref that is there, a delete of one that is not
    assert_eq!(store.update_refs(&[edit(main, None, Some(a))], true).unwrap(), [Stale]);
    assert_eq!(
        store
            .update_refs(&[edit("refs/heads/gone", Some(a), None)], true)
            .unwrap(),
        [Stale]
    );
    assert_eq!(store.update_refs(&[edit(tag, Some(b), None)], true).unwrap(), [Updated]);
    assert_eq!(all("refs/"), [(main.to_string(), c)]);
    assert!(store.compare_and_set_ref(main, Some(c), a).unwrap());
    assert!(!store.compare_and_set_ref(main, Some(c), b).unwrap());

    let hex = a.to_string();
    assert_eq!(store.find_prefix(&hex[..8], 5).unwrap(), [a]);
    assert!(store.find_prefix("zz", 5).unwrap().is_empty());
    let unknown = gix::ObjectId::from_hex(b"0123456789012345678901234567890123456789").unwrap();
    let present = store.contains_many(&[a, unknown, c]).unwrap();
    assert_eq!(present, [a, c].into_iter().collect());
    let mut objects = [
        Ok((Kind::Blob, b"a".to_vec())),
        Ok((Kind::Blob, b"new 1".to_vec())),
        Ok((Kind::Blob, b"new 2".to_vec())),
        Ok((Kind::Blob, b"new 1".to_vec())),
    ]
    .into_iter();
    assert_eq!(
        store.write_batch(&mut objects).unwrap(),
        2,
        "two new objects, one repeated"
    );
    let new = super::store::object_id(Kind::Blob, b"new 2").unwrap();
    assert_eq!(store.find(&new).unwrap().map(|(_, data)| data), Some(b"new 2".to_vec()));
}

#[test]
fn ref_updates_and_batches_are_the_same_on_each_store() {
    store_scenario(&MemoryStore::default());
    let fixture = fixture("stores");
    let _entered = fixture.context.enter();
    let path = fixture.dir.join("store.git");
    let fs_store = fs::FsStore::init(&path, "main").unwrap();
    store_scenario(&fs_store);
    store_scenario(&super::db::DbStore::new(
        fixture.entity.clone(),
        "stores",
        super::db::object_cache(),
    ));

    // a delete of a packed ref (git pack-refs moves loose refs into packed-refs)
    let packed = fixture.dir.join("packed.git");
    let store = fs::FsStore::init(&packed, "main").unwrap();
    let a = store.write(Kind::Blob, b"a").unwrap();
    assert!(store.compare_and_set_ref("refs/tags/old", None, a).unwrap());
    if git(&["--git-dir", &packed.to_string_lossy(), "pack-refs", "--all"]).is_some() {
        assert!(!packed.join("refs/tags/old").exists(), "packed");
        let store = fs::FsStore::open(&packed).unwrap();
        assert_eq!(
            store.list_refs("refs/tags/").unwrap(),
            [("refs/tags/old".to_string(), a)]
        );
        assert_eq!(
            store
                .update_refs(&[edit("refs/tags/old", Some(a), None)], true)
                .unwrap(),
            [RefOutcome::Updated]
        );
        assert!(store.list_refs("refs/").unwrap().is_empty());
        assert_eq!(
            fs::FsStore::open(&packed).unwrap().read_ref("refs/tags/old").unwrap(),
            None
        );
    }
}

/// The output of the git command line, or `None` when git is not installed.
fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr))
}

struct ObjectIdHex;

impl ObjectIdHex {
    fn parse(hex: &str) -> gix::ObjectId {
        gix::ObjectId::from_hex(hex.as_bytes()).unwrap()
    }
}

/// The rows of a db repository: id to (storage, kind).
fn rows_of(entity: &EntityFacade, id: &str) -> std::collections::HashMap<String, (String, String)> {
    let _rows = super::rows_guard();
    entity
        .find("moqui.resource.GitObject")
        .unwrap()
        .eq("repositoryId", Value::string(id))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            let text = |name: &str| {
                row.get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            (text("objectId"), (text("storage"), text("objectType")))
        })
        .collect()
}

fn gc_options(dry_run: bool) -> super::gc::GcOptions {
    let mut options = super::gc::GcOptions::from_conf(dry_run, std::time::Duration::from_secs(120));
    options.reflog_millis = 0;
    options
}

#[test]
fn gc_deletes_what_no_ref_reaches_and_repacks_the_rest() {
    let fixture = fixture("gc");
    let _entered = fixture.context.enter();
    define(&fixture.entity, "gc", "db", None, None);
    let resources = fixture.context.factory().resource();
    let provider = resources.git().unwrap().clone();
    // ten versions of a file, as a wiki page's saves are: whole rows
    let text = |version: usize| -> String {
        (0..400)
            .map(|line| {
                if line == version * 7 {
                    format!("line {line}, version {version}\n")
                } else {
                    format!("line {line} of a page\n")
                }
            })
            .collect()
    };
    for version in 0..10 {
        resources
            .write_bytes("git://gc/page.md", text(version).as_bytes())
            .unwrap();
    }
    let store = super::db::DbStore::new(fixture.entity.clone(), "gc", super::db::object_cache());
    // a branch with a file of its own, then the branch deleted: unreachable
    let author = ops::Author {
        name: "Test".into(),
        email: "test@example.com".into(),
        user_id: None,
        seconds: None,
    };
    let main = store.read_ref("refs/heads/main").unwrap().unwrap();
    store
        .update_refs(
            &[RefEdit {
                name: "refs/heads/feature".into(),
                expected: None,
                new: Some(main),
            }],
            true,
        )
        .unwrap();
    let lost = store.write(Kind::Blob, b"only on the feature branch\n").unwrap();
    ops::commit(&store, "feature", &author, |_| {
        Ok(Some((
            vec![ops::Edit::Put(
                vec!["feature.txt".into()],
                gix::objs::tree::EntryKind::Blob,
                lost,
            )],
            "Feature".into(),
        )))
    })
    .unwrap()
    .unwrap();
    let feature = store.read_ref("refs/heads/feature").unwrap().unwrap();
    store
        .update_refs(
            &[RefEdit {
                name: "refs/heads/feature".into(),
                expected: Some(feature),
                new: None,
            }],
            true,
        )
        .unwrap();
    // an unreachable blob that a writer references again keeps
    let kept = store.write(Kind::Blob, b"referenced again\n").unwrap();
    let month_ago = jiff::Timestamp::now().as_millisecond() - 30 * 24 * 60 * 60 * 1000;
    store.stamp_all(month_ago);
    store.freshen(&[kept]).unwrap();
    let before = rows_of(&fixture.entity, "gc");
    assert!(before.values().all(|(storage, _)| storage == "zlib"));

    // a dry run counts and changes nothing
    let dry = provider.gc(Some("gc"), &gc_options(true)).unwrap();
    let report = dry[0].1.as_ref().unwrap();
    assert_eq!(
        report.deleted, 3,
        "the blob, tree and commit of the feature: {report:?}"
    );
    assert_eq!(rows_of(&fixture.entity, "gc").len(), before.len());

    let outcome = provider.gc(Some("gc"), &gc_options(false)).unwrap();
    let report = outcome[0].1.as_ref().unwrap();
    assert_eq!(report.deleted, 3, "{report:?}");
    assert!(report.repacked >= 8, "{report:?}");
    assert!(report.saved_bytes > 0, "{report:?}");
    let after = rows_of(&fixture.entity, "gc");
    assert!(!after.contains_key(&lost.to_string()));
    assert!(!after.contains_key(&feature.to_string()));
    assert!(after.contains_key(&kept.to_string()));
    let deltas = after
        .values()
        .filter(|(storage, kind)| storage == "delta" && kind == "blob")
        .count();
    assert!(deltas >= 8, "{deltas} blob deltas");
    // every version still reads, through a cache of its own
    let fresh = super::db::DbStore::new(fixture.entity.clone(), "gc", super::db::object_cache());
    let history = resources.version_history("git://gc/page.md").unwrap();
    assert_eq!(history.len(), 10);
    for version in &history {
        let read = resources
            .read_version("git://gc/page.md", &version.version_name)
            .unwrap()
            .unwrap();
        assert!(String::from_utf8(read).unwrap().starts_with("line 0"));
    }
    assert_eq!(resources.get("git://gc/page.md").unwrap().read_text().unwrap(), text(9));
    for id in after.keys() {
        let id = gix::ObjectId::from_hex(id.as_bytes()).unwrap();
        assert!(fresh.find(&id).unwrap().is_some(), "{id} reads");
    }
    let fsck = provider
        .fsck("gc", std::time::Instant::now() + std::time::Duration::from_secs(60))
        .unwrap();
    assert!(fsck.errors.is_empty(), "{:?}", fsck.errors);
    assert_eq!(fsck.checked, after.len());
    {
        let _rows = super::rows_guard();
        let row = fixture
            .entity
            .find(super::GIT_REPOSITORY)
            .unwrap()
            .eq("repositoryId", Value::string("gc"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(
            row.get("objectCount").map(moqui_lib::object::to_plain_string),
            Some(after.len().to_string())
        );
        assert!(row
            .get("lastGcResult")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
            .contains("deleted 3"));
        assert_eq!(row.get("gcLeaseOwner"), Some(&Value::Null));
    }

    // a second run has nothing to do
    let again = provider.gc(Some("gc"), &gc_options(false)).unwrap();
    let report = again[0].1.as_ref().unwrap();
    assert_eq!((report.deleted, report.repacked), (0, 0), "{report:?}");

    // a repository whose refs reach a missing object is not collected
    {
        let _rows = super::rows_guard();
        let head = store.read_ref("refs/heads/main").unwrap().unwrap();
        let tree = ops::read_commit(&store, &head).unwrap().tree;
        fixture
            .entity
            .find("moqui.resource.GitObject")
            .unwrap()
            .eq("repositoryId", Value::string("gc"))
            .eq("objectId", Value::string(tree.to_string()))
            .delete_all()
            .unwrap();
    }
    let damaged = provider.gc(Some("gc"), &gc_options(false)).unwrap();
    let err = damaged[0].1.as_ref().unwrap_err();
    assert!(err.message().contains("missing object"), "{}", err.message());
}

#[test]
fn lfs_files_through_git_locations_are_content_blobs_behind_pointers() {
    let fixture = fixture("lfs-files");
    let _entered = fixture.context.enter();
    define(&fixture.entity, "site", "db", None, None);
    let resources = fixture.context.factory().resource();
    let provider = resources.git().unwrap().clone();
    let patterns = super::attributes::DEFAULT_LFS_PATTERNS;
    let before = provider.setup_lfs("site", &patterns).unwrap().unwrap();
    assert!(
        provider.setup_lfs("site", &patterns).unwrap().is_none(),
        "the patterns are there"
    );
    let image: Vec<u8> = (0..300_000u32).map(|n| (n * 17 % 253) as u8).collect();
    resources.write_bytes("git://site/images/logo.png", &image).unwrap();
    resources.write_bytes("git://site/readme.md", b"# Site\n").unwrap();

    // the commit holds a pointer; the content is a blob of the repository
    let store = super::db::DbStore::new(fixture.entity.clone(), "site", super::db::object_cache());
    let head = store.read_ref("refs/heads/main").unwrap().unwrap();
    let tree = ops::read_commit(&store, &head).unwrap().tree;
    let path = |text: &str| text.split('/').map(str::to_string).collect::<Vec<_>>();
    let (_, pointer_blob) = ops::resolve(&store, &tree, &path("images/logo.png")).unwrap().unwrap();
    let pointer = ops::object(&store, &pointer_blob, Kind::Blob).unwrap();
    let oid = crate::resource::blob::blob_id(&image);
    assert_eq!(
        super::server::lfs::pointer(&pointer),
        Some((oid.clone(), image.len() as u64))
    );
    let (_, readme) = ops::resolve(&store, &tree, &path("readme.md")).unwrap().unwrap();
    assert_eq!(ops::object(&store, &readme, Kind::Blob).unwrap(), b"# Site\n");

    // read, stat and open give the content
    assert_eq!(
        resources
            .get("git://site/images/logo.png")
            .unwrap()
            .read_bytes()
            .unwrap(),
        image
    );
    assert_eq!(
        resources.stat("git://site/images/logo.png").unwrap().unwrap().size,
        image.len() as u64
    );
    let opened = resources.open("git://site/images/logo.png").unwrap().unwrap();
    assert_eq!(opened.source.read_at(299_990, 100).unwrap(), image[299_990..]);
    assert_eq!(opened.source.tag(), Some(format!("sha256-{oid}")));

    // the branch put back to before the file, and old, its LFS object goes, then its content
    let now = store.read_ref("refs/heads/main").unwrap();
    let edit = RefEdit {
        name: "refs/heads/main".into(),
        expected: now,
        new: Some(before),
    };
    assert_eq!(store.update_refs(&[edit], true).unwrap(), [RefOutcome::Updated]);
    {
        // as a push would leave it
        let _rows = super::rows_guard();
        let mut row = fixture
            .entity
            .find(super::GIT_REPOSITORY)
            .unwrap()
            .eq("repositoryId", Value::string("site"))
            .one()
            .unwrap()
            .unwrap();
        row.set("headCommitId", Value::string(before.to_string()));
        row.update(&fixture.entity).unwrap();
    }
    provider.refresh("site");
    let month_ago = jiff::Timestamp::now().as_millisecond() - 30 * 24 * 60 * 60 * 1000;
    store.stamp_all(month_ago);
    {
        let _rows = super::rows_guard();
        let mut row = fixture
            .entity
            .find("moqui.resource.GitLfsObject")
            .unwrap()
            .eq("repositoryId", Value::string("site"))
            .one()
            .unwrap()
            .unwrap();
        row.set(
            "uploadedDate",
            moqui_lib::object::basic_convert(&Value::I64(month_ago), "java.sql.Timestamp").value,
        );
        row.update(&fixture.entity).unwrap();
    }
    let outcome = provider.gc(Some("site"), &gc_options(false)).unwrap();
    let report = outcome[0].1.as_ref().unwrap();
    assert_eq!(report.lfs_deleted, 1, "{report:?}");
    let blobs = crate::resource::blob::BlobStore::new(fixture.entity.clone());
    let (deleted, bytes, _) = blobs
        .collect(jiff::Timestamp::now().as_millisecond() + 1000, false)
        .unwrap();
    assert_eq!((deleted, bytes), (1, image.len() as u64));
}
