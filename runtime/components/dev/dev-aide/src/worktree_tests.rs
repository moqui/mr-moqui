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

use std::path::{Path, PathBuf};

use super::{default_branch, redirects, removal_refusal, tag_of, worktree_dir};
use crate::roots::{Root, RootKind};

fn root(id: &str, worktree_of: Option<&str>, conversation: Option<&str>) -> Root {
    Root {
        id: id.into(),
        label: id.into(),
        path: PathBuf::from(format!("/r/{id}")),
        kind: RootKind::Git,
        components: Vec::new(),
        upstream: false,
        read_only: false,
        publish_branch: None,
        publish_branch_required: false,
        hosted: None,
        worktree_of: worktree_of.map(str::to_string),
        conversation_id: conversation.map(str::to_string),
    }
}

#[test]
fn a_worktree_is_named_from_its_root_and_session() {
    assert_eq!(tag_of("Conv 100023/x"), "conv-100023-x");
    assert_eq!(default_branch("100023"), "agent/100023");
    assert_eq!(
        worktree_dir(Path::new("/rt/worktrees"), "my-site", "100023"),
        PathBuf::from("/rt/worktrees/my-site-100023")
    );
}

/// Two sessions' worktrees of one repository: each session's tools are rooted in its own.
#[test]
fn each_session_has_its_own_worktree_of_a_root() {
    let roots = [
        root("my-site", None, None),
        root("my-site-c1", Some("my-site"), Some("C1")),
        root("my-site-c2", Some("my-site"), Some("C2")),
    ];
    assert_eq!(
        redirects(&roots, "C1"),
        [("my-site".to_string(), "my-site-c1".to_string())]
    );
    assert_eq!(
        redirects(&roots, "C2"),
        [("my-site".to_string(), "my-site-c2".to_string())]
    );
    assert!(redirects(&roots, "C3").is_empty());
    assert!(redirects(&roots, "").is_empty());
}

/// A worktree with uncommitted changes is not removed unless they are to be discarded.
#[test]
fn removal_refuses_uncommitted_changes_unless_discarded() {
    assert!(removal_refusal("wt", "", false).is_none());
    let why = removal_refusal("wt", " M a.xml\n?? b.xml\n", false).unwrap();
    assert!(why.contains("2 files"), "{why}");
    assert!(removal_refusal("wt", " M a.xml\n", true).is_none());
}
