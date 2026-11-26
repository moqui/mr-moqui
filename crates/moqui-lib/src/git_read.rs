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

//! A read-only `git` command kept from running anything a repository's config names: `status`, `show`, `diff`,
//! `ls-files`, `log`, `blame` and the like, which the dev tools run in repositories they did not make (an upstream
//! component, an agent's worktree). A partial clone's missing object is fetched lazily from its promisor remote, whose
//! URL the repository's config gives (`ext::sh -c ...` runs a command); `core.fsmonitor` and hooks run commands too.
//! A read never fetches, so no transport is allowed it ([`harden`]); [`isolate`] drops the inherited repository
//! variables too (`GIT_DIR` of a hook the process was started from). Not for fetch, pull, push or clone, which reach a
//! remote on purpose.

use std::process::Command;

/// The transports git knows, each refused by name too, as a repository's `protocol.<name>.allow` outranks
/// `protocol.allow`.
const PROTOCOLS: [&str; 7] = ["ext", "fd", "file", "git", "ssh", "http", "https"];

/// The variables that would point git at another repository than the one it runs in.
const REPOSITORY_VARIABLES: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// The `-c` arguments of a read, to go before its subcommand: no fsmonitor, no hooks, no transport.
#[must_use]
pub fn read_config() -> Vec<String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut args = vec![
        "-c".to_string(),
        "core.fsmonitor=false".to_string(),
        "-c".to_string(),
        format!("core.hooksPath={null}"),
        "-c".to_string(),
        "protocol.allow=never".to_string(),
    ];
    for protocol in PROTOCOLS {
        args.push("-c".to_string());
        args.push(format!("protocol.{protocol}.allow=never"));
    }
    args
}

/// Make `command` (a `git` with no arguments yet) a read: [`read_config`] and no lazy fetch (`GIT_NO_LAZY_FETCH`, git
/// 2.44; the refused transports hold for an older git). The inherited repository variables stay, for a read that runs
/// as a git hook (`moqui check` in a pre-commit, whose `GIT_INDEX_FILE` is the commit's). Arguments and variables the
/// caller adds after it still apply.
pub fn harden(command: &mut Command) {
    command.args(read_config()).env("GIT_NO_LAZY_FETCH", "1");
}

/// [`harden`], and the inherited repository variables dropped: a read of the repository the command runs in (its
/// current directory), whatever repository started this process.
pub fn isolate(command: &mut Command) {
    harden(command);
    for name in REPOSITORY_VARIABLES {
        command.env_remove(name);
    }
}

#[cfg(test)]
#[path = "git_read_tests.rs"]
mod tests;
