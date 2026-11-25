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

//! A `tasklist` probe that cannot tell counts as live, so a sweep does not stop another run.

use super::tasklist_probe_live;

#[test]
fn a_tasklist_probe_that_cannot_tell_counts_as_live() {
    let pid = 4242;
    assert!(tasklist_probe_live(false, false, "", pid), "tasklist did not start");
    assert!(
        tasklist_probe_live(true, false, "ERROR: The RPC server is unavailable.", pid),
        "tasklist exited without success"
    );
    assert!(
        tasklist_probe_live(true, false, "ERROR: Invalid argument PID eq 4242", pid),
        "a failed probe that echoes the pid is still not a listing"
    );
    let listed = "\"postgres.exe\",\"4242\",\"Console\",\"1\",\"12,345 K\"\r\n";
    assert!(
        tasklist_probe_live(true, true, listed, pid),
        "the listing names the pid"
    );
    assert!(
        !tasklist_probe_live(
            true,
            true,
            "INFO: No tasks are running which match the specified criteria.\r\n",
            pid
        ),
        "the English absence line"
    );
    assert!(
        !tasklist_probe_live(true, true, "INFO: 没有运行的任务匹配指定标准。\r\n", pid),
        "a successful listing that does not name the pid"
    );
    assert!(
        !tasklist_probe_live(
            true,
            true,
            "INFO: No tasks are running which match the specified criteria. PID eq 4242\r\n",
            pid
        ),
        "an absence line that echoes the pid"
    );
}
