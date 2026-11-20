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

use super::{read_datagrams, MAX_DATAGRAMS_PER_TICK};

/// A leg reads at most MAX_DATAGRAMS_PER_TICK datagrams a tick; the rest wait for the next (SEC-COM-86).
#[test]
fn a_tick_reads_at_most_its_datagrams() {
    let leg = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    leg.set_nonblocking(true).unwrap();
    let phone = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    for n in 0..100u8 {
        phone.send_to(&[n], leg.local_addr().unwrap()).unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut buf = [0u8; 64];
    let mut seen = Vec::new();
    let first = read_datagrams(&leg, &mut buf, MAX_DATAGRAMS_PER_TICK, |packet, _| seen.push(packet[0]));
    assert_eq!(first, MAX_DATAGRAMS_PER_TICK);
    let second = read_datagrams(&leg, &mut buf, MAX_DATAGRAMS_PER_TICK, |packet, _| seen.push(packet[0]));
    assert_eq!(second, 100 - MAX_DATAGRAMS_PER_TICK);
    assert_eq!(seen, (0..100u8).collect::<Vec<_>>());
}
