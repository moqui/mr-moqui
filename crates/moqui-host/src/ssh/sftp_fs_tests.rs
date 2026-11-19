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

use std::sync::Weak;

use russh_sftp::protocol::{FileAttributes, OpenFlags};
use russh_sftp::server::Handler as _;

use super::{normalize, Backend, Held, Open, SftpFs, MAX_HANDLES};

fn message(reply: &russh_sftp::server::StatusReply) -> &str {
    reply.error_message.as_deref().unwrap_or("")
}

fn root_channel(user: &str, max_file: usize) -> SftpFs {
    SftpFs::new(
        Weak::new(),
        Backend::Root("runtime://sftp".to_string()),
        max_file,
        Held::new(user),
    )
}

#[tokio::test]
async fn open_files_and_handles_are_capped_per_channel() {
    let mut fs = root_channel("capped-channel", 100);
    let (location, bytes) = ("runtime://sftp/a".to_string(), vec![0; 60]);
    fs.handles.insert("h1".to_string(), Open::Write { location, bytes });
    let (location, bytes) = ("runtime://sftp/b".to_string(), Vec::new());
    fs.handles.insert("h2".to_string(), Open::Write { location, bytes });
    fs.held.add(60);
    fs.write(1, "h2".to_string(), 0, vec![1; 40]).await.unwrap();
    assert_eq!(fs.held.mine, 100);
    let refused = fs.write(2, "h2".to_string(), 40, vec![1]).await.unwrap_err();
    assert!(message(&refused).contains("would hold more than"), "{refused:?}");
    // Writing over bytes already held needs no room.
    fs.write(3, "h1".to_string(), 0, vec![2; 60]).await.unwrap();
    fs.close(4, "h1".to_string()).await.unwrap_err();
    assert_eq!(fs.held.mine, 40);

    for index in fs.handles.len()..MAX_HANDLES {
        fs.handles.insert(format!("d{index}"), Open::Dir { entries: None });
    }
    let refused = fs
        .open(5, "/c".to_string(), OpenFlags::READ, FileAttributes::empty())
        .await
        .unwrap_err();
    assert!(message(&refused).contains("handles are open"), "{refused:?}");
    let refused = fs.opendir(6, "/".to_string()).await.unwrap_err();
    assert!(message(&refused).contains("handles are open"), "{refused:?}");
}

/// A file open for reading is read from its source a window at a time: the channel holds a window, never the
/// file, and the bytes come back whole and in order (HL-010).
#[tokio::test]
async fn a_read_handle_holds_a_window_of_its_source() {
    use moqui_core::resource::source::{MemorySource, READ_WINDOW};
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let factory = std::sync::Arc::clone(context.factory());
    factory.request_pool().start(1, 4).unwrap();
    let mut fs = SftpFs::new(
        std::sync::Arc::downgrade(&factory),
        Backend::Root("runtime://sftp".to_string()),
        usize::MAX,
        Held::new("windowed-reader"),
    );
    let content: Vec<u8> = (0..READ_WINDOW * 5 / 2).map(|at| (at % 251) as u8).collect();
    fs.handles.insert(
        "r".to_string(),
        Open::Read {
            size: content.len() as u64,
            source: std::sync::Arc::new(MemorySource::new(content.clone())),
            window_at: 0,
            window: Vec::new(),
        },
    );
    let mut read = Vec::new();
    loop {
        match fs.read(1, "r".to_string(), read.len() as u64, 32 * 1024).await {
            Ok(data) => {
                assert!(!data.data.is_empty());
                read.extend_from_slice(&data.data);
                assert!(fs.held.mine <= READ_WINDOW, "{}", fs.held.mine);
            }
            Err(reply) => {
                assert_eq!(reply.status_code, russh_sftp::protocol::StatusCode::Eof);
                break;
            }
        }
    }
    assert!(read == content, "the bytes read are the file's");
    fs.close(2, "r".to_string()).await.unwrap();
    assert_eq!(fs.held.mine, 0);
}

/// SEC-FIL-84: a read asking for far more than a window gets at most READ_WINDOW bytes, and the handle holds at
/// most a window; a window the user's room does not hold is refused, and one that fits is read.
#[tokio::test]
async fn a_read_window_is_at_most_its_size_and_within_the_users_room() {
    use moqui_core::resource::source::{MemorySource, READ_WINDOW};
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let factory = std::sync::Arc::clone(context.factory());
    factory.request_pool().start(1, 4).unwrap();
    let content: Vec<u8> = (0..READ_WINDOW * 3).map(|at| (at % 251) as u8).collect();
    let reader = |fs: &mut SftpFs| {
        fs.handles.insert(
            "r".to_string(),
            Open::Read {
                size: content.len() as u64,
                source: std::sync::Arc::new(MemorySource::new(content.clone())),
                window_at: 0,
                window: Vec::new(),
            },
        );
    };
    let mut fs = SftpFs::new(
        std::sync::Arc::downgrade(&factory),
        Backend::Root("runtime://sftp".to_string()),
        READ_WINDOW,
        Held::new("large-reader"),
    );
    reader(&mut fs);
    let data = fs.read(1, "r".to_string(), 0, u32::MAX).await.unwrap();
    assert_eq!(data.data.len(), READ_WINDOW, "a short read, not the whole file");
    assert_eq!(data.data[..], content[..READ_WINDOW]);
    assert_eq!(fs.held.mine, READ_WINDOW);
    let data = fs.read(2, "r".to_string(), READ_WINDOW as u64, u32::MAX).await.unwrap();
    assert_eq!(data.data[..], content[READ_WINDOW..READ_WINDOW * 2]);
    assert_eq!(fs.held.mine, READ_WINDOW, "the new window took the old one's place");

    // the user's room is one byte short of a window: refused
    let mut short = SftpFs::new(
        std::sync::Arc::downgrade(&factory),
        Backend::Root("runtime://sftp".to_string()),
        READ_WINDOW - 1,
        Held::new("short-reader"),
    );
    reader(&mut short);
    let refused = short.read(3, "r".to_string(), 0, 1024).await.unwrap_err();
    assert!(message(&refused).contains("would hold more than"), "{refused:?}");
    assert_eq!(short.held.mine, 0);
}

/// SEC-FIL-61: the bytes open files hold are counted per user over all of the user's channels, so a second channel
/// has only what the first left, and gets it back when the first channel ends.
#[tokio::test]
async fn the_bytes_held_are_capped_per_user_over_every_channel() {
    let mut first = root_channel("many-channels", 100);
    let mut second = root_channel("many-channels", 100);
    let mut other = root_channel("another-user", 100);
    for fs in [&mut first, &mut second, &mut other] {
        let (location, bytes) = ("runtime://sftp/a".to_string(), Vec::new());
        fs.handles.insert("h".to_string(), Open::Write { location, bytes });
    }
    first.write(1, "h".to_string(), 0, vec![1; 70]).await.unwrap();
    let refused = second.write(2, "h".to_string(), 0, vec![1; 40]).await.unwrap_err();
    assert!(message(&refused).contains("would hold more than"), "{refused:?}");
    second.write(3, "h".to_string(), 0, vec![1; 30]).await.unwrap();
    // another user is not held back
    other.write(4, "h".to_string(), 0, vec![1; 100]).await.unwrap();
    drop(first);
    second.write(5, "h".to_string(), 30, vec![1; 70]).await.unwrap();
    assert_eq!(second.held.total(), 100);
}

#[test]
fn paths_stay_under_the_root() {
    assert_eq!(normalize("/").as_deref(), Some("/"));
    assert_eq!(normalize(".").as_deref(), Some("/"));
    assert_eq!(normalize("a/./b//c/").as_deref(), Some("/a/b/c"));
    assert_eq!(normalize("/a/b/../c").as_deref(), Some("/a/c"));
    assert_eq!(normalize("/a/../.."), None);
    assert_eq!(normalize("../etc/passwd"), None);
}
