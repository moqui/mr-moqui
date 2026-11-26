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

use super::{default_branch, in_worktrees, redirects, removal_refusal, session_refusal, tag_of, worktree_dir};
use crate::roots::{Root, RootKind};

fn root(id: &str, worktree_of: Option<&str>, conversation: Option<&str>) -> Root {
    rooted(id, &format!("/r/{id}"), worktree_of, conversation, Some("ANN"))
}

fn rooted(id: &str, path: &str, worktree_of: Option<&str>, conversation: Option<&str>, user: Option<&str>) -> Root {
    Root {
        id: id.into(),
        label: id.into(),
        path: PathBuf::from(path),
        kind: RootKind::Git,
        components: Vec::new(),
        upstream: false,
        read_only: false,
        publish_branch: None,
        publish_branch_required: false,
        hosted: None,
        worktree_of: worktree_of.map(str::to_string),
        conversation_id: conversation.map(str::to_string),
        worktree_user_id: worktree_of.and(user).map(str::to_string),
        hosted_branch: None,
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
        redirects(&roots, "C1", "ANN"),
        [("my-site".to_string(), "my-site-c1".to_string())]
    );
    assert_eq!(
        redirects(&roots, "C2", "ANN"),
        [("my-site".to_string(), "my-site-c2".to_string())]
    );
    assert!(redirects(&roots, "C3", "ANN").is_empty());
    assert!(redirects(&roots, "", "ANN").is_empty());
}

/// A worktree with uncommitted changes is not removed unless they are to be discarded.
#[test]
fn removal_refuses_uncommitted_changes_unless_discarded() {
    assert!(removal_refusal("wt", "", false).is_none());
    let why = removal_refusal("wt", " M a.xml\n?? b.xml\n", false).unwrap();
    assert!(why.contains("2 files"), "{why}");
    assert!(removal_refusal("wt", " M a.xml\n", true).is_none());
}

/// A worktree is a session's only for the user who made it, whatever conversation it names.
#[test]
fn another_users_worktree_is_never_a_sessions() {
    let roots = [
        root("my-site", None, None),
        rooted("my-site-c1", "/r/my-site-c1", Some("my-site"), Some("C1"), Some("BOB")),
    ];
    assert!(redirects(&roots, "C1", "ANN").is_empty());
    assert_eq!(redirects(&roots, "C1", "BOB").len(), 1);
    assert!(redirects(&roots, "C1", "").is_empty());
}

/// A session with a worktree reaches neither another session's worktree nor a root inside the tree its worktree stands
/// in for (a nested repository of the running checkout); others it reaches.
#[test]
fn a_session_reaches_only_its_own_worktree_of_a_tree() {
    let roots = [
        rooted("checkout", "/w/checkout", None, None, None),
        rooted(
            "mantle",
            "/w/checkout/runtime/components/addon/mantle",
            None,
            None,
            None,
        ),
        rooted("other", "/w/other", None, None, None),
        rooted(
            "checkout-c1",
            "/w/checkout/runtime/worktrees/checkout-c1",
            Some("checkout"),
            Some("C1"),
            Some("ANN"),
        ),
        rooted(
            "checkout-c2",
            "/w/checkout/runtime/worktrees/checkout-c2",
            Some("checkout"),
            Some("C2"),
            Some("ANN"),
        ),
    ];
    let redirect = redirects(&roots, "C1", "ANN");
    assert!(session_refusal(&roots, &redirect, &roots[3]).is_none());
    assert!(session_refusal(&roots, &redirect, &roots[2]).is_none());
    assert!(session_refusal(&roots, &redirect, &roots[4])
        .unwrap()
        .contains("another session"));
    assert!(session_refusal(&roots, &redirect, &roots[1]).is_some());
    assert!(session_refusal(&roots, &redirect, &roots[0]).is_some());
    assert!(session_refusal(&roots, &[], &roots[4]).is_none());
}

/// remove#Worktree removes only in the runtime's worktrees directory.
#[test]
fn only_a_worktree_under_the_runtime_is_removed() {
    let runtime = Path::new("/nowhere/runtime");
    assert!(in_worktrees(
        runtime,
        Path::new("/nowhere/runtime/worktrees/my-site-c1")
    ));
    assert!(!in_worktrees(runtime, Path::new("/nowhere/runtime/worktrees")));
    assert!(!in_worktrees(runtime, Path::new("/nowhere/my-site")));
    assert!(!in_worktrees(
        runtime,
        Path::new("/nowhere/runtime/components/addon/my-site")
    ));
}
