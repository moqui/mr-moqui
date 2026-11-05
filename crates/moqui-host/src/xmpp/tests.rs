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

//! The XMPP connection admission: the per-source cap counts an IPv6 /64 as one source.

use std::net::IpAddr;

use super::{admit, XmppSlot, PER_SOURCE};

/// The 21st connection from one /64 is refused, from any address in it; another /64 and an IPv4 address are let in,
/// and a closed connection gives its place back (HL-032).
#[test]
fn the_per_source_cap_counts_a_64_as_one_source() {
    let slot = XmppSlot::default();
    let held: Vec<_> = (1..=PER_SOURCE)
        .map(|n| {
            let ip: IpAddr = format!("2001:db8:5:6::{n:x}").parse().unwrap();
            admit(&slot.sources, ip).expect("within the share")
        })
        .collect();
    let next: IpAddr = "2001:db8:5:6:ffff:ffff:ffff:fffe".parse().unwrap();
    assert!(admit(&slot.sources, next).is_none(), "the 21st from the /64");
    let other: IpAddr = "2001:db8:5:7::1".parse().unwrap();
    assert!(admit(&slot.sources, other).is_some());
    let v4: IpAddr = "198.51.100.4".parse().unwrap();
    assert!(admit(&slot.sources, v4).is_some());
    drop(held);
    assert!(admit(&slot.sources, next).is_some());
}
