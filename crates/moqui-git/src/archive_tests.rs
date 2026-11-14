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

use gix::objs::tree::EntryKind;

use super::super::store::MemoryStore;
use super::*;

const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_entries: 1000,
    max_disk_bytes: u64::MAX,
    max_concurrent: 1,
};

fn blob(store: &MemoryStore, data: &[u8]) -> ObjectId {
    store.write(Kind::Blob, data).unwrap()
}

/// A tree of these entries, in git's encoding, as they are given: no client sorts or checks them.
fn tree(store: &MemoryStore, entries: &[(&[u8], EntryKind, ObjectId)]) -> ObjectId {
    let mut buf = Vec::new();
    for (name, kind, oid) in entries {
        let mode = match kind {
            EntryKind::Tree => "40000",
            EntryKind::Blob => "100644",
            EntryKind::BlobExecutable => "100755",
            EntryKind::Link => "120000",
            EntryKind::Commit => "160000",
        };
        buf.extend_from_slice(mode.as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(name);
        buf.push(0);
        buf.extend_from_slice(oid.as_bytes());
    }
    store.write(Kind::Tree, &buf).unwrap()
}

fn walked(store: &MemoryStore, root: &ObjectId, limits: Limits) -> Result<Walked, Error> {
    walk(store, &|id| Ok(store.object_size(id)?.unwrap_or(0)), root, limits)
}

fn refusal(store: &MemoryStore, root: &ObjectId, limits: Limits) -> String {
    walked(store, root, limits).unwrap_err().message().to_string()
}

#[test]
fn a_tree_is_walked_in_order_with_its_links_and_submodules() {
    let store = MemoryStore::default();
    let file = blob(&store, b"content\n");
    let inside = blob(&store, b"../a.txt");
    let absolute = blob(&store, b"/etc/passwd");
    let climbs = blob(&store, b"../../outside");
    let dir = tree(
        &store,
        &[
            (b"b.txt", EntryKind::Blob, file),
            (b"climbs", EntryKind::Link, climbs),
            (b"inside", EntryKind::Link, inside),
        ],
    );
    let root = tree(
        &store,
        &[
            (b"a.txt", EntryKind::Blob, file),
            (b"abs", EntryKind::Link, absolute),
            (b"dir", EntryKind::Tree, dir),
            (b"run.sh", EntryKind::BlobExecutable, file),
            (b"sub", EntryKind::Commit, ObjectId::null(gix::hash::Kind::Sha1)),
        ],
    );
    let found = walked(&store, &root, LIMITS).unwrap();
    let entries: Vec<(&str, &What)> = found
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.what))
        .collect();
    assert_eq!(
        entries,
        [
            ("a.txt", &What::File { executable: false }),
            ("run.sh", &What::File { executable: true }),
            ("sub", &What::Directory),
            ("dir/b.txt", &What::File { executable: false }),
            ("dir/inside", &What::Link("../a.txt".to_string())),
        ]
    );
    // a link out of the archive is left out and named; the content is the files', each time it is in the tree
    assert_eq!(found.skipped, ["abs", "dir/climbs"]);
    assert_eq!(found.content_bytes, 3 * 8);
    assert!(link_stays_inside("b/../c", 0) && link_stays_inside("./x", 0));
    assert!(!link_stays_inside("..", 0) && !link_stays_inside("a/../../b", 0) && !link_stays_inside("", 3));
    assert!(!link_stays_inside("c:\\windows", 2) && link_stays_inside("../..", 2));
}

#[test]
fn names_that_could_leave_the_archive_refuse_it() {
    let store = MemoryStore::default();
    let file = blob(&store, b"x");
    // a NUL ends a name in a tree, so no tree holds one
    assert_eq!(entry_name(b"a\0b"), None);
    let hostile: [&[u8]; 8] = [b"..", b".", b".git", b".GIT", b"", b"a/b", b"a\\b", b"\xff\xfe"];
    for name in hostile {
        let root = tree(
            &store,
            &[(b"ok.txt", EntryKind::Blob, file), (name, EntryKind::Blob, file)],
        );
        let refused = refusal(&store, &root, LIMITS);
        assert!(refused.contains("an archive cannot hold"), "{name:?}: {refused}");
        // a directory of that name too, and below another directory
        let below = tree(&store, &[(name, EntryKind::Tree, root)]);
        let outer = tree(&store, &[(b"dir", EntryKind::Tree, below)]);
        assert!(
            refusal(&store, &outer, LIMITS).contains("an archive cannot hold: dir/"),
            "{name:?}"
        );
    }
    let twice = tree(
        &store,
        &[(b"same", EntryKind::Blob, file), (b"same", EntryKind::Link, file)],
    );
    assert!(refusal(&store, &twice, LIMITS).contains("names same more than once"));
    assert_eq!(entry_name(b".gitignore"), Some(".gitignore"));
    assert_eq!(entry_name("caf\u{e9} 1.txt".as_bytes()), Some("caf\u{e9} 1.txt"));
}

#[test]
fn a_deep_tree_and_a_tree_bomb_are_refused_without_being_walked_whole() {
    let store = MemoryStore::default();
    let file = blob(&store, b"x");
    // 100,000 directories, one in the other: no stack is spent on them
    let mut deep = tree(&store, &[(b"f", EntryKind::Blob, file)]);
    for _ in 0..100_000 {
        deep = tree(&store, &[(b"d", EntryKind::Tree, deep)]);
    }
    let refused = refusal(&store, &deep, LIMITS);
    assert!(refused.contains("deeper than 64 directories"), "{refused}");
    // as deep as is taken
    let mut level = tree(&store, &[(b"f", EntryKind::Blob, file)]);
    for _ in 0..MAX_DEPTH {
        level = tree(&store, &[(b"d", EntryKind::Tree, level)]);
    }
    assert_eq!(walked(&store, &level, LIMITS).unwrap().entries.len(), 1);

    // a bomb: ten levels of a tree that names the one below it a hundred times, 100^10 directories and no file
    let names: Vec<String> = (0..100).map(|n| format!("n{n:02}")).collect();
    let mut bomb = tree(&store, &[]);
    for _ in 0..10 {
        let entries: Vec<(&[u8], EntryKind, ObjectId)> = names
            .iter()
            .map(|name| (name.as_bytes(), EntryKind::Tree, bomb))
            .collect();
        bomb = tree(&store, &entries);
    }
    let started = std::time::Instant::now();
    let refused = refusal(&store, &bomb, LIMITS);
    assert!(refused.contains("more than 1000 entries"), "{refused}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    // and one of files: the bytes are counted as often as they are named
    let big = blob(&store, &vec![b'x'; 300_000]);
    let files: Vec<(&[u8], EntryKind, ObjectId)> = names
        .iter()
        .map(|name| (name.as_bytes(), EntryKind::Blob, big))
        .collect();
    let refused = refusal(&store, &tree(&store, &files), LIMITS);
    assert!(refused.contains("more than 1048576 bytes"), "{refused}");
}

#[test]
fn the_builds_at_once_are_counted_and_an_old_file_is_swept() {
    let building = AtomicUsize::new(0);
    let first = BuildSlot::take(&building, 1).expect("one build");
    assert!(BuildSlot::take(&building, 1).is_none(), "a second at once");
    drop(first);
    assert!(BuildSlot::take(&building, 1).is_some());
    assert!(BuildSlot::take(&building, 0).is_some(), "a limit of none is one");

    let dir = std::env::temp_dir().join(format!("moqui-git-archive-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let aged = |name: &str, age: Duration| {
        let path = dir.join(name);
        std::fs::write(&path, b"12345").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
        path
    };
    let used = aged("used.zip", Duration::from_secs(60));
    let old = aged("old.zip", KEEP + Duration::from_secs(60));
    let writing = aged("new.0011.tmp", Duration::from_secs(60));
    let dead = aged("dead.0022.tmp", KEEP_PARTIAL + Duration::from_secs(60));
    assert_eq!(sweep(&dir), 10, "the bytes of what is kept");
    assert!(used.exists() && writing.exists() && !old.exists() && !dead.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
