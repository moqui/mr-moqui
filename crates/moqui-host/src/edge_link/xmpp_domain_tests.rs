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

use tokio::io::AsyncWriteExt as _;

use super::*;

const HEADER: &str = "<?xml version='1.0'?><stream:stream xmlns='jabber:client' \
    xmlns:stream='http://etherx.jabber.org/streams' to='Chat.One.Example.' version='1.0'>";

#[test]
fn the_stream_header_names_its_domain() {
    assert_eq!(
        stream_to(HEADER.as_bytes()).unwrap().unwrap().as_deref(),
        Some("chat.one.example")
    );
    // in parts: nothing until the header's end has come
    for cut in [0, 5, 30, HEADER.len() - 1] {
        assert!(stream_to(&HEADER.as_bytes()[..cut]).is_none(), "{cut}");
    }
    let quoted = r#"<stream:stream from="ann@x.example" to="two.example" version="1.0" a='>'>"#;
    assert_eq!(
        stream_to(quoted.as_bytes()).unwrap().unwrap().as_deref(),
        Some("two.example")
    );
    let other_prefix = "<s:stream xmlns:s='http://etherx.jabber.org/streams' to='three.example'>";
    assert_eq!(
        stream_to(other_prefix.as_bytes()).unwrap().unwrap().as_deref(),
        Some("three.example")
    );
    assert_eq!(
        stream_to(b"<stream:stream version='1.0'>").unwrap().unwrap(),
        None,
        "no to"
    );
    assert_eq!(stream_to(b"<stream:stream xto='a.example'>").unwrap().unwrap(), None);
    assert_eq!(stream_to(b"<stream:stream to='not a host'>").unwrap().unwrap(), None);
    assert!(stream_to(b"GET / HTTP/1.1\r\n").unwrap().is_err(), "not a stream");
    assert!(
        stream_to(b"<message to='a.example'>").unwrap().is_err(),
        "not a stream header"
    );
}

#[tokio::test]
async fn what_was_read_is_read_again_first() {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(HEADER.as_bytes()).await.unwrap();
    client.write_all(b"<starttls/>").await.unwrap();
    let mut server = server;
    let (domain, seen) = sniff(&mut server, false).await.unwrap();
    assert_eq!(domain.as_deref(), Some("chat.one.example"));
    let mut whole = Prefixed::new(seen, server);
    let mut read = vec![0_u8; HEADER.len() + 11];
    whole.read_exact(&mut read).await.unwrap();
    assert_eq!(String::from_utf8(read).unwrap(), format!("{HEADER}<starttls/>"));
}

#[tokio::test]
async fn a_client_hello_names_its_server() {
    let (client, mut server) = tokio::io::duplex(16 * 1024);
    let roots = rustls::RootCertStore::empty();
    let config = std::sync::Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let connector = tokio_rustls::TlsConnector::from(config);
    let name = rustls::pki_types::ServerName::try_from("Two.Example").unwrap();
    let connecting = tokio::spawn(async move { connector.connect(name, client).await });
    let (domain, seen) = sniff(&mut server, true).await.unwrap();
    assert_eq!(domain.as_deref(), Some("two.example"));
    assert!(seen.first() == Some(&0x16), "the ClientHello's bytes are kept");
    drop(server);
    let _ = connecting.await;
}
