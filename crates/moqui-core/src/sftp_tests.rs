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

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

/// A remote file system in memory: paths to file bytes, and the directories.
#[derive(Default)]
pub(crate) struct Disk {
    pub files: BTreeMap<String, Vec<u8>>,
    pub dirs: std::collections::BTreeSet<String>,
    pub chmods: Vec<String>,
    /// The name of the thread each session closed on.
    pub closed_on: Vec<String>,
}

pub(crate) struct MemorySession {
    disk: Arc<Mutex<Disk>>,
    open: bool,
    handles: BTreeMap<u64, (String, usize)>,
}

impl SftpSession for MemorySession {
    fn read_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, Error> {
        let disk = self.disk.lock().unwrap();
        let prefix = format!("{}/", path.trim_end_matches('/'));
        if !disk.dirs.contains(path.trim_end_matches('/')) {
            return Err(Error::new(format!("No such file: {path}")));
        }
        let mut out = Vec::new();
        for (name, bytes) in &disk.files {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    out.push(SftpEntry {
                        name: rest.to_string(),
                        attributes: SftpAttributes {
                            kind: SftpKind::File,
                            size: Some(bytes.len() as u64),
                            access_time: Some(1),
                            modified_time: Some(2),
                        },
                    });
                }
            }
        }
        for dir in &disk.dirs {
            if let Some(rest) = dir.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    out.push(SftpEntry {
                        name: rest.to_string(),
                        attributes: SftpAttributes {
                            kind: SftpKind::Directory,
                            size: None,
                            access_time: None,
                            modified_time: None,
                        },
                    });
                }
            }
        }
        Ok(out)
    }
    fn stat(&mut self, path: &str) -> Result<Option<SftpAttributes>, Error> {
        let disk = self.disk.lock().unwrap();
        let path = path.trim_end_matches('/');
        Ok(if disk.dirs.contains(path) {
            Some(SftpAttributes {
                kind: SftpKind::Directory,
                size: None,
                access_time: None,
                modified_time: None,
            })
        } else {
            disk.files.get(path).map(|bytes| SftpAttributes {
                kind: SftpKind::File,
                size: Some(bytes.len() as u64),
                access_time: None,
                modified_time: None,
            })
        })
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        self.disk
            .lock()
            .unwrap()
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| Error::new(format!("No such file: {path}")))
    }
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let mut disk = self.disk.lock().unwrap();
        if !disk.dirs.contains(&parent_of(path)) {
            return Err(Error::new(format!("No such file: {path}")));
        }
        disk.files.insert(path.to_string(), bytes.to_vec());
        Ok(())
    }
    fn set_permissions(&mut self, path: &str, _mode: u32) -> Result<(), Error> {
        self.disk.lock().unwrap().chmods.push(path.to_string());
        Ok(())
    }
    fn make_dir(&mut self, path: &str) -> Result<(), Error> {
        self.disk.lock().unwrap().dirs.insert(path.to_string());
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let mut disk = self.disk.lock().unwrap();
        if let Some(bytes) = disk.files.remove(from) {
            disk.files.insert(to.to_string(), bytes);
            return Ok(());
        }
        let from_dir = from.trim_end_matches('/').to_string();
        if !disk.dirs.contains(&from_dir) {
            return Err(Error::new(format!("No such file: {from}")));
        }
        disk.dirs.remove(&from_dir);
        disk.dirs.insert(to.trim_end_matches('/').to_string());
        let prefix = format!("{from_dir}/");
        let files: Vec<String> = disk
            .files
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect();
        for key in files {
            let bytes = disk.files.remove(&key).unwrap_or_default();
            let rest = key.strip_prefix(&from_dir).unwrap_or("");
            disk.files.insert(format!("{to}{rest}"), bytes);
        }
        let dirs: Vec<String> = disk
            .dirs
            .iter()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect();
        let to_dir = to.trim_end_matches('/');
        for key in dirs {
            disk.dirs.remove(&key);
            let rest = key.strip_prefix(&from_dir).unwrap_or("");
            disk.dirs.insert(format!("{to_dir}{rest}"));
        }
        Ok(())
    }
    fn remove_file(&mut self, path: &str) -> Result<(), Error> {
        self.disk
            .lock()
            .unwrap()
            .files
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| Error::new(format!("No such file: {path}")))
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), Error> {
        let path = path.trim_end_matches('/').to_string();
        let mut disk = self.disk.lock().unwrap();
        let prefix = format!("{path}/");
        if disk.files.keys().any(|name| name.starts_with(&prefix))
            || disk.dirs.iter().any(|name| name.starts_with(&prefix))
        {
            return Err(Error::new(format!("{path} is not empty")));
        }
        if disk.dirs.remove(&path) {
            Ok(())
        } else {
            Err(Error::new(format!("No such file: {path}")))
        }
    }
    fn open(&mut self, path: &str, write: bool) -> Result<u64, Error> {
        if write {
            self.write(path, b"")?;
        } else {
            self.read(path)?;
        }
        let handle = self.handles.len() as u64 + 1;
        self.handles.insert(handle, (path.to_string(), 0));
        Ok(handle)
    }
    fn read_chunk(&mut self, handle: u64, max: usize) -> Result<Vec<u8>, Error> {
        let (path, offset) = self.handles.get(&handle).cloned().unwrap();
        let bytes = self.read(&path)?;
        let end = (offset + max).min(bytes.len());
        self.handles.insert(handle, (path, end));
        Ok(bytes[offset..end].to_vec())
    }
    fn write_chunk(&mut self, handle: u64, bytes: &[u8]) -> Result<(), Error> {
        let (path, _) = self.handles.get(&handle).cloned().unwrap();
        self.disk
            .lock()
            .unwrap()
            .files
            .entry(path)
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }
    fn close_handle(&mut self, handle: u64) -> Result<(), Error> {
        self.handles.remove(&handle);
        Ok(())
    }
    fn close(&mut self) {
        if std::mem::take(&mut self.open) {
            let thread = std::thread::current().name().unwrap_or_default().to_string();
            self.disk.lock().unwrap().closed_on.push(thread);
        }
    }
    fn is_connected(&self) -> bool {
        self.open
    }
}

/// Connects every config to one shared disk and records the configs it saw.
#[derive(Default)]
pub(crate) struct MemoryConnector {
    pub disk: Arc<Mutex<Disk>>,
    pub seen: Mutex<Vec<SftpConfig>>,
}

impl SftpConnector for MemoryConnector {
    fn connect(&self, config: &SftpConfig) -> Result<Box<dyn SftpSession>, Error> {
        self.seen.lock().unwrap().push(config.clone());
        Ok(Box::new(MemorySession {
            disk: Arc::clone(&self.disk),
            open: true,
            handles: BTreeMap::new(),
        }))
    }
}

/// Install a fresh memory connector with `/in` as a directory on `factory`.
pub(crate) fn memory(factory: &ExecutionContextFactory) -> Arc<MemoryConnector> {
    let connector = Arc::new(MemoryConnector::default());
    connector.disk.lock().unwrap().dirs.insert("/in".to_string());
    install_sftp_connector(factory, connector.clone());
    connector
}

#[test]
fn the_client_keeps_the_java_behavior() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let mut client = SftpClient::new("example.com", "joe", None).unwrap();
    assert_eq!(client.config().port, SSH_PORT);
    assert_eq!(SftpClient::new("h:2222", "joe", Some(22)).unwrap().config().port, 2222);
    let err = client.connect().err().unwrap();
    assert_eq!(
        err.message(),
        "No password or KeyProvider specified, cannot connect to joe@example.com:22"
    );
    let err = SftpClient::new("", "joe", None)
        .unwrap()
        .password("x")
        .connect()
        .err()
        .unwrap();
    assert!(
        err.message().starts_with("Host or username not specified"),
        "{}",
        err.message()
    );
    assert_eq!(client.ls_files("/").unwrap_err().message(), "SFTP Client not connected");
    client.password("pw");
    assert_eq!(client.connect().err().unwrap().message(), SFTP_UNAVAILABLE);

    let connector = memory(context.factory());
    client.connect().unwrap();
    // A directory target uses the file name; a file target is the path.
    assert_eq!(
        client
            .put_text("/in", "a.txt", "caf\u{e9}", "ISO-8859-1", false)
            .unwrap(),
        "/in/a.txt"
    );
    assert_eq!(connector.disk.lock().unwrap().files["/in/a.txt"], b"caf\xe9");
    assert_eq!(client.get_text("/in/a.txt", "ISO-8859-1").unwrap(), "caf\u{e9}");
    assert_eq!(
        client.put_text("/in/b.txt", "ignored", "b", "", false).unwrap(),
        "/in/b.txt"
    );
    // createDir makes the parent of the path.
    client.put_text("/out/deep/c.txt", "x", "c", "UTF-8", true).unwrap();
    assert!(connector.disk.lock().unwrap().dirs.contains("/out/deep"));
    assert_eq!(client.ls_files("/in").unwrap(), ["a.txt", "b.txt"]);
    let listed = client.ls("/out").unwrap();
    assert_eq!(listed[0].0, "/out/deep");
    assert_eq!(listed[0].1.attributes.kind, SftpKind::Directory);
    assert_eq!(
        client.move_file("/in/a.txt", "/in/done", true).unwrap(),
        "/in/done/a.txt"
    );
    assert_eq!(client.rename_file("/in/b.txt", "b2.txt").unwrap(), "/in/b2.txt");
    client.rm("/in/b2.txt").unwrap();
    assert_eq!(client.ls_files("/in").unwrap(), Vec::<String>::new());
    assert_eq!(connector.disk.lock().unwrap().chmods.len(), 3);
    client.preserve_attributes(false);
    client.put_bytes("/in", "d.bin", &[0, 1]).unwrap();
    assert_eq!(connector.disk.lock().unwrap().chmods.len(), 3);
    assert!(client.get_text("/in/d.bin", "no-such-charset").is_err());
    client.close();
    assert_eq!(
        client.ls_files("/in").unwrap_err().message(),
        "SFTP Client not connected"
    );
}

/// An SFTP connection is a remote call: refused under the read fence and in a sim preview before anything opens,
/// and denied in sim mode.
#[test]
fn a_connection_is_refused_under_the_fence_and_in_sim_mode() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let connector = memory(context.factory());
    for sim in [false, true] {
        let fence = if sim {
            crate::entity::fence::enter_sim("a preview")
        } else {
            crate::entity::fence::enter("tool a_read")
        };
        let mut client = SftpClient::new("example.com", "joe", None).unwrap();
        let refused = client.password("pw").connect().err().unwrap();
        assert!(crate::entity::fence::is_fence_error(&refused), "{refused}");
        assert!(refused.message().contains("SFTP joe@example.com:22"), "{refused}");
        assert_eq!(fence.violations().len(), 1);
    }
    let facade = crate::testing::open();
    context.factory().set_entity(facade.clone());
    facade.set_sim_active(true);
    let mut client = SftpClient::new("example.com", "joe", None).unwrap();
    let denied = client.password("pw").connect().err().unwrap();
    facade.set_sim_active(false);
    assert!(denied.message().contains("sim mode is active"), "{denied}");
    assert!(
        connector.disk.lock().unwrap().closed_on.is_empty(),
        "no session was opened"
    );
    client.password("pw").connect().unwrap();
}

#[test]
fn a_client_dropped_open_closes_on_a_thread_of_its_own() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let connector = memory(context.factory());
    let mut client = SftpClient::new("example.com", "joe", None).unwrap();
    client.password("pw").connect().unwrap();
    drop(client);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while connector.disk.lock().unwrap().closed_on.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(connector.disk.lock().unwrap().closed_on, ["sftp-close"]);
}

#[test]
fn each_factory_uses_its_own_connector() {
    let first = ExecutionContextFactory::test_context();
    let second = ExecutionContextFactory::test_context();
    let connector = memory(first.factory());
    {
        let _entered = second.enter();
        let mut client = SftpClient::new("example.com", "joe", None).unwrap();
        assert_eq!(
            client.password("pw").connect().err().unwrap().message(),
            SFTP_UNAVAILABLE
        );
    }
    let _entered = first.enter();
    let mut client = SftpClient::new("example.com", "joe", None).unwrap();
    client.password("pw").connect().unwrap();
    assert_eq!(connector.seen.lock().unwrap().len(), 1);
    client.close();
    clear_sftp_connector(first.factory());
    assert_eq!(client.connect().err().unwrap().message(), SFTP_UNAVAILABLE);
}

#[test]
fn hosts_split_with_ipv6_brackets() {
    let split = |text: &str| split_host_port(text, SSH_PORT).unwrap();
    assert_eq!(split("example.com"), ("example.com".to_string(), 22));
    assert_eq!(split("example.com:2222"), ("example.com".to_string(), 2222));
    assert_eq!(split("[::1]:2222"), ("::1".to_string(), 2222));
    assert_eq!(split("[::1]"), ("::1".to_string(), 22));
    assert_eq!(split("fe80::1"), ("fe80::1".to_string(), 22));
    assert!(split_host_port("[::1", SSH_PORT).is_err());
    assert!(split_host_port("[::1]x", SSH_PORT).is_err());
    assert!(split_host_port("host:port", SSH_PORT).is_err());
    assert_eq!(split(":2222"), (":2222".to_string(), 22));
    assert_eq!(split("h\u{e9}te:2222"), ("h\u{e9}te".to_string(), 2222));
}

#[test]
fn parents_drop_the_last_segment_and_trailing_slashes() {
    assert_eq!(parent_of("/in/a.txt"), "/in");
    assert_eq!(parent_of("/in/sub/"), "/in");
    assert_eq!(parent_of("/a.txt"), "/");
    assert_eq!(parent_of("a.txt"), "");
    assert_eq!(parent_of("in/\u{e9}t\u{e9}.txt"), "in");
}

#[test]
fn debug_leaves_out_the_password_and_private_key() {
    let mut client = SftpClient::new("example.com", "joe", None).unwrap();
    client
        .password("hunter2")
        .public_key_pkcs8("PRIVATE-PEM", Some("ssh-ed25519 AAAA"));
    let shown = format!("{:?}", client.config());
    assert!(!shown.contains("hunter2") && !shown.contains("PRIVATE-PEM"), "{shown}");
    assert!(
        shown.contains("ssh-ed25519 AAAA") && shown.contains("example.com"),
        "{shown}"
    );
}
