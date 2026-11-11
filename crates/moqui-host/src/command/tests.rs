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

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{interactive, MAX_LINE};

#[tokio::test]
async fn an_overlong_shell_line_or_js_block_is_refused() {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let shell =
        tokio::spawn(
            async move { interactive(server, &std::sync::Weak::new(), "tester", false, Err("no apps")).await },
        );
    let (mut reader, mut writer) = tokio::io::split(client);
    let send = tokio::spawn(async move {
        writer.write_all(&vec![b'a'; MAX_LINE + 10]).await.unwrap();
        writer.write_all(b"\njs\n").await.unwrap();
        let chunk = vec![b'b'; MAX_LINE / 2];
        for _ in 0..3 {
            writer.write_all(&chunk).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
        writer.write_all(b"exit\n").await.unwrap();
        writer
    });
    let mut seen = Vec::new();
    reader.read_to_end(&mut seen).await.unwrap();
    let seen = String::from_utf8_lossy(&seen);
    assert!(seen.contains("error: the line is longer than"), "{seen}");
    assert!(seen.contains("error: the js block is longer than"), "{seen}");
    assert!(seen.ends_with("bye\n"), "{seen}");
    drop(send.await.unwrap());
    assert_eq!(shell.await.unwrap(), 0);
}
