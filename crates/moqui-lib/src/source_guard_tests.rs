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

#[test]
fn a_flood_from_one_source_is_held_to_its_rate_while_another_goes_on() {
    let mut gate = RequestGate::new(RequestLimits {
        rate: 2.0,
        burst: 5,
        max_transactions: 0,
        max_connections: 0,
        max_connections_per_source: 0,
    });
    let t0 = Instant::now();
    let flood: IpAddr = "203.0.113.9".parse().unwrap();
    let other: IpAddr = "198.51.100.4".parse().unwrap();
    let passed = (0..50).filter(|_| gate.admit(flood, 0, t0) == Admit::Pass).count();
    assert_eq!(passed, 5, "the burst, then nothing");
    assert_eq!(gate.admit(flood, 0, t0), Admit::RateLimited);
    assert_eq!(gate.admit(other, 0, t0), Admit::Pass, "another source is not held");
    // the same /64 is the same source
    let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
    let b: IpAddr = "2001:db8:1:2::2".parse().unwrap();
    for _ in 0..5 {
        assert_eq!(gate.admit(a, 0, t0), Admit::Pass);
    }
    assert_eq!(gate.admit(b, 0, t0), Admit::RateLimited);
    // the rate refills: one second is two more
    let t1 = t0 + Duration::from_secs(1);
    assert_eq!(gate.admit(flood, 0, t1), Admit::Pass);
    assert_eq!(gate.admit(flood, 0, t1), Admit::Pass);
    assert_eq!(gate.admit(flood, 0, t1), Admit::RateLimited);
}

#[test]
fn the_transaction_and_connection_caps() {
    let mut gate = RequestGate::new(RequestLimits {
        rate: 0.0,
        burst: 0,
        max_transactions: 10,
        max_connections: 2,
        max_connections_per_source: 0,
    });
    let t0 = Instant::now();
    let source: IpAddr = "198.51.100.4".parse().unwrap();
    assert_eq!(gate.admit(source, 9, t0), Admit::Pass);
    assert_eq!(gate.admit(source, 10, t0), Admit::Busy);
    assert!(gate.connection_opened(None));
    assert!(gate.connection_opened(None));
    assert!(!gate.connection_opened(None), "the third is closed");
    gate.connection_closed(None);
    assert!(
        !gate.connection_opened(None),
        "still two open with the one past the cap counted until its close"
    );
    gate.connection_closed(None);
    gate.connection_closed(None);
    assert!(gate.connection_opened(None));
    assert_eq!(RequestLimits::default().max_connections, 500);
}

#[test]
fn one_source_cannot_hold_every_connection() {
    let mut gate = RequestGate::new(RequestLimits {
        max_connections: 10,
        max_connections_per_source: 2,
        ..Default::default()
    });
    let flood: IpAddr = "203.0.113.9".parse().unwrap();
    let office: IpAddr = "198.51.100.4".parse().unwrap();
    assert!(gate.connection_opened(Some(flood)));
    assert!(gate.connection_opened(Some(flood)));
    assert!(
        !gate.connection_opened(Some(flood)),
        "the third from one source is closed"
    );
    assert!(gate.connection_opened(Some(office)), "another source still connects");
    // the one past the cap closes, then one of the two: the source may open one again
    gate.connection_closed(Some(flood));
    gate.connection_closed(Some(flood));
    assert!(gate.connection_opened(Some(flood)));
    // the same /64 is the same source
    let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
    let b: IpAddr = "2001:db8:1:2::2".parse().unwrap();
    assert!(gate.connection_opened(Some(a)));
    assert!(gate.connection_opened(Some(b)));
    assert!(!gate.connection_opened(Some(a)));
}

#[test]
fn failures_block_a_source_for_a_while_and_not_another() {
    let policy = GuardPolicy {
        limit: 3,
        window: Duration::from_secs(60),
        block: Duration::from_secs(600),
    };
    let mut guard = SourceGuard::new(policy);
    let t0 = Instant::now();
    let scanner: IpAddr = "203.0.113.9".parse().unwrap();
    let trunk: IpAddr = "198.51.100.4".parse().unwrap();
    assert_eq!(guard.failure(scanner, t0), Failure::Counted(1));
    assert_eq!(guard.failure(scanner, t0 + Duration::from_secs(1)), Failure::Counted(2));
    assert_eq!(guard.failure(trunk, t0), Failure::Counted(1));
    assert_eq!(guard.failure(scanner, t0 + Duration::from_secs(2)), Failure::Blocked);
    assert!(guard.is_blocked(scanner, t0 + Duration::from_secs(3)));
    assert_eq!(
        guard.failure(scanner, t0 + Duration::from_secs(4)),
        Failure::StillBlocked
    );
    assert!(
        !guard.is_blocked(trunk, t0 + Duration::from_secs(3)),
        "another source goes on"
    );
    guard.success(trunk);
    assert_eq!(guard.failure(trunk, t0 + Duration::from_secs(5)), Failure::Counted(1));
    // the block ends
    assert!(!guard.is_blocked(scanner, t0 + Duration::from_secs(603)));
    // failures spread past the window do not add up
    assert_eq!(guard.failure(trunk, t0 + Duration::from_secs(120)), Failure::Counted(1));
    // a block from the services, and clearing it
    guard.set_block(trunk, Some(t0 + Duration::from_secs(900)));
    assert!(guard.is_blocked(trunk, t0 + Duration::from_secs(130)));
    guard.set_block(trunk, None);
    assert!(!guard.is_blocked(trunk, t0 + Duration::from_secs(130)));
}

#[test]
fn an_ipv6_source_counts_as_its_64_and_a_full_table_keeps_its_blocks() {
    let policy = GuardPolicy {
        limit: 2,
        window: Duration::from_secs(60),
        block: Duration::from_secs(600),
    };
    let mut guard = SourceGuard::new(policy);
    let t0 = Instant::now();
    let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
    let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
    let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
    assert_eq!(guard.failure(a, t0), Failure::Counted(1));
    assert_eq!(guard.failure(b, t0), Failure::Blocked, "the same /64");
    assert!(!guard.is_blocked(other, t0));
    let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
    let v4: IpAddr = "203.0.113.9".parse().unwrap();
    guard.failure(mapped, t0);
    assert_eq!(
        guard.failure(v4, t0),
        Failure::Blocked,
        "an IPv4-mapped address is its IPv4 one"
    );
    // a scanner rotating through more sources than the table holds does not clear the blocks
    let mut blocked = 0u32;
    for i in 0..(MAX_SOURCES as u32 + 10) {
        let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
        guard.failure(ip, t0);
        if i < 5 {
            blocked += u32::from(guard.failure(ip, t0) == Failure::Blocked);
        }
    }
    assert_eq!(blocked, 5);
    assert!(guard.is_blocked(a, t0 + Duration::from_secs(1)));
    assert!(guard.entries.len() <= MAX_SOURCES);
}

/// A source holds at most its share of the slots, an IPv6 /64 and an IPv4-mapped address counted as one source, and
/// a dropped slot is given back (HL-032).
#[test]
fn slots_keep_a_share_per_source() {
    let slots = SourceSlots::new(4, 2);
    let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
    let a2: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
    let v4: IpAddr = "203.0.113.9".parse().unwrap();
    let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
    let first = slots.take(a).unwrap();
    let _second = slots.take(a2).unwrap();
    assert_eq!(slots.take(a).err(), Some(SlotRefused::Source), "the same /64");
    let _third = slots.take(v4).unwrap();
    let _fourth = slots.take(mapped).unwrap();
    assert_eq!(slots.take(mapped).err(), Some(SlotRefused::Full));
    assert_eq!(slots.held_by(v4), 2);
    drop(first);
    assert_eq!(slots.held(), 3);
    assert!(slots.take(a).is_ok());
    assert_eq!(slots.held(), 3, "the slot taken above was dropped again");
}
