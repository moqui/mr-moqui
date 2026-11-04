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
use gix::objs::Kind;

use super::super::ops::{self, Author, Edit};
use super::super::store::{GitStore, MemoryStore};
use super::write_commit;

fn author() -> Author {
    Author {
        name: "Test User".to_string(),
        email: "test@example.com".to_string(),
        user_id: None,
        seconds: None,
    }
}

fn commit(store: &dyn GitStore, edits: Vec<Edit>) -> gix::ObjectId {
    ops::commit(store, "main", &author(), |_| {
        Ok(Some((edits.clone(), "files".to_string())))
    })
    .unwrap()
    .unwrap()
}

fn blob(store: &dyn GitStore, text: &str) -> gix::ObjectId {
    store.write(Kind::Blob, text.as_bytes()).unwrap()
}

fn path(text: &str) -> Vec<String> {
    text.split('/').map(str::to_string).collect()
}

/// A commit's files are written to a new directory, nested ones in their directories, an executable one executable;
/// a symbolic link is left out and named, an LFS pointer is written and named, and a target that exists is refused.
#[test]
fn a_commit_is_written_to_a_plain_directory() {
    let store = MemoryStore::default();
    let edits = vec![
        Edit::Put(
            path("component.xml"),
            EntryKind::Blob,
            blob(&store, "<component name=\"c\"/>"),
        ),
        Edit::Put(
            path("service/c/CServices.xml"),
            EntryKind::Blob,
            blob(&store, "<services/>"),
        ),
        Edit::Put(
            path("bin/run.sh"),
            EntryKind::BlobExecutable,
            blob(&store, "#!/bin/sh\n"),
        ),
        Edit::Put(path("link"), EntryKind::Link, blob(&store, "component.xml")),
        Edit::Put(
            path("data/big.bin"),
            EntryKind::Blob,
            blob(
                &store,
                "version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 1\n",
            ),
        ),
    ];
    let first = commit(&store, edits);
    let dir = std::env::temp_dir().join(format!("moqui-git-checkout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let target = dir.join("c");
    let written = write_commit(&store, "main", None, &target).unwrap();
    assert_eq!(written.commit, first.to_string());
    assert_eq!(written.files, 4);
    assert_eq!(written.skipped, ["link"]);
    assert_eq!(written.lfs_pointers, ["data/big.bin"]);
    assert_eq!(
        std::fs::read_to_string(target.join("service/c/CServices.xml")).unwrap(),
        "<services/>"
    );
    assert!(!target.join("link").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target.join("bin/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }
    assert!(
        write_commit(&store, "main", None, &target).is_err(),
        "a target that exists is refused"
    );

    // an earlier commit by its id
    let second = commit(
        &store,
        vec![Edit::Put(
            path("component.xml"),
            EntryKind::Blob,
            blob(&store, "<component name=\"c2\"/>"),
        )],
    );
    let earlier = dir.join("earlier");
    let written = write_commit(&store, "main", Some(&first.to_string()), &earlier).unwrap();
    assert_eq!(written.commit, first.to_string());
    assert!(std::fs::read_to_string(earlier.join("component.xml"))
        .unwrap()
        .contains("\"c\""));
    let changes = super::super::diff::tree_changes(
        &store,
        Some(&ops::read_commit(&store, &first).unwrap().tree),
        Some(&ops::read_commit(&store, &second).unwrap().tree),
    )
    .unwrap();
    let paths: Vec<String> = changes.into_iter().map(|change| change.path).collect();
    assert_eq!(paths, ["component.xml"]);
    let _ = std::fs::remove_dir_all(&dir);
}
