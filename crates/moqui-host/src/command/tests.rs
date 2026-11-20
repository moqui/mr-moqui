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

use super::{interactive, kept_history, with_newlines, MAX_LINE};

/// The kept history holds a call's name and no parameters, and nothing of `js` or a `js` block (SEC-FIL-90).
#[test]
fn the_kept_history_holds_no_parameters_or_code() {
    let typed: Vec<String> = [
        r#"service org.moqui.impl.UserServices.update#Password {"newPassword": "s3cret"}"#,
        "js ec.factory.getConfValue('x')",
        "js",
        "const key = 'sk-live-123';",
        "",
        "entity moqui.security.UserAccount {\"username\": \"ann\"}",
        "status",
        "status",
        "cache clear entity.definition",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        kept_history(&typed),
        [
            "service org.moqui.impl.UserServices.update#Password",
            "entity moqui.security.UserAccount",
            "status",
            "cache clear entity.definition",
        ]
    );
}

#[tokio::test]
async fn an_overlong_shell_line_or_js_block_is_refused() {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let shell = tokio::spawn(async move {
        interactive(
            server,
            &std::sync::Weak::new(),
            "tester",
            "127.0.0.1",
            false,
            Err("no apps"),
            Default::default(),
        )
        .await
    });
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

#[test]
fn shell_output_drops_terminal_controls_but_keeps_tab_and_newlines() {
    // a title change (OSC 0), a clipboard write (OSC 52), a C1 CSI, DEL and a bidirectional override
    let error = "failed: \u{1b}]0;owned\u{7}name\u{1b}]52;c;Zm9v\u{7}\u{9b}2J\u{7f}\u{202e}x\tend\nnext";
    assert_eq!(
        with_newlines(error, "\r\n"),
        "failed: ]0;ownedname]52;c;Zm9v2Jx\tend\r\nnext"
    );
    assert_eq!(with_newlines("a\rb\nc", "\n"), "ab\nc");
}
