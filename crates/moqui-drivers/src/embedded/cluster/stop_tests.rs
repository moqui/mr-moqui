/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the public
 * domain worldwide. This software is distributed without any warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Ctrl-C must not reach a process started the way `pg_ctl stop` is.

use std::process::{Command, Stdio};

/// Ctrl-C (`0`) to the process group `MOQUI_CTRL_PID`, which is that process's id when it was started in a new group.
/// The console's own group is not signalled, so the suite keeps running.
const HELPER: &str = r#"
$target = [uint32]$env:MOQUI_CTRL_PID
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public class MoquiCtrl {
  [DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(IntPtr HandlerRoutine, bool Add);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GenerateConsoleCtrlEvent(uint dwCtrlEvent, uint dwProcessGroupId);
}
'@
[MoquiCtrl]::SetConsoleCtrlHandler([IntPtr]::Zero, $true) | Out-Null
if (-not [MoquiCtrl]::GenerateConsoleCtrlEvent(0, $target)) {
  Write-Error ("GenerateConsoleCtrlEvent failed: " + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
  exit 3
}
Start-Sleep -Milliseconds 400
exit 0
"#;

#[test]
fn ctrl_c_does_not_stop_a_shielded_process() {
    let mut command = Command::new("ping");
    command
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The same flags `pg_ctl stop` uses. No new console: a new console enables Ctrl-C again.
    super::shield_from_ctrl_c(&mut command);
    let mut sleeper = command.spawn().expect("ping");
    let pid = sleeper.id();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        sleeper.try_wait().unwrap().is_none(),
        "the sleeper exited before Ctrl-C"
    );

    let helper = Command::new("powershell")
        .args(["-NoProfile", "-Command", HELPER])
        .env("MOQUI_CTRL_PID", pid.to_string())
        .stdin(Stdio::null())
        .output()
        .expect("powershell");
    let ended = sleeper.try_wait().unwrap();
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    assert!(
        helper.status.success(),
        "Ctrl-C was not sent to the shielded process group: {}\n{}",
        helper.status,
        String::from_utf8_lossy(&helper.stderr)
    );
    assert!(
        ended.is_none(),
        "Ctrl-C stopped the shielded process: {}",
        ended.map(|status| status.to_string()).unwrap_or_default()
    );
}
