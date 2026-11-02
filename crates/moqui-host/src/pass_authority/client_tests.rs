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
fn an_endpoint_is_a_grpc_url_or_the_edge_link() {
    assert_eq!(parse_endpoint("edge:").unwrap(), Endpoint::Edge);
    assert_eq!(
        parse_endpoint("grpcs://authority.example.com:4443").unwrap(),
        Endpoint::Direct {
            host: "authority.example.com".to_string(),
            port: 4443,
            tls: true
        }
    );
    assert_eq!(
        parse_endpoint(" grpc://127.0.0.1/ ").unwrap(),
        Endpoint::Direct {
            host: "127.0.0.1".to_string(),
            port: 4317,
            tls: false
        }
    );
    assert_eq!(
        parse_endpoint("grpc://[::1]:9000").unwrap(),
        Endpoint::Direct {
            host: "::1".to_string(),
            port: 9000,
            tls: false
        }
    );
    for bad in ["https://authority.example.com", "edge", "grpc://", ""] {
        assert!(parse_endpoint(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_download_reads_its_chunks_in_order_and_ends_or_fails_with_the_stream() {
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    sender.try_send(Ok(Bytes::from_static(b"hello "))).unwrap();
    sender.try_send(Ok(Bytes::from_static(b"world"))).unwrap();
    drop(sender);
    let mut reader = ChannelReader {
        receiver,
        current: Bytes::new(),
    };
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "hello world");

    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    sender.try_send(Ok(Bytes::from_static(b"part"))).unwrap();
    sender
        .try_send(Err("the PASS authority's download stalled".to_string()))
        .unwrap();
    let mut reader = ChannelReader {
        receiver,
        current: Bytes::new(),
    };
    let mut bytes = Vec::new();
    let err = reader.read_to_end(&mut bytes).unwrap_err();
    assert!(err.to_string().contains("stalled"), "{err}");
    assert_eq!(bytes, b"part");
}
