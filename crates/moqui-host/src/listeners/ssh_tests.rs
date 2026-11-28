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

use std::collections::HashMap;
use std::time::Duration;

use super::{ending, ssh_overlaps, ssh_wanted, SshBind};
use crate::listeners::{StopHow, Wanted};
use crate::ssh::intake::Ending;
use crate::ssh::live::Access;

fn wanted(given: &[(&str, &str)]) -> Wanted<SshBind, crate::ssh::live::SshLive> {
    let conf: HashMap<String, String> = given
        .iter()
        .map(|(selector, value)| ((*selector).to_string(), (*value).to_string()))
        .collect();
    ssh_wanted(&|selector: &str| conf.get(selector).cloned())
}

fn bind(host: &str, port: u16) -> SshBind {
    SshBind {
        host: host.to_string(),
        port,
    }
}

/// Off while the switch is off, whatever the port, or while the port is not one, naming the setting; on, the bind and
/// the settings in place from the same reading, `dav-server@sftp-drives` among them.
#[test]
fn the_switch_and_port_decide_and_the_rest_applies_in_place() {
    assert_eq!(
        wanted(&[("ssh-server@port", "2222")]),
        Wanted::Off("ssh-server@enabled is false".to_string())
    );
    let Wanted::Off(reason) = wanted(&[("ssh-server@enabled", "true"), ("ssh-server@port", "")]) else {
        panic!("an empty port is not a bind");
    };
    assert!(
        reason.contains("ssh-server@port") && reason.contains("ssh-server@enabled"),
        "{reason}"
    );
    let on = wanted(&[
        ("ssh-server@enabled", " TRUE "),
        ("ssh-server@port", "2223"),
        ("ssh-server@host", "0.0.0.0"),
        ("ssh-server@access", "key"),
        ("dav-server@sftp-drives", "false"),
    ]);
    let Wanted::On { bind: at, in_place } = on else {
        panic!("on: {on:?}");
    };
    assert_eq!(at, bind("0.0.0.0", 2223));
    assert_eq!(in_place.access, Access::Key);
    assert!(!in_place.sftp_drives);
    let Wanted::On { bind: default_host, .. } = wanted(&[("ssh-server@enabled", "true"), ("ssh-server@port", "0")])
    else {
        panic!("on with port 0");
    };
    assert_eq!(default_host, bind("127.0.0.1", 0));
}

/// A new bind collides with the old on the same port at the same address, or with either on every address, or a host
/// that is a name; a port of the kernel's choosing never does.
#[test]
fn a_bind_overlaps_on_its_port_at_one_address() {
    assert!(ssh_overlaps(&bind("127.0.0.1", 2222), &bind("127.0.0.1", 2222)));
    assert!(ssh_overlaps(&bind("0.0.0.0", 2222), &bind("127.0.0.1", 2222)));
    assert!(ssh_overlaps(&bind("127.0.0.1", 2222), &bind("::", 2222)));
    assert!(ssh_overlaps(&bind("localhost", 2222), &bind("127.0.0.1", 2222)));
    assert!(!ssh_overlaps(&bind("127.0.0.1", 2222), &bind("127.0.0.2", 2222)));
    assert!(!ssh_overlaps(&bind("127.0.0.1", 2222), &bind("127.0.0.1", 2223)));
    assert!(!ssh_overlaps(&bind("127.0.0.1", 0), &bind("127.0.0.1", 0)));
}

/// The process stop ends the sessions at once, as before the registry; a live stop gives its grace.
#[test]
fn the_process_stop_ends_the_sessions_at_once() {
    assert_eq!(ending(StopHow::Process), Ending::Now);
    assert_eq!(ending(StopHow::Now), Ending::Now);
    let grace = Duration::from_secs(5);
    assert_eq!(ending(StopHow::Grace(grace)), Ending::After(grace));
}
