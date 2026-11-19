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

//! The requester address of a client spec (docs/Build-Test.md, "Client specs"): a server started by `moqui test
//! --serve` takes a request's `X-Moqui-Test-Peer` header, an address in 127.0.0.0/8, as the request's peer when the
//! request comes from loopback (its TCP peer and the client address a believed proxy named, both), so `moqui test-client` can give each spec its own address and a limit keyed by the
//! requester (an access-token lookup's, a sign-in's) counts per spec, not for the whole suite from 127.0.0.1.
//!
//! Only that process honours it ([`allow`], which nothing but `moqui test --serve` calls; no property or conf turns it
//! on), only from a loopback peer, and only for a loopback address: a client can name no address it is not already
//! connecting from in effect, and a server of `moqui run` reads no such header at all
//! (docs/security/Attack-Surface-Web.md).

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};

/// The header a client spec's requests carry.
pub(crate) const HEADER: &str = "x-moqui-test-peer";

static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Honour [`HEADER`] in this process from now on: `moqui test --serve` alone, before its server starts (which embedded
/// Postgres builds, `test_cmd/serve.rs`).
#[cfg(feature = "embedded-postgres")]
pub(crate) fn allow() {
    ALLOWED.store(true, Ordering::Relaxed);
}

/// The address a request from TCP peer `remote`, client `client`, names in [`HEADER`] (`value`), when this process
/// honours it, both are loopback (a request a local proxy forwards for a remote client is not) and the value is an IPv4
/// address in 127.0.0.0/8.
pub(crate) fn named(remote: IpAddr, client: IpAddr, value: Option<&str>) -> Option<IpAddr> {
    if !ALLOWED.load(Ordering::Relaxed) {
        return None;
    }
    resolve(remote, client, value)
}

fn resolve(remote: IpAddr, client: IpAddr, value: Option<&str>) -> Option<IpAddr> {
    if !remote.to_canonical().is_loopback() || !client.to_canonical().is_loopback() {
        return None;
    }
    let address = value?.trim().parse::<std::net::Ipv4Addr>().ok()?;
    address.is_loopback().then_some(IpAddr::V4(address))
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn only_a_loopback_peer_names_only_a_loopback_address() {
        let ip = |text: &str| text.parse().unwrap();
        let local = ip("127.0.0.1");
        assert_eq!(resolve(local, local, Some("127.1.0.7")), Some(ip("127.1.0.7")));
        assert_eq!(
            resolve(ip("::1"), ip("::1"), Some(" 127.1.2.3 ")),
            Some(ip("127.1.2.3"))
        );
        assert_eq!(resolve(ip("10.0.0.5"), ip("10.0.0.5"), Some("127.1.0.7")), None);
        // a local proxy forwarding a remote client's request
        assert_eq!(resolve(local, ip("203.0.113.5"), Some("127.1.0.7")), None);
        assert_eq!(resolve(local, local, Some("10.0.0.5")), None);
        assert_eq!(resolve(local, local, Some("::1")), None);
        assert_eq!(resolve(local, local, Some("not an address")), None);
        assert_eq!(resolve(local, local, None), None);
    }
}
