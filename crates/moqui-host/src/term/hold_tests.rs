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

use std::path::Path;

use super::{read_line, socket_path, write_line, PROTOCOL};

#[tokio::test]
async fn a_line_of_json_is_read_without_what_follows_it() {
    let (mut client, mut server) = tokio::io::duplex(1024);
    write_line(&mut client, &serde_json::json!({"op": "attach", "id": "s1"}))
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut client, b"raw bytes")
        .await
        .unwrap();
    let line = read_line(&mut server).await.unwrap();
    assert_eq!(line["id"], "s1");
    let mut rest = [0u8; 9];
    tokio::io::AsyncReadExt::read_exact(&mut server, &mut rest)
        .await
        .unwrap();
    assert_eq!(&rest, b"raw bytes", "the stream after the line is left as it was");
}

#[test]
fn each_protocol_version_has_its_own_socket() {
    let dir = Path::new("/srv/runtime/tmp/term-hold");
    assert!(socket_path(dir, PROTOCOL).ends_with(format!("hold-v{PROTOCOL}.sock")));
    assert_ne!(socket_path(dir, PROTOCOL), socket_path(dir, PROTOCOL + 1));
}
