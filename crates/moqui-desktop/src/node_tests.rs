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

use super::{choose_port, save_port, saved_port};
use std::net::{Ipv4Addr, TcpListener};

#[test]
fn the_saved_port_is_kept_while_free_and_replaced_when_taken() {
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(choose_port(Some(free)).unwrap(), free);
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    assert_ne!(choose_port(Some(port)).unwrap(), port);
    assert_ne!(choose_port(None).unwrap(), 0);
}

#[test]
fn the_port_file_round_trips_and_a_bad_one_is_none() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-port-{}", std::process::id()));
    let path = dir.join("node-port");
    assert_eq!(saved_port(&path), None);
    save_port(&path, 51234);
    assert_eq!(saved_port(&path), Some(51234));
    std::fs::write(&path, "not a port").unwrap();
    assert_eq!(saved_port(&path), None);
    std::fs::remove_dir_all(&dir).unwrap();
}
