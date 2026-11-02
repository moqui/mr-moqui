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

//! The poll's handling of each file, on a remote and receipts in memory. The service over the SFTP client and the
//! database is `system_sync::tests::a_message_goes_out_by_sftp_and_a_poll_receives_moves_and_skips`.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// Files by path, with the paths whose read or removal fails.
#[derive(Default)]
struct Remote {
    files: BTreeMap<String, String>,
    unreadable: BTreeSet<String>,
    unremovable: BTreeSet<String>,
}

impl RemoteFiles for Remote {
    fn ls_files(&mut self, path: &str) -> Result<Vec<String>, LibError> {
        let prefix = format!("{path}/");
        Ok(self
            .files
            .keys()
            .filter_map(|name| name.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('/'))
            .map(str::to_string)
            .collect())
    }
    fn get_text(&mut self, path: &str, _charset: &str) -> Result<String, LibError> {
        if self.unreadable.contains(path) {
            return Err(LibError::new(format!("read of {path} failed")));
        }
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| LibError::new(format!("No such file: {path}")))
    }
    fn rm(&mut self, path: &str) -> Result<(), LibError> {
        if self.unremovable.contains(path) {
            return Err(LibError::new(format!("remove of {path} failed")));
        }
        self.files.remove(path).map(|_| ()).ok_or_else(|| LibError::new("gone"))
    }
    fn move_file(&mut self, file_path: &str, dest_dir: &str) -> Result<String, LibError> {
        let name = file_path.rsplit('/').next().unwrap_or_default();
        let text = self.files.remove(file_path).ok_or_else(|| LibError::new("gone"))?;
        let to = format!("{dest_dir}/{name}");
        self.files.insert(to.clone(), text);
        Ok(to)
    }
}

/// Messages by ID as (file name, text); a receive of a text with "bad" fails and one with "drop" keeps nothing.
#[derive(Default)]
struct Kept {
    messages: Vec<(String, String)>,
    received: Vec<String>,
}

impl Receipts for Kept {
    fn holding(&mut self, filename: &str, text: &str) -> Result<Vec<String>, LibError> {
        Ok(self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, (name, kept))| name == filename && kept == text)
            .map(|(index, _)| format!("M{index}"))
            .collect())
    }
    fn receive(&mut self, filename: &str, text: String) -> Result<Vec<String>, LibError> {
        self.received.push(filename.to_string());
        if text.contains("bad") {
            return Err(LibError::new("Test receive failed"));
        }
        if text.contains("drop") {
            return Ok(Vec::new());
        }
        self.messages.push((filename.to_string(), text));
        Ok(vec![format!("M{}", self.messages.len() - 1)])
    }
}

fn poll_of<'a>(response: &'a str) -> Poll<'a> {
    Poll {
        type_id: "EdiIn",
        remote_id: "Partner",
        receive_path: "/out",
        move_path: "/out/done",
        pattern: Some(".*\\.x12"),
        response,
        charset: "",
        limit: None,
    }
}

#[test]
fn each_file_is_on_its_own_and_removed_only_after_a_durable_receipt() {
    let mut remote = Remote::default();
    for (name, text) in [
        ("a.x12", "unreadable"),
        ("b.x12", "a bad file"),
        ("c.x12", "good c"),
        ("d.x12", "good d"),
        ("e.x12", "drop me"),
        ("f.txt", "not matched"),
    ] {
        remote.files.insert(format!("/out/{name}"), text.to_string());
    }
    remote.unreadable.insert("/out/a.x12".to_string());
    remote.unremovable.insert("/out/c.x12".to_string());
    let mut kept = Kept::default();
    let poll = poll_of("MsgRrDelete");

    // A failed read, a failed receive, a failed removal and a receive that keeps nothing do not stop the others.
    let polled = poll_each(&mut remote, &mut kept, &poll).unwrap();
    assert_eq!(polled.file_count, 5);
    assert_eq!(polled.failed, ["a.x12", "b.x12", "c.x12", "e.x12"]);
    assert_eq!(kept.received, ["b.x12", "c.x12", "d.x12", "e.x12"]);
    assert_eq!(
        kept.messages,
        [
            ("c.x12".to_string(), "good c".to_string()),
            ("d.x12".to_string(), "good d".to_string())
        ]
    );
    let left: Vec<&str> = remote.files.keys().map(String::as_str).collect();
    assert_eq!(
        left,
        ["/out/a.x12", "/out/b.x12", "/out/c.x12", "/out/e.x12", "/out/f.txt"]
    );

    // The next poll: c was received before its removal failed, so it is removed now and not received again.
    remote.unremovable.clear();
    remote.unreadable.clear();
    kept.received.clear();
    let polled = poll_each(&mut remote, &mut kept, &poll).unwrap();
    assert_eq!(polled.failed, ["b.x12", "e.x12"]);
    assert_eq!(kept.received, ["a.x12", "b.x12", "e.x12"]);
    assert_eq!(kept.messages.len(), 3);
    let left: Vec<&str> = remote.files.keys().map(String::as_str).collect();
    assert_eq!(left, ["/out/b.x12", "/out/e.x12", "/out/f.txt"]);

    // A file of a received name with other text is a new file.
    remote
        .files
        .insert("/out/d.x12".to_string(), "good d, sent again changed".to_string());
    kept.received.clear();
    let polled = poll_each(&mut remote, &mut kept, &poll_of("MsgRrMove")).unwrap();
    assert_eq!(kept.received, ["b.x12", "d.x12", "e.x12"]);
    assert_eq!(polled.failed, ["b.x12", "e.x12"]);
    assert!(remote.files.contains_key("/out/done/d.x12"));
    assert_eq!(kept.messages.len(), 4);
}

#[test]
fn a_file_left_on_the_server_is_not_received_twice_and_a_listing_failure_stops_the_poll() {
    let mut remote = Remote::default();
    remote.files.insert("/out/a.x12".to_string(), "good a".to_string());
    let mut kept = Kept::default();
    // MsgRrNone leaves each file: the second poll finds its text held and does not receive it again.
    for _ in 0..2 {
        let polled = poll_each(&mut remote, &mut kept, &poll_of("MsgRrNone")).unwrap();
        assert!(polled.failed.is_empty());
    }
    assert_eq!(kept.received, ["a.x12"]);
    assert!(remote.files.contains_key("/out/a.x12"));
    // A pattern that is not a regular expression is the poll's error.
    let mut bad = poll_of("MsgRrNone");
    bad.pattern = Some("(");
    assert!(poll_each(&mut remote, &mut kept, &bad).is_err());
}
