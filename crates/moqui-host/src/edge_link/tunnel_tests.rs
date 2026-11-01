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

use tokio::io::AsyncReadExt as _;

use super::*;

#[tokio::test]
async fn a_passthrough_tunnel_starts_with_the_clients_address_then_its_hello() {
    let (client_side, mut client) = tokio::io::duplex(4096);
    let (tunnel_side, mut local) = tokio::io::duplex(4096);
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let browser: SocketAddr = "203.0.113.9:51000".parse().unwrap();
    let edge: SocketAddr = "192.0.2.1:443".parse().unwrap();
    let relayed = tokio::spawn(relay(
        shutdown,
        client_side,
        tunnel_side,
        b"\x16\x03\x01hello",
        browser,
        edge,
    ));
    // what the local node reads: the PROXY header naming the browser, then the ClientHello's bytes
    let header = crate::proxy_header::read(&mut local).await.unwrap();
    assert_eq!(header, Some(browser));
    let mut hello = [0_u8; 8];
    local.read_exact(&mut hello).await.unwrap();
    assert_eq!(&hello, b"\x16\x03\x01hello");
    // then both ways
    client.write_all(b"more").await.unwrap();
    let mut more = [0_u8; 4];
    local.read_exact(&mut more).await.unwrap();
    assert_eq!(&more, b"more");
    local.write_all(b"back").await.unwrap();
    let mut back = [0_u8; 4];
    client.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"back");
    drop((client, local));
    relayed.await.unwrap();
}
