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

//! The git layer on the real git command, in a working tree whose root is canonical as the dev-aide's roots are: on
//! Windows a `\\?\C:\...` path, which git is run in and whose files confinement names. Git is kept from the desktop
//! (no global or system config, a scratch home, no prompt). Without git on the PATH the test says so and passes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::confine;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The environment that keeps git from the desktop's config, credentials and prompts, with a fixed identity; and from
/// the repository a git hook runs this test in (GIT_DIR, GIT_INDEX_FILE and the rest set to the scratch tree's own,
/// since `Git::run` adds variables and removes none).
fn isolated(home: &Path, root: &Path) -> Vec<(&'static str, String)> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let home = home.display().to_string();
    let git_dir = root.join(".git");
    vec![
        ("GIT_DIR", git_dir.display().to_string()),
        ("GIT_WORK_TREE", root.display().to_string()),
        ("GIT_INDEX_FILE", git_dir.join("index").display().to_string()),
        ("GIT_COMMON_DIR", git_dir.display().to_string()),
        ("GIT_OBJECT_DIRECTORY", git_dir.join("objects").display().to_string()),
        ("HOME", home.clone()),
        ("USERPROFILE", home.clone()),
        ("XDG_CONFIG_HOME", home),
        ("GIT_CONFIG_GLOBAL", null.to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_AUTHOR_NAME", "Ann Dev".to_string()),
        ("GIT_AUTHOR_EMAIL", "ann@example.com".to_string()),
        ("GIT_COMMITTER_NAME", "Ann Dev".to_string()),
        ("GIT_COMMITTER_EMAIL", "ann@example.com".to_string()),
    ]
}

/// git in a canonical root: a commit, then a changed file and a new one with a space in its name (each named by
/// confinement), read back through status, log and blame as the dev-aide parses them.
#[test]
fn git_runs_in_a_canonical_root_and_its_output_parses() {
    let git = Git {
        command: "git".to_string(),
        read_timeout: Duration::from_secs(15),
        network_timeout: Duration::from_secs(60),
        write_timeout: Duration::from_secs(30),
    };
    if !git.available() {
        // CI's runners have git: there its absence is a failure, not a pass
        let ci = ["CI", "GITHUB_ACTIONS"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
        assert!(!ci, "no git on the PATH of a CI run");
        eprintln!("git_runs_in_a_canonical_root_and_its_output_parses: no git on the PATH, not run");
        return;
    }
    let base = std::env::temp_dir().join(format!("dev-aide-git-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let _scratch = Scratch(base.clone());
    let home = base.join("home");
    let runtime = base.join("runtime");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(base.join("site")).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    // as a root is: canonical (on Windows the verbatim form)
    let root = base.join("site").canonicalize().unwrap();
    #[cfg(windows)]
    assert!(
        root.to_string_lossy().starts_with(r"\\?\"),
        "a canonical root on Windows is verbatim: {}",
        root.display()
    );
    let env = isolated(&home, &root);
    let run = |mode: Mode, args: &[&str]| {
        git.run(&root, mode, args, &env, None)
            .unwrap()
            .checked(args[0])
            .unwrap_or_else(|err| panic!("git {args:?} in {}: {err}", root.display()))
            .stdout
    };
    let write = |relative: &str, text: &str| {
        let resolved = confine::resolve(&root, &runtime, &[], relative).unwrap();
        assert!(confine::inside(&resolved.path, &root), "{}", resolved.path.display());
        std::fs::create_dir_all(resolved.path.parent().unwrap()).unwrap();
        std::fs::write(&resolved.path, text).unwrap();
    };

    run(Mode::Write, &["init", "-q", "-b", "main"]);
    write(
        "service/A.xml",
        "<services>\n    <service verb=\"get\" noun=\"A\"/>\n</services>\n",
    );
    run(Mode::Write, &["add", "-A"]);
    run(
        Mode::Write,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "Start"],
    );
    write(
        "service/A.xml",
        "<services>\n    <service verb=\"get\" noun=\"B\"/>\n</services>\n",
    );
    write("screen\\New Page.xml", "<screen/>\n");

    let status = parse_status(&run(
        Mode::Read,
        &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"],
    ));
    assert_eq!(status.branch.as_deref(), Some("main"));
    let entries: Vec<(&str, char)> = status
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.letter()))
        .collect();
    assert_eq!(
        entries,
        [("service/A.xml", 'M'), ("screen/New Page.xml", '?')],
        "{status:?}"
    );

    let log = parse_log(&run(Mode::Read, &["log", LOG_FORMAT]));
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(
        (log[0].subject.as_str(), log[0].author_email.as_str()),
        ("Start", "ann@example.com")
    );
    assert_eq!(status.oid.as_deref(), Some(log[0].id.as_str()));

    let blame = parse_blame(&run(
        Mode::Read,
        &["blame", "--porcelain", "HEAD", "--", "service/A.xml"],
    ));
    assert_eq!(blame.len(), 1, "{blame:?}");
    assert_eq!(
        (blame[0].from, blame[0].to, blame[0].author.as_str()),
        (1, 3, "Ann Dev")
    );
}
