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

use super::*;

fn required(listener: &'static str, property: &'static str, port: u16) -> Target {
    Target {
        required: true,
        ..Target::optional(listener, property, "127.0.0.1", port, Proto::Tcp)
    }
}

#[test]
fn a_held_required_port_is_named_and_a_free_one_passes() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    let free_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let targets = [
        required("HTTP", "webapp_http_port", held_port),
        required("SSH", "ssh_port", free_port),
    ];
    let err = check(&targets).unwrap_err().to_string();
    assert!(
        err.contains(&format!("{held_port} (webapp_http_port) on 127.0.0.1")),
        "{err}"
    );
    assert!(!err.contains("ssh_port"), "{err}");
    #[cfg(target_os = "linux")]
    assert!(err.contains(&format!("held by pid {}", std::process::id())), "{err}");
    drop(held);
    // The freed ephemeral port can be taken at once by another listener on a busy machine, so only this process
    // must be gone from it.
    if let Err(err) = check(&targets) {
        assert!(
            !err.to_string().contains(&format!("pid {} ", std::process::id())),
            "{err}"
        );
    }
}

#[test]
fn a_held_optional_port_is_returned_and_does_not_stop_the_start() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    let busy = check(&[Target::optional(
        "SMTP",
        "smtp_listen_port",
        "127.0.0.1",
        held_port,
        Proto::Tcp,
    )])
    .unwrap();
    assert_eq!(busy, vec![("SMTP", held_port)]);
}

#[test]
fn a_held_udp_port_is_found() {
    let held = UdpSocket::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    let busy = check(&[Target::optional(
        "Call media",
        "rtc_media_port",
        "127.0.0.1",
        held_port,
        Proto::Udp,
    )])
    .unwrap();
    assert_eq!(busy, vec![("Call media", held_port)]);
    #[cfg(target_os = "linux")]
    assert!(
        holders(held_port, Proto::Udp)
            .holders
            .iter()
            .any(|holder| holder.pid == std::process::id()),
        "this process holds the UDP port"
    );
}

#[test]
fn port_zero_is_not_checked() {
    assert!(check(&[required("Cluster", "cluster_https_port", 0)])
        .unwrap()
        .is_empty());
}

#[test]
fn busy_names_the_port_of_a_skipped_listener() {
    *lock(&BUSY) = vec![("SIP", 5060)];
    assert_eq!(busy("SIP").as_deref(), Some("port 5060 was busy at the start check"));
    assert_eq!(busy("XMPP"), None);
    lock(&BUSY).clear();
}

#[cfg(target_os = "linux")]
#[test]
fn holders_finds_this_process() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let lookup = holders(held.local_addr().unwrap().port(), Proto::Tcp);
    let me = lookup
        .holders
        .iter()
        .find(|holder| holder.pid == std::process::id())
        .expect("this process holds the port");
    assert!(!me.name.is_empty());
    assert!(me.exe.is_some());
}

#[test]
fn listening_sockets_reads_listen_rows_of_the_port() {
    let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:08AE 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4444 1 0000000000000000 100 0 0 10 0
";
    assert_eq!(
        listening_sockets(text, 8080, Proto::Tcp),
        vec![Socket { uid: 1000, inode: 4242 }]
    );
    assert_eq!(
        listening_sockets(text, 2222, Proto::Tcp),
        vec![Socket { uid: 0, inode: 4444 }]
    );
    let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:20FB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5151 1 0000000000000000 100 0 0 10 0
";
    assert_eq!(
        listening_sockets(v6, 8443, Proto::Tcp),
        vec![Socket { uid: 1000, inode: 5151 }]
    );
    let udp = "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  100: 00000000:13C4 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 6161 2 0000000000000000 0
";
    assert_eq!(
        listening_sockets(udp, 5060, Proto::Udp),
        vec![Socket { uid: 1000, inode: 6161 }]
    );
    assert!(listening_sockets(udp, 5060, Proto::Tcp).is_empty());
}

#[test]
fn lsof_holders_pairs_pid_and_command() {
    let holders = lsof_holders("p123\ncjava\np456\ncnc\n");
    assert_eq!(
        holders,
        vec![
            Holder {
                pid: 123,
                name: "java".to_string(),
                exe: None
            },
            Holder {
                pid: 456,
                name: "nc".to_string(),
                exe: None
            },
        ]
    );
}
