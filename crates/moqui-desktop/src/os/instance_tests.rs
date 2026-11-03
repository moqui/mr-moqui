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

use super::{decode, encode};
use tauri::Url;

fn links() -> Vec<Url> {
    vec![
        Url::parse("mailto:a@example.com?subject=Hi%20there").unwrap(),
        Url::parse("tel:+15551234567").unwrap(),
    ]
}

#[test]
fn a_launch_message_carries_its_links() {
    assert_eq!(decode(&encode(&links())), Some(links()));
    assert_eq!(decode(&encode(&[])), Some(Vec::new()));
    assert_eq!(decode("GET / HTTP/1.1\r\n\r\n"), None);
    assert_eq!(decode(""), None);
}

#[cfg(unix)]
#[test]
fn a_later_launch_hands_its_links_to_the_first_and_a_stale_socket_is_taken() {
    use super::{claim, release, Claim};
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("mqd-si-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("instance.sock");

    // a socket left by an instance that is gone: connecting is refused, so this launch takes it
    drop(std::os::unix::net::UnixListener::bind({
        std::fs::create_dir_all(&dir).unwrap();
        &path
    }));
    assert!(path.exists());
    let Claim::First(listener) = claim(&path, &[]) else {
        panic!("a stale socket is taken");
    };
    let (sender, received) = mpsc::channel();
    let served = listener.serve(move |links| {
        let _ = sender.send(links);
    });
    assert_eq!(served, path);

    assert!(matches!(claim(&path, &links()), Claim::Forwarded));
    assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), links());

    release(&path);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
