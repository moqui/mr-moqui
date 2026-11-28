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

//! SSH and SFTP on the seeded embedded Postgres, with the server as its own SFTP peer: the built-in sshd serves a
//! `dbresource://` root, the SFTP client logs in with a password and with a `UserSshKey`, the `moqui-sftp` services send
//! a SystemMessage and poll it back, the host key is trusted on first use and a changed key is refused, and the admin
//! shell runs for a user with `SSH_SHELL`, refuses one without, and checks artifact authorization for the user. Every
//! user gets the terminal client (`apps`), and a client with no key gets its login dialog. While the server runs SSH is
//! switched off and on and moved to another port, each access mode offers and takes its methods, and a dialog ends when
//! the mode leaves `open` (plans/Runtime-Config.md §3.4).

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "ssh"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use moqui_core::sftp::SftpClient;
use moqui_lib::object::{to_plain_string, Value};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{HashAlg, PrivateKey};

/// Host key seeds: the server's, and the one it is replaced with.
const HOST_SEED: [u8; 32] = [7; 32];
const OTHER_SEED: [u8; 32] = [9; 32];
/// john.doe's login key.
const USER_SEED: [u8; 32] = [3; 32];

fn data(user_key: &PrivateKey) -> String {
    let public = user_key.public_key().to_openssh().unwrap();
    let fingerprint = user_key.public_key().fingerprint(HashAlg::Sha256).to_string();
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.security.UserSshKey sshKeyId="JOHN_KEY" userId="EX_JOHN_DOE" publicKey="{public}" fingerprint="{fingerprint}"
        fromDate="1265184000000"/>
    <!-- password=moqui, as john.doe -->
    <moqui.security.UserAccount userId="SFTP_ONLY" username="sftp.only" userFullName="SFTP Only"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="SFTP_ONLY" description="SFTP only"/>
    <moqui.security.UserGroupMember userGroupId="SFTP_ONLY" userId="SFTP_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="SFTP_ONLY" userPermissionId="SSH_SFTP" fromDate="1265184000000"/>
    <moqui.security.UserAccount userId="SHELL_ONLY" username="shell.only" userFullName="Shell Only"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="SHELL_ONLY" description="SSH shell, no artifact authorization"/>
    <moqui.security.UserGroupMember userGroupId="SHELL_ONLY" userId="SHELL_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="SHELL_ONLY" userPermissionId="SSH_SHELL" fromDate="1265184000000"/>
    <moqui.security.UserAccount userId="NO_SSH" username="no.ssh" userFullName="No SSH"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>

    <moqui.service.message.SystemMessageType systemMessageTypeId="SshOut" description="Out by SFTP"
        sendServiceName="org.moqui.sftp.SftpMessageServices.send#SystemMessageSftp"
        sendPath="/outbox/${{systemMessageTypeId}}"/>
    <moqui.service.message.SystemMessageType systemMessageTypeId="SshIn" description="In by SFTP"
        consumeServiceName="org.moqui.impl.SystemMessageServices.reset#SystemMessageInError"
        receivePath="/outbox/SshOut" receiveFilePattern=".*" receiveResponseEnumId="MsgRrMove"
        receiveMovePath="/outbox/done"/>
</entity-facade-xml>"#
    )
}

/// As many connections to the SSH server at `address` as it has pre-auth slots, from 127.0.0.2, sending nothing, so
/// they wait to log in; returned once the server has taken or closed each of them.
fn hold_preauth_slots(address: std::net::SocketAddr) -> Vec<std::net::TcpStream> {
    let held = connect_from(address, "127.0.0.2", 16);
    if held.is_empty() {
        return held;
    }
    // the server answers each connection it keeps with its version line, and closes one past the share
    let mut kept = 0;
    for stream in &held {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut first = [0_u8; 4];
        if matches!(std::io::Read::read(&mut &*stream, &mut first), Ok(n) if n > 0) {
            kept += 1;
        }
    }
    assert!(kept > 0 && kept < 16, "{kept} of 16 kept from one address");
    held
}

/// `count` connections to `address` from the loopback address `source`. None where the host does not route `source`:
/// Linux routes all of 127/8 to lo, macOS only 127.0.0.1 unless lo0 has aliases, so there the checks of separate
/// sources are skipped (said on stderr) and the rest of the test runs.
fn connect_from(address: std::net::SocketAddr, source: &str, count: usize) -> Vec<std::net::TcpStream> {
    let source: std::net::SocketAddr = format!("{source}:0").parse().unwrap();
    let mut streams = Vec::new();
    for _ in 0..count {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        if let Err(err) = socket.bind(&source.into()) {
            if cfg!(not(target_os = "linux")) && err.kind() == std::io::ErrorKind::AddrNotAvailable {
                eprintln!("skipping the connections from {source}: this host does not route it ({err})");
                return Vec::new();
            }
            panic!("{source}: {err}");
        }
        socket.connect(&address.into()).unwrap();
        streams.push(socket.into());
    }
    streams
}

fn key(seed: [u8; 32]) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&seed))
}

fn sync(name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let result = moqui_core::service::sync(name, &parameters).unwrap_or_default();
    let errors = moqui_core::context::ExecutionContext::with_current(|ctx| {
        let text = ctx.message().errors().join("\n");
        ctx.message().clear();
        text
    })
    .unwrap_or_default();
    (result, errors)
}

/// Run one command over `exec` and return the output and the exit status.
fn exec(address: std::net::SocketAddr, username: &str, password: &str, command: &str) -> Result<(String, u32), String> {
    struct Accept;
    impl russh::client::Handler for Accept {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }
    let (username, password, command) = (username.to_string(), password.to_string(), command.to_string());
    moqui_host::edge::block_on(async move {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept)
            .await
            .map_err(|err| err.to_string())?;
        let auth = handle
            .authenticate_password(username, password)
            .await
            .map_err(|err| err.to_string())?;
        if !auth.success() {
            return Err("login refused".to_string());
        }
        let mut channel = handle.channel_open_session().await.map_err(|err| err.to_string())?;
        channel.exec(true, command).await.map_err(|err| err.to_string())?;
        let mut out = Vec::new();
        let mut status = None;
        while let Some(message) = channel.wait().await {
            match message {
                russh::ChannelMsg::Data { data } => out.extend_from_slice(&data),
                russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                russh::ChannelMsg::Failure => return Err("channel request refused".to_string()),
                _ => {}
            }
        }
        Ok((String::from_utf8_lossy(&out).into_owned(), status.unwrap_or(255)))
    })
    .map_err(|err| err.to_string())?
}

/// Open up to `count` session channels on one connection, sending no request on them, and return how many opened.
fn open_session_channels(address: std::net::SocketAddr, username: &str, password: &str, count: usize) -> usize {
    struct Accept;
    impl russh::client::Handler for Accept {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }
    let (username, password) = (username.to_string(), password.to_string());
    moqui_host::edge::block_on(async move {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.unwrap();
        assert!(handle
            .authenticate_password(username, password)
            .await
            .unwrap()
            .success());
        let mut opened = Vec::new();
        for _ in 0..count {
            match handle.channel_open_session().await {
                Ok(channel) => opened.push(channel),
                Err(_) => break,
            }
        }
        opened.len()
    })
    .unwrap()
}

#[test]
fn sftp_and_the_admin_shell_run_on_the_built_in_server() {
    if !crate::common::own_process() {
        return;
    }
    let user_key = key(USER_SEED);
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh",
        Some(&data(&user_key)),
        r#"<ssh-server sftp-root="dbresource://sftp"/>"#,
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
            ("moqui_conf__cluster_member__transport", "ssh"),
        ],
    );
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let host = format!("127.0.0.1:{}", address.port());
    let known_hosts = gate.runtime.join("conf/ssh_known_hosts");
    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    // The test reads the resource and message rows directly; the services check their own authorization.
    let authz = context.disable_authz();

    // Password login; the first connection records the host key.
    let mut client = SftpClient::new(&host, "john.doe", None).unwrap();
    client.password("moqui").connect().unwrap();
    assert!(std::fs::read_to_string(&known_hosts)
        .unwrap()
        .contains(&format!("[127.0.0.1]:{}", address.port())));
    client.mkdirs("/drop/deep").unwrap();
    assert_eq!(
        client
            .put_text("/drop", "a.txt", "caf\u{e9}", "ISO-8859-1", false)
            .unwrap(),
        "/drop/a.txt"
    );
    client.put_bytes("/drop/deep/b.bin", "", &[0, 1, 2, 255]).unwrap();
    assert_eq!(client.get_text("/drop/a.txt", "ISO-8859-1").unwrap(), "caf\u{e9}");
    assert_eq!(client.get_binary("/drop/deep/b.bin").unwrap(), [0, 1, 2, 255]);
    let listed = client.ls("/drop").unwrap();
    let names: Vec<&str> = listed.iter().map(|(_, entry)| entry.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "deep"]);
    assert_eq!(client.ls_files("/drop").unwrap(), ["a.txt"]);
    assert_eq!(client.rename_file("/drop/a.txt", "c.txt").unwrap(), "/drop/c.txt");
    assert_eq!(client.move_file("/drop/c.txt", "/kept", true).unwrap(), "/kept/c.txt");
    client.rm("/kept/c.txt").unwrap();
    assert!(client.ls_files("/kept").unwrap().is_empty());
    // A path that climbs out of the root is refused.
    assert!(client.get_binary("/../../etc/passwd").is_err());
    // The file landed in the database resource tree.
    let stored = factory
        .resource()
        .stat("dbresource://sftp/drop/deep/b.bin")
        .unwrap()
        .unwrap();
    assert_eq!(stored.size, 4);
    // Streams, as Java's openStream and putStream.
    let handle = client.put_stream("/drop/s.txt").unwrap();
    client.write_chunk(handle, b"one ").unwrap();
    client.write_chunk(handle, b"two").unwrap();
    client.close_handle(handle).unwrap();
    assert_eq!(client.get_binary("/drop/s.txt").unwrap(), b"one two");
    // The send below puts into this directory; without it the sendPath would be the file's own name, as in Java.
    client.mkdirs("/outbox/SshOut").unwrap();
    client.close();

    // Connections that never log in do not take the ssh-server max-sessions (8) slots from one that does (from two
    // other addresses, each within its share of the pre-auth slots).
    let mut idle = connect_from(address, "127.0.0.3", 4);
    idle.extend(connect_from(address, "127.0.0.4", 4));
    let mut past_idle = SftpClient::new(&host, "john.doe", None).unwrap();
    past_idle.password("moqui").connect().unwrap();
    assert_eq!(past_idle.ls_files("/drop").unwrap(), ["s.txt"]);
    past_idle.close();
    drop(idle);

    // Key login with the user's UserSshKey, and a key that is not one is refused. Another address holding every
    // pre-auth slot it can does not keep the login out (HL-032).
    let held = hold_preauth_slots(address);
    let pem = user_key.to_openssh(LineEnding::LF).unwrap().to_string();
    let mut keyed = SftpClient::new(&host, "john.doe", None).unwrap();
    keyed.public_key_pkcs8(&pem, None).connect().unwrap();
    assert_eq!(keyed.ls_files("/drop").unwrap(), ["s.txt"]);
    keyed.close();
    drop(held);
    let stranger = key(OTHER_SEED).to_openssh(LineEnding::LF).unwrap().to_string();
    let mut refused = SftpClient::new(&host, "john.doe", None).unwrap();
    assert!(refused.public_key_pkcs8(&stranger, None).connect().is_err());
    // A wrong password is refused.
    assert!(SftpClient::new(&host, "john.doe", None)
        .unwrap()
        .password("wrong")
        .connect()
        .is_err());
    // A user without SSH_SFTP gets the drives they may open (dav-server sftp-drives), as that user, by an app password
    // too: a write is an upload into the drive, a removal goes to its trash.
    moqui_core::user::internal_login_user("no.ssh").unwrap();
    let (out, errors) = sync(
        "org.moqui.impl.UserServices.create#UserAppPassword",
        &[("name", Value::string("Script")), ("scope", Value::string("ssh"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let app_password = to_plain_string(out.get("password").unwrap());
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let mut drives = SftpClient::new(&host, "no.ssh", None).unwrap();
    drives.password(&app_password).connect().unwrap();
    assert_eq!(
        drives.ls_files("/").unwrap(),
        Vec::<String>::new(),
        "the top holds folders only"
    );
    let tops: Vec<String> = drives
        .ls("/")
        .unwrap()
        .into_iter()
        .map(|(_, entry)| entry.name)
        .collect();
    assert_eq!(tops, ["My files"]);
    drives.mkdirs("/My files/Reports").unwrap();
    drives.put_bytes("/My files/Reports/r.csv", "", b"a,b\n1,2\n").unwrap();
    assert_eq!(drives.get_binary("/My files/Reports/r.csv").unwrap(), b"a,b\n1,2\n");
    assert_eq!(
        drives.rename_file("/My files/Reports/r.csv", "q.csv").unwrap(),
        "/My files/Reports/q.csv"
    );
    assert_eq!(drives.ls_files("/My files/Reports").unwrap(), ["q.csv"]);
    assert!(
        drives.put_bytes("/elsewhere.txt", "", b"x").is_err(),
        "the top is not a drive"
    );
    assert!(drives.get_binary("/My files/../../etc/passwd").is_err());
    drives.rm("/My files/Reports/q.csv").unwrap();
    assert!(drives.ls_files("/My files/Reports").unwrap().is_empty());
    drives.close();
    moqui_core::user::internal_login_user("no.ssh").unwrap();
    let (drives_out, _) = sync("org.moqui.resource.FileServices.get#UserDrives", &[]);
    let drive_id = to_plain_string(drives_out.get("personalDriveId").unwrap());
    let (trash, errors) = sync(
        "org.moqui.resource.FileServices.list#Trash",
        &[("driveId", Value::string(drive_id))],
    );
    moqui_core::user::internal_login_user("john.doe").unwrap();
    assert!(errors.is_empty(), "{errors}");
    assert!(format!("{trash:?}").contains("q.csv"), "{trash:?}");

    // The moqui-sftp services: a queued message goes out by SFTP, and the poll brings it back in.
    let (_, errors) = sync(
        "store#moqui.service.message.SystemMessageRemote",
        &[
            ("systemMessageRemoteId", Value::string("SshSelf")),
            ("sendUrl", Value::string(&host)),
            ("username", Value::string("john.doe")),
            ("password", Value::string("moqui")),
            // the server is this machine: the private network is allowed by the row (SEC-TLS-96)
            ("allowPrivateNetwork", Value::string("Y")),
        ],
    );
    assert_eq!(errors, "");
    let (queued, errors) = sync(
        "org.moqui.impl.SystemMessageServices.queue#SystemMessage",
        &[
            ("systemMessageTypeId", Value::string("SshOut")),
            ("systemMessageRemoteId", Value::string("SshSelf")),
            ("messageText", Value::string("{\"order\": 1}")),
            ("sendNow", Value::Bool(false)),
        ],
    );
    assert_eq!(errors, "");
    let sent_id = to_plain_string(queued.get("systemMessageId").unwrap());
    let (_, errors) = sync(
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &[("systemMessageId", Value::string(&sent_id))],
    );
    assert_eq!(errors, "");
    let row = |id: &str, field: &str| {
        factory
            .entity()
            .unwrap()
            .find("moqui.service.message.SystemMessage")
            .unwrap()
            .disable_authz()
            .eq("systemMessageId", Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(to_plain_string))
            .unwrap_or_default()
    };
    let send_errors: Vec<String> = factory
        .entity()
        .unwrap()
        .find("moqui.service.message.SystemMessageError")
        .unwrap()
        .disable_authz()
        .eq("systemMessageId", Value::string(&sent_id))
        .list()
        .unwrap()
        .values()
        .iter()
        .map(|error| error.get("errorText").map(to_plain_string).unwrap_or_default())
        .collect();
    assert_eq!(row(&sent_id, "statusId"), "SmsgSent", "{send_errors:?}");
    assert_eq!(row(&sent_id, "remoteMessageId"), sent_id);
    assert!(factory
        .resource()
        .stat(&format!("dbresource://sftp/outbox/SshOut/{sent_id}"))
        .unwrap()
        .is_some());

    let (polled, errors) = sync(
        "org.moqui.sftp.SftpMessageServices.poll#SystemMessageSftp",
        &[
            ("systemMessageTypeId", Value::string("SshIn")),
            ("systemMessageRemoteId", Value::string("SshSelf")),
        ],
    );
    assert_eq!(errors, "");
    assert_eq!(polled.get("fileCount").map(to_plain_string).as_deref(), Some("1"));
    let received = factory
        .entity()
        .unwrap()
        .find("moqui.service.message.SystemMessage")
        .unwrap()
        .disable_authz()
        .eq("systemMessageTypeId", Value::string("SshIn"))
        .list()
        .unwrap();
    assert_eq!(received.values().len(), 1);
    assert_eq!(
        received.values()[0].get("messageText").map(to_plain_string).as_deref(),
        Some("{\"order\": 1}")
    );
    assert!(factory
        .resource()
        .stat(&format!("dbresource://sftp/outbox/done/{sent_id}"))
        .unwrap()
        .is_some());
    assert!(factory
        .resource()
        .stat(&format!("dbresource://sftp/outbox/SshOut/{sent_id}"))
        .unwrap()
        .is_none());

    // A pinned fingerprint must match.
    let real = key(HOST_SEED).public_key().fingerprint(HashAlg::Sha256).to_string();
    let mut pinned = SftpClient::new(&host, "john.doe", None).unwrap();
    pinned.password("moqui").host_key_fingerprint(&real).connect().unwrap();
    pinned.close();
    let wrong = key(OTHER_SEED).public_key().fingerprint(HashAlg::Sha256).to_string();
    let mut pinned = SftpClient::new(&host, "john.doe", None).unwrap();
    let err = pinned
        .password("moqui")
        .host_key_fingerprint(&wrong)
        .connect()
        .err()
        .unwrap();
    assert!(err.message().contains("not the pinned"), "{}", err.message());

    // The admin shell: exec as john.doe (SSH_SHELL), and refused for a user with SFTP only.
    let (out, status) = exec(address, "john.doe", "moqui", "status").unwrap();
    assert_eq!(status, 0, "{out}");
    assert!(out.contains("user john.doe"), "{out}");
    let (out, status) = exec(address, "john.doe", "moqui", "js 6 * 7").unwrap();
    assert_eq!((out.trim(), status), ("42", 0));
    let (out, status) = exec(
        address,
        "john.doe",
        "moqui",
        "js const n = ec.user.username; return n + '!'",
    )
    .unwrap();
    assert_eq!((out.trim(), status), ("\"john.doe!\"", 0));
    let (out, status) = exec(
        address,
        "john.doe",
        "moqui",
        "entity moqui.security.UserAccount {\"username\": \"sftp.only\"}",
    )
    .unwrap();
    assert_eq!(status, 0, "{out}");
    assert!(out.contains("SFTP_ONLY"), "{out}");
    let (out, status) = exec(address, "john.doe", "moqui", "service no.such#Service").unwrap();
    assert_eq!(status, 1, "{out}");
    // SEC-FIL-62: each line that ran has an audit row (who, from where, what, its exit), a js line by its code's digest
    let audited: Vec<(String, String, i64)> = factory
        .entity()
        .unwrap()
        .find("moqui.terminal.TerminalSession")
        .unwrap()
        .disable_authz()
        .eq("username", Value::string("john.doe"))
        .eq("kind", Value::string("command"))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            let text = |field: &str| row.get(field).map(to_plain_string).unwrap_or_default();
            let exit = text("exitStatus").parse().unwrap_or(-1);
            (text("commandLine"), text("clientIp"), exit)
        })
        .collect();
    let line = |start: &str| audited.iter().find(|(line, _, _)| line.starts_with(start));
    let js = line("js sha256:").unwrap_or_else(|| panic!("{audited:?}"));
    assert!(
        !audited
            .iter()
            .any(|(line, _, _)| line.contains("6 * 7") || line.contains("ec.user")),
        "{audited:?}"
    );
    assert_eq!((js.1.as_str(), js.2), ("127.0.0.1", 0), "{audited:?}");
    let entity = line("entity moqui.security.UserAccount").unwrap_or_else(|| panic!("{audited:?}"));
    assert!(!entity.0.contains("sftp.only"), "{audited:?}");
    assert_eq!(line("service no.such#Service").map(|row| row.2), Some(1), "{audited:?}");
    assert!(line("status").is_none(), "{audited:?}");
    let refused = exec(address, "sftp.only", "moqui", "status");
    assert_eq!(refused, Err("channel request refused".to_string()));
    let mut sftp_only = SftpClient::new(&host, "sftp.only", None).unwrap();
    sftp_only.password("moqui").connect().unwrap();
    sftp_only.close();
    // SEC-FIL-02: an app password, which no second factor guards, opens the drives over SFTP and nothing else, for a
    // user with SSH_SHELL and SSH_SFTP too: no admin shell, no sftp-root, no agent-apps.
    let (out, errors) = sync(
        "org.moqui.impl.UserServices.create#UserAppPassword",
        &[("name", Value::string("Sync client")), ("scope", Value::string("ssh"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let john_app = to_plain_string(out.get("password").unwrap());
    assert_eq!(
        exec(address, "john.doe", &john_app, "status"),
        Err("channel request refused".to_string())
    );
    let (out, status) = exec(address, "john.doe", &john_app, "agent-apps").unwrap();
    assert_eq!(status, 2, "{out}");
    assert!(out.contains("drives over SFTP only"), "{out}");
    let mut by_app = SftpClient::new(&host, "john.doe", None).unwrap();
    by_app.password(&john_app).connect().unwrap();
    let tops: Vec<String> = by_app
        .ls("/")
        .unwrap()
        .into_iter()
        .map(|(_, entry)| entry.name)
        .collect();
    assert!(
        tops.contains(&"My files".to_string()),
        "the drives, not sftp-root: {tops:?}"
    );
    assert!(!tops.contains(&"drop".to_string()), "{tops:?}");
    by_app.close();
    // SEC-FIL-61: one connection holds at most 8 session channels waiting for a request or running sftp
    assert_eq!(open_session_channels(address, "john.doe", "moqui", 12), 8);
    // A shell user's commands run as that user: with no artifact authorization, entity and service calls are refused.
    let (out, status) = exec(address, "shell.only", "moqui", "status").unwrap();
    assert_eq!(status, 0, "{out}");
    let (out, status) = exec(
        address,
        "shell.only",
        "moqui",
        "entity moqui.security.UserAccount {\"username\": \"john.doe\"}",
    )
    .unwrap();
    assert_eq!(status, 1, "{out}");
    assert!(out.contains("not authorized"), "{out}");
    let (out, status) = exec(
        address,
        "shell.only",
        "moqui",
        "service update#moqui.security.UserAccount {\"userId\": \"SHELL_ONLY\", \"userFullName\": \"Changed\"}",
    )
    .unwrap();
    assert_eq!(status, 1, "{out}");
    assert!(out.contains("not authorized"), "{out}");
    let name = factory
        .entity()
        .unwrap()
        .find("moqui.security.UserAccount")
        .unwrap()
        .disable_authz()
        .eq("userId", Value::string("SHELL_ONLY"))
        .one()
        .unwrap()
        .and_then(|row| row.get("userFullName").map(to_plain_string));
    assert_eq!(name.as_deref(), Some("Shell Only"));

    // A server whose key changed is refused: the known_hosts line now names another key.
    let text = std::fs::read_to_string(&known_hosts).unwrap();
    let other = key(OTHER_SEED).public_key().to_openssh().unwrap();
    let marker = format!("[127.0.0.1]:{}", address.port());
    let changed: String = text
        .lines()
        .map(|line| {
            if line.starts_with(&marker) {
                format!("{marker} {other}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&known_hosts, changed + "\n").unwrap();
    let started = Instant::now();
    let err = SftpClient::new(&host, "john.doe", None)
        .unwrap()
        .password("moqui")
        .connect()
        .err()
        .unwrap();
    assert!(err.message().contains("changed"), "{}", err.message());
    assert!(started.elapsed() < Duration::from_secs(30));
    // The SSH cluster channel: this member registered ssh://its address with its host key; a peer's post arrives
    // through the channel and is applied as an HTTPS post would be.
    let base_url = format!("ssh://{address}");
    let member = factory
        .entity()
        .unwrap()
        .find("moqui.server.ClusterMember")
        .unwrap()
        .disable_authz()
        .list()
        .unwrap();
    let row = member.values().first().expect("the member row");
    assert_eq!(
        row.get("clusterBaseUrl").map(to_plain_string).as_deref(),
        Some(base_url.as_str())
    );
    assert_eq!(
        row.get("sshHostKeyFingerprint").map(to_plain_string),
        Some(real.clone())
    );
    let sender = moqui_host::ssh::cluster::sender(&factory).unwrap();
    let payload = br#"{"origin_id": "peer-1", "message_id": "m-1", "body": {"entityName": "moqui.basic.Enumeration"}}"#;
    let before = gate.server.cluster_posts().len();
    sender.post(&base_url, "entity-cache-invalidate", payload).unwrap();
    sender.post(&base_url, "entity-cache-invalidate", payload).unwrap();
    let posts = gate.server.cluster_posts();
    assert_eq!(posts.len(), before + 2, "{posts:?}");
    assert_eq!(posts.last().map(String::as_str), Some("entity-cache-invalidate"));
    let large = serde_json::to_vec(&serde_json::json!({
        "origin_id":"peer-large", "message_id":"large", "body": {
            "entityName":"moqui.basic.Enumeration", "padding":"x".repeat(2 * 1024 * 1024)
        }
    }))
    .unwrap();
    assert_eq!(
        sender.post(&base_url, "entity-cache-invalidate", &large).unwrap(),
        moqui_core::cluster::PostOutcome::Accepted
    );
    assert_eq!(
        sender
            .post(
                &base_url,
                "entity-cache-invalidate",
                &vec![b'x'; moqui_core::cluster::MAX_CLUSTER_BODY + 1]
            )
            .unwrap(),
        moqui_core::cluster::PostOutcome::Rejected(413)
    );
    assert_eq!(
        sender.post(&base_url, "entity-cache-invalidate", payload).unwrap(),
        moqui_core::cluster::PostOutcome::Accepted
    );
    // An unknown topic is refused by the peer, and a key that is not the cluster key cannot log in as the cluster.
    assert_eq!(
        sender.post(&base_url, "no-such-topic", payload).unwrap(),
        moqui_core::cluster::PostOutcome::Rejected(404)
    );
    let cluster_key = Arc::new(moqui_host::ssh::cluster::cluster_login_key().unwrap());
    moqui_host::edge::block_on(async move {
        struct Accept;
        impl russh::client::Handler for Accept {
            type Error = russh::Error;
            async fn check_server_key(&mut self, _: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
                Ok(true)
            }
        }
        let mut handle = russh::client::connect(Arc::new(russh::client::Config::default()), address, Accept)
            .await
            .unwrap();
        assert!(handle
            .authenticate_publickey(
                "moqui-cluster",
                russh::keys::PrivateKeyWithHashAlg::new(cluster_key, None)
            )
            .await
            .unwrap()
            .success());
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.request_shell(true).await.unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), channel.wait())
            .await
            .unwrap();
        assert!(
            matches!(answer, Some(russh::ChannelMsg::Failure)),
            "cluster key opened a shell: {answer:?}"
        );
    })
    .unwrap();
    // SEC-CLU-04: logged in with the cluster key, a channel's frames are still taken only after the channel's proof.
    // A peer that sends a frame with no proof (as an older member) or proves another key is refused, nothing applied.
    let before = gate.server.cluster_posts().len();
    for proof_seed in [None, Some([9u8; 32])] {
        let cluster_key = Arc::new(moqui_host::ssh::cluster::cluster_login_key().unwrap());
        let host_fingerprint = real.clone();
        let refused = moqui_host::edge::block_on(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            struct Accept;
            impl russh::client::Handler for Accept {
                type Error = russh::Error;
                async fn check_server_key(&mut self, _: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
                    Ok(true)
                }
            }
            let mut handle = russh::client::connect(Arc::new(russh::client::Config::default()), address, Accept)
                .await
                .unwrap();
            assert!(handle
                .authenticate_publickey(
                    "moqui-cluster",
                    russh::keys::PrivateKeyWithHashAlg::new(cluster_key, None)
                )
                .await
                .unwrap()
                .success());
            let channel = handle.channel_open_session().await.unwrap();
            channel.exec(true, "cluster entity-cache-invalidate").await.unwrap();
            let mut stream = tokio::io::BufReader::new(channel.into_stream());
            match proof_seed {
                None => {
                    let mut challenge = String::new();
                    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut challenge)).await;
                    assert!(challenge.starts_with("challenge "), "the server sends a challenge first: {challenge:?}");
                    let frame = br#"{"origin_id": "peer-2", "message_id": "m-9", "body": {"entityName": "moqui.basic.Enumeration"}}"#;
                    let out = stream.get_mut();
                    out.write_all(format!("{}\n", frame.len()).as_bytes()).await.unwrap();
                    out.write_all(frame).await.unwrap();
                    out.flush().await.unwrap();
                    let mut answer = String::new();
                    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut answer)).await;
                    answer.trim().to_string()
                }
                Some(seed) => {
                    let stranger = aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
                    moqui_host::ssh::cluster::prove_client(&mut stream, &stranger, &host_fingerprint)
                        .await
                        .unwrap_err()
                }
            }
        })
        .unwrap();
        assert!(
            refused == "403" || refused.contains("refused this member's proof"),
            "{proof_seed:?}: {refused}"
        );
    }
    assert_eq!(
        gate.server.cluster_posts().len(),
        before,
        "nothing applied without the channel's proof"
    );
    let stranger = Arc::new(key(OTHER_SEED));
    let refused = moqui_host::edge::block_on(async move {
        struct Accept;
        impl russh::client::Handler for Accept {
            type Error = russh::Error;
            async fn check_server_key(
                &mut self,
                _key: &russh::keys::PublicKeyOrCertificate,
            ) -> Result<bool, Self::Error> {
                Ok(true)
            }
        }
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.unwrap();
        let key = russh::keys::PrivateKeyWithHashAlg::new(stranger, None);
        handle
            .authenticate_publickey("moqui-cluster", key)
            .await
            .unwrap()
            .success()
    })
    .unwrap();
    assert!(!refused, "a stranger key logged in as the cluster");

    // SEC-FIL-11: a permission taken away refuses what an open connection asks for next: shell.only's second exec on
    // one connection, after its group membership (SSH_SHELL) ended, is refused.
    struct Accept;
    impl russh::client::Handler for Accept {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }
    async fn run(handle: &russh::client::Handle<Accept>, command: &str) -> Result<u32, String> {
        let mut channel = handle.channel_open_session().await.map_err(|err| err.to_string())?;
        channel.exec(true, command).await.map_err(|err| err.to_string())?;
        let mut status = None;
        while let Some(message) = channel.wait().await {
            match message {
                russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                russh::ChannelMsg::Failure => return Err("channel request refused".to_string()),
                _ => {}
            }
        }
        Ok(status.unwrap_or(255))
    }
    let (handle, first) = moqui_host::edge::block_on(async move {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.unwrap();
        assert!(handle
            .authenticate_password("shell.only", "moqui")
            .await
            .unwrap()
            .success());
        let first = run(&handle, "help").await;
        (handle, first)
    })
    .unwrap();
    assert!(first.is_ok(), "{first:?}");
    let entity = factory.entity().unwrap();
    let mut member = entity
        .find("moqui.security.UserGroupMember")
        .unwrap()
        .disable_authz()
        .eq("userId", Value::string("SHELL_ONLY"))
        .eq("userGroupId", Value::string("SHELL_ONLY"))
        .one()
        .unwrap()
        .unwrap();
    member.set(
        "thruDate",
        moqui_lib::object::basic_convert(&Value::I64(1_265_184_000_001), "Timestamp").value,
    );
    member.update(&entity).unwrap();
    let second = moqui_host::edge::block_on(async move { run(&handle, "help").await }).unwrap();
    assert_eq!(second, Err("channel request refused".to_string()));
    // and an account disabled while its connection is open is refused its next request, whatever it holds
    let (handle, first) = moqui_host::edge::block_on(async move {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.unwrap();
        assert!(handle
            .authenticate_password("john.doe", "moqui")
            .await
            .unwrap()
            .success());
        let first = run(&handle, "help").await;
        (handle, first)
    })
    .unwrap();
    assert!(first.is_ok(), "{first:?}");
    // the check at each request reads the account and changes nothing: the failed-login count a web password guess
    // is held to stays as it is
    let failed_logins = |set: Option<i64>| {
        let mut account = entity
            .find("moqui.security.UserAccount")
            .unwrap()
            .disable_authz()
            .eq("username", Value::string("john.doe"))
            .one()
            .unwrap()
            .unwrap();
        if let Some(count) = set {
            account.set("successiveFailedLogins", Value::I64(count));
            account.update(&entity).unwrap();
        }
        account
            .get("successiveFailedLogins")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    failed_logins(Some(2));
    let (handle, counted) = moqui_host::edge::block_on(async move {
        let result = run(&handle, "help").await;
        (handle, result)
    })
    .unwrap();
    assert!(counted.is_ok(), "{counted:?}");
    assert_eq!(failed_logins(None), "2", "a channel request leaves the count");
    failed_logins(Some(0));
    let set_disabled = |disabled: &str| {
        let mut account = entity
            .find("moqui.security.UserAccount")
            .unwrap()
            .disable_authz()
            .eq("username", Value::string("john.doe"))
            .one()
            .unwrap()
            .unwrap();
        account.set("disabled", Value::string(disabled));
        account.update(&entity).unwrap();
    };
    set_disabled("Y");
    let disabled = moqui_host::edge::block_on(async move { run(&handle, "help").await }).unwrap();
    set_disabled("N");
    assert_eq!(disabled, Err("channel request refused".to_string()));

    drop(authz);
    drop(_entered);
    drop(context);
}

/// Users of the `apps` tests: one with no SSH permission, and one with `SSH_SHELL` alone. john.doe (ADMIN) has the
/// shell too.
#[cfg(feature = "tui")]
const APPS_DATA: &str = r#"<entity-facade-xml type="seed">
    <!-- password=moqui, as john.doe -->
    <moqui.security.UserAccount userId="APPS_ONLY" username="apps.only" userFullName="Apps Only"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="SHELL_ONLY" username="shell.only" userFullName="Shell Only"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="SHELL_ONLY" description="SSH shell only"/>
    <moqui.security.UserGroupMember userGroupId="SHELL_ONLY" userId="SHELL_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="SHELL_ONLY" userPermissionId="SSH_SHELL" fromDate="1265184000000"/>
</entity-facade-xml>"#;

#[cfg(feature = "tui")]
mod apps {
    use std::sync::Arc;
    use std::time::Duration;

    use russh::client::{Handle, Msg};
    use russh::{Channel, ChannelMsg};

    pub struct Accept;

    impl russh::client::Handler for Accept {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    pub async fn connect(address: std::net::SocketAddr, username: &str, password: &str) -> Option<Handle<Accept>> {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.ok()?;
        let auth = handle.authenticate_password(username, password).await.ok()?;
        auth.success().then_some(handle)
    }

    /// A connection with no key and no password: keyboard-interactive, which the server takes with no questions.
    pub async fn connect_for_dialog(address: std::net::SocketAddr, username: &str) -> Option<Handle<Accept>> {
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(config, address, Accept).await.ok()?;
        let auth = handle
            .authenticate_keyboard_interactive_start(username, None)
            .await
            .ok()?;
        matches!(auth, russh::client::KeyboardInteractiveAuthResponse::Success).then_some(handle)
    }

    /// A session channel with a pty of `columns` by `rows`.
    pub async fn pty(handle: &Handle<Accept>, columns: u32, rows: u32) -> Channel<Msg> {
        let channel = handle.channel_open_session().await.unwrap();
        channel.set_env(false, "COLORTERM", "truecolor").await.unwrap();
        channel
            .request_pty(true, "xterm-256color", columns, rows, 0, 0, &[])
            .await
            .unwrap();
        channel
    }

    pub async fn send(channel: &Channel<Msg>, bytes: &[u8]) {
        channel.data(bytes).await.unwrap();
    }

    /// The channel's output with the escape sequences taken out: the text the frames wrote, in order.
    pub fn text(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                out.push(ch);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&next) {
                            break;
                        }
                    }
                    // A cursor move ends a run of text.
                    out.push('\n');
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Read until the output holds `want` (raw bytes when `raw`, else [`text`]); the exit status when the channel
    /// ends first.
    pub async fn read_until(
        channel: &mut Channel<Msg>,
        seen: &mut Vec<u8>,
        want: &str,
        raw: bool,
    ) -> Result<(), String> {
        let found = |seen: &[u8]| {
            if raw {
                seen.windows(want.len()).any(|window| window == want.as_bytes())
            } else {
                text(seen).contains(want)
            }
        };
        let wait = async {
            while !found(seen) {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => seen.extend_from_slice(&data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => return Err(format!("exited {exit_status}")),
                    Some(ChannelMsg::Failure) => return Err("channel request refused".to_string()),
                    Some(_) => {}
                    None => return Err("channel closed".to_string()),
                }
            }
            Ok(())
        };
        match tokio::time::timeout(Duration::from_secs(30), wait).await {
            Ok(result) => result.map_err(|err| format!("{err} before {want:?}:\n{}", text(seen))),
            Err(_) => Err(format!("{want:?} never came:\n{}", text(seen))),
        }
    }

    /// The exit status, once the channel ends.
    pub async fn exit_status(channel: &mut Channel<Msg>, seen: &mut Vec<u8>) -> Option<u32> {
        let mut status = None;
        let wait = async {
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => seen.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    _ => {}
                }
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(30), wait).await;
        status
    }
}

/// `apps` over SSH: a user with no SSH permission lands in it from `ssh user@host`; `ssh -t host apps` runs it; a window
/// change redraws at the new size; `apps` at the shell prompt comes back to the prompt; a channel without a pty is told
/// why not; and `terminal` `max-sessions-per-user` holds.
#[cfg(feature = "tui")]
#[test]
fn apps_runs_the_terminal_client_over_ssh() {
    if !crate::common::own_process() {
        return;
    }
    use apps::{connect, exit_status, pty, read_until, send, text};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh_apps",
        Some(APPS_DATA),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
        ],
    );
    let address = gate.server.ssh_address().expect("the SSH server listens");
    moqui_host::edge::block_on(async move {
        // No SSH permission: the shell request is the client, at the pty's size; Ctrl+Q ends the session.
        let handle = connect(address, "apps.only", "moqui").await.expect("apps.only logs in");
        let mut channel = pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "\x1b[?1049h", true).await.unwrap();
        read_until(&mut channel, &mut seen, "apps.only", false).await.unwrap();
        // True color, from the COLORTERM env request.
        read_until(&mut channel, &mut seen, "38;2;", true).await.unwrap();
        // apps.only may not view the System app, so Alt+L offers no log.
        send(&channel, b"\x1bl").await;
        read_until(&mut channel, &mut seen, "The log is not offered", false)
            .await
            .unwrap();

        // A second session of the same user is past terminal max-sessions-per-user (1).
        let mut second = pty(&handle, 100, 40).await;
        second.exec(true, "apps").await.unwrap();
        let mut refused = Vec::new();
        assert_eq!(exit_status(&mut second, &mut refused).await, Some(1));
        assert!(text(&refused).contains("max-sessions-per-user"), "{}", text(&refused));

        // A window change: the next frame is drawn whole after a clear.
        let before = seen.len();
        channel.window_change(180, 50, 0, 0).await.unwrap();
        let mut after = seen.split_off(before);
        read_until(&mut channel, &mut after, "\x1b[2J", true).await.unwrap();
        seen.extend(after);
        send(&channel, b"\x11").await;
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(0));
        assert!(seen.ends_with(b"\x1b[?1049l"), "the terminal is put back");

        // A user with SSH_SHELL alone: apps at the prompt needs no other permission, and quitting it is the prompt again.
        let handle = connect(address, "shell.only", "moqui")
            .await
            .expect("shell.only logs in");
        let mut channel = pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", true).await.unwrap();
        send(&channel, b"apps\r").await;
        read_until(&mut channel, &mut seen, "\x1b[?1049h", true).await.unwrap();
        read_until(&mut channel, &mut seen, "shell.only", false).await.unwrap();
        send(&channel, b"\x11").await;
        read_until(&mut channel, &mut seen, "\x1b[?1049l", true).await.unwrap();
        let mark = seen.len();
        let mut prompt = seen.split_off(mark);
        read_until(&mut channel, &mut prompt, "moqui> ", true).await.unwrap();
        send(&channel, b"exit\r").await;
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(0));

        // john.doe has the shell too: apps at the prompt, to the Tools user list by go to, and back to the prompt on quit.
        let handle = connect(address, "john.doe", "moqui").await.expect("john.doe logs in");
        let mut channel = pty(&handle, 160, 45).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", true).await.unwrap();
        send(&channel, b"apps\r").await;
        read_until(&mut channel, &mut seen, "\x1b[?1049h", true).await.unwrap();
        read_until(&mut channel, &mut seen, "john.doe", false).await.unwrap();
        let mark = seen.len();
        send(&channel, b"g").await;
        send(&channel, b"/apps/system/Security/UserAccount/UserAccountList").await;
        send(&channel, b"\r").await;
        let mut list = seen.split_off(mark);
        read_until(&mut channel, &mut list, "EX_JOHN_DOE", false).await.unwrap();
        seen.extend(list);
        // An admin's Alt+L opens the log pane (its WARN and ERROR lines: `logging::tap_at`, unit tested there).
        send(&channel, b"\x1bl").await;
        read_until(&mut channel, &mut seen, "Log · Alt+L closes", false)
            .await
            .unwrap();
        send(&channel, b"\x11").await;
        read_until(&mut channel, &mut seen, "\x1b[?1049l", true).await.unwrap();
        read_until(&mut channel, &mut seen, "moqui> ", true).await.unwrap();
        send(&channel, b"exit\r").await;
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(0));

        // Without a pty, apps says to ask for one.
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "apps").await.unwrap();
        let mut seen = Vec::new();
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(1));
        assert!(text(&seen).contains("ssh -t"), "{}", text(&seen));
    })
    .unwrap();
}

/// One exec with `input` written to its stdin (then end of input), as `ssh host command < input`: the output and the
/// exit status.
#[cfg(feature = "tui")]
fn exec_with_input(address: std::net::SocketAddr, username: &str, command: &str, input: &[u8]) -> (String, u32) {
    use apps::{connect, exit_status};
    let (username, command, input) = (username.to_string(), command.to_string(), input.to_vec());
    moqui_host::edge::block_on(async move {
        let handle = connect(address, &username, "moqui").await.expect("logs in");
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, command).await.unwrap();
        if !input.is_empty() {
            channel.data(&input[..]).await.unwrap();
        }
        channel.eof().await.unwrap();
        let mut seen = Vec::new();
        let status = exit_status(&mut channel, &mut seen).await.unwrap_or(255);
        (String::from_utf8_lossy(&seen).into_owned(), status)
    })
    .unwrap()
}

/// `agent-apps`: a session the server keeps between one-command execs, found only by its own user, counted against
/// `terminal` `max-sessions-per-user`; and the line mode, many commands over one exec.
#[cfg(feature = "tui")]
#[test]
fn agent_apps_answers_one_command_per_exec() {
    if !crate::common::own_process() {
        return;
    }
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh_agent",
        Some(APPS_DATA),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
        ],
    );
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let run = |user: &str, command: &str| exec_with_input(address, user, command, b"");

    let (out, status) = run(
        "john.doe",
        "agent-apps open /apps/system/Security/UserAccount/UserAccountList",
    );
    assert_eq!(status, 0, "{out}");
    let id = out
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("session "))
        .unwrap_or_else(|| panic!("{out}"))
        .to_string();
    assert!(
        out.contains("path /apps/system/Security/UserAccount/UserAccountList"),
        "{out}"
    );
    assert!(out.contains("john.doe") && out.trim_end().ends_with("--- ok"), "{out}");
    assert!(!out.contains("Tried to render subscreen"), "{out}");

    // The find dialog, its Username field by label, and Enter: one exec each.
    let (out, status) = run("john.doe", &format!("agent-apps {id} key f"));
    assert_eq!(status, 0, "{out}");
    assert!(out.contains("overlay find"), "{out}");
    let (out, status) = run("john.doe", &format!("agent-apps {id} set Username john"));
    assert_eq!(status, 0, "{out}");
    let (out, status) = run("john.doe", &format!("agent-apps {id} key Enter"));
    assert_eq!(status, 0, "{out}");
    assert!(out.contains("EX_JOHN_DOE") && !out.contains("bob.developer"), "{out}");
    let (out, status) = run("john.doe", &format!("agent-apps {id} elements --json"));
    assert_eq!(status, 0, "{out}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|err| panic!("{err}: {out}"));
    assert!(
        value["elements"]
            .as_array()
            .is_some_and(|elements| !elements.is_empty()),
        "{out}"
    );
    let (out, status) = run("john.doe", &format!("agent-apps {id} dance"));
    assert_eq!(status, 1, "{out}");

    // Another user does not find it, and john.doe's one session (terminal max-sessions-per-user) is in use.
    let (out, status) = run("apps.only", &format!("agent-apps {id} look"));
    assert_eq!(status, 2, "{out}");
    assert!(out.contains(&format!("no session {id}")), "{out}");
    let (out, status) = run("john.doe", "agent-apps open");
    assert_eq!(status, 2, "{out}");
    assert!(out.contains("max-sessions-per-user"), "{out}");
    // Any user who logged in by password may open one; shell.only has no SSH permission but the shell.
    let (out, status) = run("shell.only", "agent-apps open");
    assert_eq!(status, 0, "{out}");
    let other = out
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("session "))
        .unwrap_or_else(|| panic!("{out}"))
        .to_string();
    let (out, status) = run("shell.only", &format!("agent-apps {other} close"));
    assert_eq!(status, 0, "{out}");

    // close frees the slot.
    let (out, status) = run("john.doe", &format!("agent-apps {id} close"));
    assert_eq!(status, 0, "{out}");
    let (out, status) = run("john.doe", &format!("agent-apps {id} look"));
    assert_eq!(status, 2, "{out}");

    // The line mode: the first screen, then an answer per line, over one exec.
    let (out, status) = exec_with_input(
        address,
        "john.doe",
        "agent-apps",
        b"go /apps/system/Security/UserAccount/UserAccountList\nfocus Create User Account\nclose\n",
    );
    assert_eq!(status, 0, "{out}");
    assert_eq!(out.matches("--- ok").count(), 4, "{out}");
    assert!(out.contains("focus dialog"), "{out}");
    assert!(out.trim_end().ends_with("closed\n--- ok"), "{out}");
}

/// The login dialog over SSH: a client with no key is let in with no user (keyboard-interactive, no questions), gets
/// the apps' login dialog with its SSH name filled in, and once logged in the apps as that user, with no shell prompt
/// even for a user with `SSH_SHELL`. Such a connection gets nothing but `apps` on a pty, and a session closes after
/// five refused logins.
#[cfg(feature = "tui")]
#[test]
fn the_login_dialog_lets_in_a_client_with_no_key() {
    if !crate::common::own_process() {
        return;
    }
    use apps::{connect_for_dialog, exit_status, pty, read_until, send, text};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties(
        "ssh_dialog",
        Some(APPS_DATA),
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
        ],
    );
    let address = gate.server.ssh_address().expect("the SSH server listens");
    moqui_host::edge::block_on(async move {
        let handle = connect_for_dialog(address, "john.doe")
            .await
            .expect("keyboard-interactive lets the client in");

        // Commands, agent-apps and SFTP are refused on such a connection.
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "status").await.unwrap();
        let mut seen = Vec::new();
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(1));
        assert!(
            text(&seen).contains("log in with a key or a password"),
            "{}",
            text(&seen)
        );
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "agent-apps open").await.unwrap();
        let mut seen = Vec::new();
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(1));
        assert!(
            text(&seen).contains("log in with a key or a password"),
            "{}",
            text(&seen)
        );
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();
        let mut seen = Vec::new();
        assert!(read_until(&mut channel, &mut seen, "never", true)
            .await
            .unwrap_err()
            .contains("channel request refused"));
        // A TERM past 256 bytes is refused, so no channel keeps one of the size a packet holds (SEC-FIL-88).
        let mut channel = handle.channel_open_session().await.unwrap();
        channel
            .request_pty(true, &"x".repeat(300), 80, 24, 0, 0, &[])
            .await
            .unwrap();
        let mut seen = Vec::new();
        assert!(read_until(&mut channel, &mut seen, "never", true)
            .await
            .unwrap_err()
            .contains("channel request refused"));
        channel.close().await.unwrap();
        // and so is an env value past 256 bytes, a terminal variable's or GIT_PROTOCOL's
        let mut channel = handle.channel_open_session().await.unwrap();
        channel.set_env(true, "COLORTERM", "x".repeat(300)).await.unwrap();
        let mut seen = Vec::new();
        assert!(read_until(&mut channel, &mut seen, "never", true)
            .await
            .unwrap_err()
            .contains("channel request refused"));
        channel.close().await.unwrap();

        // The shell request is the apps with the login dialog, the name filled in and the password next.
        let mut channel = pty(&handle, 120, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "\x1b[?1049h", true).await.unwrap();
        read_until(&mut channel, &mut seen, "Log in", false).await.unwrap();
        read_until(&mut channel, &mut seen, "john.doe", false).await.unwrap();
        send(&channel, b"wrong\r").await;
        read_until(&mut channel, &mut seen, "not valid", false).await.unwrap();
        send(&channel, b"moqui\r").await;
        // Logged in: go to the user list as john.doe.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let mark = seen.len();
        send(&channel, b"g").await;
        send(&channel, b"/apps/system/Security/UserAccount/UserAccountList").await;
        send(&channel, b"\r").await;
        let mut list = seen.split_off(mark);
        read_until(&mut channel, &mut list, "EX_JOHN_DOE", false).await.unwrap();
        seen.extend(list);
        // A second login screen on the same connection is refused, so its channels cannot fill the sessions.
        let mut second = pty(&handle, 120, 40).await;
        second.exec(true, "apps").await.unwrap();
        let mut refused = Vec::new();
        assert_eq!(exit_status(&mut second, &mut refused).await, Some(1));
        assert!(
            text(&refused).contains("one login screen per connection"),
            "{}",
            text(&refused)
        );
        // Quitting ends the channel: no prompt, though john.doe has SSH_SHELL.
        send(&channel, b"\x11").await;
        assert_eq!(exit_status(&mut channel, &mut seen).await, Some(0));
        assert!(!text(&seen).contains("moqui> "), "{}", text(&seen));

        // Five refused logins close the session.
        let handle = connect_for_dialog(address, "nobody.here")
            .await
            .expect("keyboard-interactive lets the client in");
        let mut channel = pty(&handle, 120, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "Log in", false).await.unwrap();
        for _ in 0..5 {
            send(&channel, b"x\r").await;
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
        let status = exit_status(&mut channel, &mut seen).await;
        assert_eq!(status, Some(0), "{}", text(&seen));
        assert!(text(&seen).contains("5 refused logins"), "{}", text(&seen));
    })
    .unwrap();
}

/// `sh` at the SSH prompt (plans/archive/Terminal-Fancy.md §6): a program on the server's operating system for a user
/// with SSH_SHELL and TERMINAL_HOST on a dev instance, from this machine; the prompt comes back when it ends. A user
/// with SSH_SHELL alone is refused it.
#[cfg(all(feature = "tui", feature = "devtools"))]
#[test]
fn sh_at_the_ssh_prompt_runs_a_host_program() {
    if !crate::common::own_process() {
        return;
    }
    use apps::{connect, read_until, send, text};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let helper = crate::termws::helper_property();
    let gate = crate::common::start_with_conf_properties(
        "ssh_sh",
        Some(APPS_DATA),
        r#"<terminal><host-profile name="plain" command="sh"/></terminal>"#,
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
            ("moqui_conf__instance__purpose", "dev"),
            (helper.0, &helper.1),
        ],
    );
    let address = gate.server.ssh_address().expect("the SSH server listens");
    moqui_host::edge::block_on(async move {
        let handle = connect(address, "john.doe", "moqui").await.expect("john.doe logs in");
        let mut channel = apps::pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", false).await.unwrap();
        send(&channel, b"sh plain\r").await;
        send(&channel, b"echo x-$((2*3))\r").await;
        read_until(&mut channel, &mut seen, "x-6", false).await.unwrap();
        let before = seen.len();
        send(&channel, b"exit\r").await;
        let mut after = seen.split_off(before);
        // raw: the title the prompt sets again is an OSC that text() does not read past
        read_until(&mut channel, &mut after, "moqui> ", true).await.unwrap();

        let handle = connect(address, "shell.only", "moqui")
            .await
            .expect("shell.only logs in");
        let mut channel = apps::pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", false).await.unwrap();
        send(&channel, b"sh plain\r").await;
        read_until(&mut channel, &mut seen, "not available here", false)
            .await
            .unwrap();
        assert!(!text(&seen).contains("x-6"));
    })
    .unwrap();
}

/// `attach <id>` at the SSH prompt (plans/archive/Terminal-Fancy.md §15): the SSH window takes a session the browser
/// opened, with its output replayed, and gives it up on Ctrl+\ with the session still running. Another user's id, or
/// none, is no such session.
#[cfg(all(feature = "tui", feature = "devtools"))]
#[test]
fn attach_at_the_ssh_prompt_takes_a_kept_session() {
    if !crate::common::own_process() {
        return;
    }
    use apps::{connect, read_until, send};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let helper = crate::termws::helper_property();
    let gate = crate::common::start_with_conf_properties(
        "ssh_attach",
        Some(APPS_DATA),
        r#"<terminal><host-profile name="plain" command="sh"/></terminal>"#,
        &[
            ("moqui_conf__ssh_server__port", "0"),
            ("moqui_conf__ssh_server__enabled", "true"),
            ("moqui_conf__ssh_server__host_key", &seed),
            ("moqui_conf__instance__purpose", "dev"),
            (helper.0, &helper.1),
        ],
    );
    // a shell opened in the browser, with something on its screen
    let page = crate::termws::Page::signed_in(gate.server.address(), "john.doe");
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain", "cols": 100, "rows": 30})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut browser = page.attach(&id, None);
    browser
        .send(tungstenite::Message::Binary(b"echo m-$((4*5))\r".to_vec().into()))
        .unwrap();
    crate::termws::read_messages_until(&mut browser, "m-20").unwrap();

    let address = gate.server.ssh_address().expect("the SSH server listens");
    let prefix = id[..8].to_string();
    let attached = moqui_host::edge::block_on(async move {
        let handle = connect(address, "john.doe", "moqui").await.expect("john.doe logs in");
        let mut channel = apps::pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", false).await.unwrap();
        send(&channel, b"attach nosuch\r").await;
        // raw from here: the title an attach sets back is an OSC that text() does not read past
        read_until(&mut channel, &mut seen, "no such session nosuch", true)
            .await
            .unwrap();
        // the start of the id is enough; the replay brings back what the browser saw
        send(&channel, format!("attach {prefix}\r").as_bytes()).await;
        read_until(&mut channel, &mut seen, "m-20", true).await.unwrap();
        send(&channel, b"echo y-$((3*3))\r").await;
        read_until(&mut channel, &mut seen, "y-9", true).await.unwrap();
        let before = seen.len();
        send(&channel, &[0x1c]).await;
        let mut after = seen.split_off(before);
        read_until(&mut channel, &mut after, "moqui> ", true).await.unwrap();
        let back = String::from_utf8_lossy(&after).to_string();
        assert!(back.contains("detached; attach"), "{back}");
        let before = after.len();
        send(&channel, b"sessions\r").await;
        let mut listed = after.split_off(before);
        // the editor redraws the prompt as it echoes: wait for the listing's own words
        read_until(&mut channel, &mut listed, "running", true).await.unwrap();
        let listed = String::from_utf8_lossy(&listed).to_string();

        // another user's id is one that does not exist
        let handle = connect(address, "shell.only", "moqui")
            .await
            .expect("shell.only logs in");
        let mut channel = apps::pty(&handle, 100, 40).await;
        channel.request_shell(true).await.unwrap();
        let mut seen = Vec::new();
        read_until(&mut channel, &mut seen, "moqui> ", false).await.unwrap();
        send(&channel, format!("attach {prefix}\r").as_bytes()).await;
        read_until(&mut channel, &mut seen, "no such session", true)
            .await
            .unwrap();
        listed
    })
    .unwrap();
    assert!(
        attached
            .lines()
            .any(|line| line.contains(&id) && line.contains("running")),
        "the session goes on after the detach: {attached}"
    );
    // the browser's window was let go when SSH took the session
    let closed = crate::termws::read_messages_until(&mut browser, "never").unwrap_err();
    assert!(closed.contains("opened elsewhere"), "{closed}");
    page.call("close", Some(serde_json::json!({"sessionId": id})));
}

/// The SSH server switched while it runs (plans/Runtime-Config.md §3.4, §8.4, RC3): each change set as a spec's
/// `setConf` sets it (`set_conf_value`, whose `listeners` hook asks the registry for a pass), or with the hook held and
/// one pass that ends the connections now, as the listener services will.
mod switched {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use moqui_core::context::ExecutionContextFactory;
    use moqui_core::listeners::{Cause, Outcome, ReconcileRequest};
    use russh::client::{AuthResult, Handle};
    use russh::keys::PrivateKey;
    use russh::MethodKind;

    pub struct Accept;

    impl russh::client::Handler for Accept {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    /// A connection to `address`, not logged in.
    async fn connect(address: SocketAddr) -> Handle<Accept> {
        let config = Arc::new(russh::client::Config::default());
        russh::client::connect(config, address, Accept).await.unwrap()
    }

    /// A session logged in by password, kept open; none when the login is refused.
    pub fn by_password(address: SocketAddr, username: &str, password: &str) -> Option<Handle<Accept>> {
        let (username, password) = (username.to_string(), password.to_string());
        moqui_host::edge::block_on(async move {
            let mut handle = connect(address).await;
            let auth = handle.authenticate_password(username, password).await.ok()?;
            auth.success().then_some(handle)
        })
        .unwrap()
    }

    /// Whether a login by `key` as `username` is taken.
    pub fn by_key(address: SocketAddr, username: &str, key: &PrivateKey) -> bool {
        let (username, key) = (username.to_string(), Arc::new(key.clone()));
        moqui_host::edge::block_on(async move {
            let mut handle = connect(address).await;
            let key = russh::keys::PrivateKeyWithHashAlg::new(key, None);
            handle
                .authenticate_publickey(username, key)
                .await
                .is_ok_and(|auth| auth.success())
        })
        .unwrap()
    }

    /// A session logged in by `key` as `username`, kept open; none when the login is refused.
    pub fn by_key_kept(address: SocketAddr, username: &str, key: &PrivateKey) -> Option<Handle<Accept>> {
        let (username, key) = (username.to_string(), Arc::new(key.clone()));
        moqui_host::edge::block_on(async move {
            let mut handle = connect(address).await;
            let key = russh::keys::PrivateKeyWithHashAlg::new(key, None);
            let auth = handle.authenticate_publickey(username, key).await.ok()?;
            auth.success().then_some(handle)
        })
        .unwrap()
    }

    /// Whether `username`, logged in by `password`, is served the `sftp` subsystem: the server's answer to the request,
    /// which a client library's own wait would only time out on when it is refused.
    pub fn sftp_served(address: SocketAddr, username: &str, password: &str) -> bool {
        let handle = by_password(address, username, password).expect("the login");
        moqui_host::edge::block_on(async move {
            let mut channel = handle.channel_open_session().await.unwrap();
            channel.request_subsystem(true, "sftp").await.unwrap();
            loop {
                match tokio::time::timeout(Duration::from_secs(5), channel.wait()).await {
                    Ok(Some(russh::ChannelMsg::Success)) => return true,
                    Ok(Some(russh::ChannelMsg::Failure) | None) | Err(_) => return false,
                    Ok(Some(_)) => {}
                }
            }
        })
        .unwrap()
    }

    /// The methods a client asking to log in with nothing ("none") is told to go on with.
    pub fn offered(address: SocketAddr) -> Vec<MethodKind> {
        moqui_host::edge::block_on(async move {
            let mut handle = connect(address).await;
            match handle.authenticate_none("john.doe").await.unwrap() {
                AuthResult::Failure { remaining_methods, .. } => remaining_methods.to_vec(),
                AuthResult::Success => panic!("a login with nothing was taken"),
            }
        })
        .unwrap()
    }

    /// Whether `handle`'s session is ended by the server within `limit`.
    pub fn closed_within<H: russh::client::Handler>(handle: &Handle<H>, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if handle.is_closed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        handle.is_closed()
    }

    /// Whether `handle`'s session still opens a channel.
    pub fn still_open(handle: Handle<Accept>) -> bool {
        moqui_host::edge::block_on(async move { handle.channel_open_session().await.is_ok() }).unwrap()
    }

    pub fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
        let started = Instant::now();
        while !check() {
            assert!(started.elapsed() < limit, "{what} within {limit:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Set `selector` for this process's server: its `listeners` hook asks for a pass.
    pub fn set(factory: &ExecutionContextFactory, selector: &str, value: &str) {
        factory.set_conf_value(selector, Some(value), false).unwrap();
    }

    /// Set `selector` with the `listeners` hook held, then one pass that ends SSH's connections now; SSH's outcome.
    pub fn set_ending_now(factory: &ExecutionContextFactory, selector: &str, value: &str) -> Outcome {
        moqui_core::context::conf_reload::set_many_holding(
            factory,
            &[(selector, Some(value))],
            false,
            moqui_core::context::conf_schema::Group::Listeners,
        )
        .unwrap();
        let control = moqui_core::listeners::control(factory).expect("the server's listener control");
        let request = ReconcileRequest {
            cause: Cause::SetValue(selector.to_string()),
            end_now: vec!["SSH".to_string()],
        };
        let outcomes = control.reconcile(request, Duration::from_secs(10)).unwrap();
        outcomes
            .into_iter()
            .find(|outcome| outcome.name == "SSH")
            .expect("SSH's outcome")
    }

    /// SSH's row as the registry last published it.
    pub fn info(factory: &ExecutionContextFactory) -> moqui_core::listeners::ListenerInfo {
        moqui_core::listeners::control(factory)
            .expect("the server's listener control")
            .listeners()
            .into_iter()
            .find(|info| info.name == "SSH")
            .expect("SSH's row")
    }
}

/// Whether the admin shell at `address` answers john.doe, logged in by password.
fn shell_answers(address: std::net::SocketAddr) -> bool {
    matches!(exec(address, "john.doe", "moqui", "js 6 * 7"), Ok((out, 0)) if out.trim() == "42")
}

/// The properties of a server with SSH on, on a port of the kernel's choosing, and its host key.
fn ssh_on(seed: &str) -> [(&'static str, &str); 3] {
    [
        ("moqui_conf__ssh_server__port", "0"),
        ("moqui_conf__ssh_server__enabled", "true"),
        ("moqui_conf__ssh_server__host_key", seed),
    ]
}

/// SSH turned off on a running server ends its sessions (at once here, as "end connections now" asks) and lets its port
/// go; turned on again it listens and logs in; a new port binds there and lets the old one go. Each change is applied
/// with the `listeners` hook held and one pass of its own, as the listener services will, so the test waits on no
/// pass it did not ask for; the hook's own passes are the other tests'.
#[test]
fn ssh_is_switched_off_and_on_and_moves_its_port_while_the_server_runs() {
    use moqui_core::listeners::Action;
    use switched::{by_password, closed_within, info, set_ending_now};
    if !crate::common::own_process() {
        return;
    }
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties("ssh_switched", Some(&data(&key(USER_SEED))), &ssh_on(&seed));
    let factory = gate.server.factory();
    let first = gate.server.ssh_address().expect("the SSH server listens");
    let session = by_password(first, "john.doe", "moqui").expect("john.doe logs in");
    // the row counts the bind's sessions as of the last pass
    let control = moqui_core::listeners::control(factory).unwrap();
    let request = moqui_core::listeners::ReconcileRequest::new(moqui_core::listeners::Cause::ConfReload);
    control.reconcile(request, Duration::from_secs(10)).unwrap();
    assert_eq!(info(factory).connections, Some(1));

    let outcome = set_ending_now(factory, "ssh-server@enabled", "false");
    assert_eq!(outcome.action, Action::Stopped, "{}", outcome.detail);
    assert!(
        closed_within(&session, Duration::from_secs(2)),
        "the session ended at once"
    );
    assert!(std::net::TcpStream::connect(first).is_err(), "the port is let go");
    assert_eq!(gate.server.ssh_address(), None);
    let off = info(factory);
    assert_eq!(off.state.word(), Some("off"), "{:?}", off.state);
    assert!(
        off.state.detail().contains("ssh-server@enabled is false"),
        "{:?}",
        off.state
    );

    let outcome = set_ending_now(factory, "ssh-server@enabled", "true");
    assert_eq!(outcome.action, Action::Started, "{}", outcome.detail);
    let second = gate.server.ssh_address().expect("the SSH server listens again");
    assert!(shell_answers(second));
    let on = info(factory);
    assert!(
        on.state.detail().starts_with(&format!("tcp {second}, access open")),
        "{:?}",
        on.state
    );

    let port = crate::common::free_port();
    let outcome = set_ending_now(factory, "ssh-server@port", &port.to_string());
    assert_eq!(outcome.action, Action::Rebound, "{}", outcome.detail);
    let moved = gate
        .server
        .ssh_address()
        .expect("the SSH server listens on its new port");
    assert_eq!(moved.port(), port);
    assert!(shell_answers(moved));
    assert!(std::net::TcpStream::connect(second).is_err(), "the old port is let go");
}

/// Each access mode offers its methods to a client that asks with nothing, and a password is refused under `key`,
/// whatever the client tries, while a registered key still logs in; the change applies in place, on the same port.
#[test]
fn each_access_mode_offers_its_methods_and_key_refuses_a_password() {
    use moqui_core::listeners::Action;
    use russh::MethodKind::{KeyboardInteractive, Password, PublicKey};
    use switched::{by_key, by_key_kept, by_password, info, offered, set, still_open, wait_until};
    if !crate::common::own_process() {
        return;
    }
    let user_key = key(USER_SEED);
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties("ssh_access", Some(&data(&user_key)), &ssh_on(&seed));
    let factory = gate.server.factory();
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let open = if cfg!(feature = "tui") {
        vec![PublicKey, KeyboardInteractive, Password]
    } else {
        vec![PublicKey, Password]
    };
    assert_eq!(offered(address), open);

    set(factory, "ssh-server@access", "login");
    wait_until(Duration::from_secs(10), "login's methods", || {
        offered(address) == [PublicKey, Password]
    });
    let by_password_before = by_password(address, "john.doe", "moqui").expect("a password under login");
    let by_key_before = by_key_kept(address, "john.doe", &user_key).expect("a key under login");

    set(factory, "ssh-server@access", "key");
    wait_until(Duration::from_secs(10), "key's methods", || {
        offered(address) == [PublicKey]
    });
    assert!(
        by_password(address, "john.doe", "moqui").is_none(),
        "no password under key"
    );
    // signed in by a password before: it stays, and opens no new channel; one signed in by a key goes on
    assert!(!by_password_before.is_closed(), "the session stays");
    assert!(
        !still_open(by_password_before),
        "no new channel for a password session under key"
    );
    assert!(still_open(by_key_before), "a key session opens its channels");
    assert!(exec(address, "john.doe", "moqui", "status").is_err());
    assert!(by_key(address, "john.doe", &user_key), "a registered key under key");
    assert_eq!(
        gate.server.ssh_address(),
        Some(address),
        "applied in place, on the same port"
    );
    let row = info(factory);
    assert!(row.state.detail().contains("access key"), "{:?}", row.state);
    let control = moqui_core::listeners::control(factory).unwrap();
    let outcomes = control
        .reconcile(
            moqui_core::listeners::ReconcileRequest::new(moqui_core::listeners::Cause::ConfReload),
            Duration::from_secs(10),
        )
        .unwrap();
    let ssh = outcomes.iter().find(|outcome| outcome.name == "SSH").unwrap();
    assert_eq!(ssh.action, Action::Same, "{}", ssh.detail);
}

/// A connection still at the sign-in dialog ends when the access mode leaves `open`, and a new one is not let in;
/// a session signed in by password stays.
#[cfg(feature = "tui")]
#[test]
fn a_dialog_ends_when_access_leaves_open_and_a_signed_in_session_stays() {
    use switched::{by_password, closed_within, set, still_open};
    if !crate::common::own_process() {
        return;
    }
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties("ssh_dialog_off", Some(APPS_DATA), &ssh_on(&seed));
    let factory = gate.server.factory();
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let dialog = moqui_host::edge::block_on(async move { apps::connect_for_dialog(address, "john.doe").await })
        .unwrap()
        .expect("keyboard-interactive lets the client in");
    let signed_in = by_password(address, "john.doe", "moqui").expect("john.doe logs in");

    set(factory, "ssh-server@access", "login");
    assert!(closed_within(&dialog, Duration::from_secs(5)), "the dialog ended");
    assert!(!signed_in.is_closed(), "the signed-in session stays");
    assert!(still_open(signed_in), "and opens a channel");
    let again = moqui_host::edge::block_on(async move { apps::connect_for_dialog(address, "john.doe").await }).unwrap();
    assert!(again.is_none(), "no dialog under login");
}

/// Each sub-service flag turned off refuses its own channel for everyone, while the others serve; and under
/// `access="key"` an app password opens no SFTP, while the cluster's own key and a user's registered key log in
/// (plans/Runtime-Config.md §3.4).
#[test]
fn each_flag_refuses_its_channel_and_key_takes_keys_alone() {
    use switched::{by_key, offered, set, sftp_served, wait_until};
    if !crate::common::own_process() {
        return;
    }
    let user_key = key(USER_SEED);
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let mut properties = ssh_on(&seed).to_vec();
    // the cluster's key logs in from a member on the SSH transport: this server's own row
    properties.push(("moqui_conf__cluster_member__transport", "ssh"));
    let gate = crate::common::start_with_properties("ssh_flags", Some(&data(&user_key)), &properties);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let host = format!("127.0.0.1:{}", address.port());
    // the SFTP client is the factory's, so a context stays entered
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let john_app = {
        let _authz = context.disable_authz();
        let (out, errors) = sync(
            "org.moqui.impl.UserServices.create#UserAppPassword",
            &[("name", Value::string("Flags")), ("scope", Value::string("ssh"))],
        );
        assert!(errors.is_empty(), "{errors}");
        to_plain_string(out.get("password").unwrap())
    };
    let sftp = |password: &str| {
        SftpClient::new(&host, "john.doe", None)
            .unwrap()
            .password(password)
            .connect()
            .map(|client| client.close())
    };
    assert!(sftp(&john_app).is_ok(), "an app password opens the drives under open");

    for flag in ["tui-enabled", "sftp-enabled", "git-enabled", "shell-enabled"] {
        set(&factory, &format!("ssh-server@{flag}"), "false");
    }
    wait_until(Duration::from_secs(10), "the flags applied", || {
        exec(address, "john.doe", "moqui", "status") == Err("channel request refused".to_string())
    });
    let (out, status) = exec(address, "john.doe", "moqui", "apps").unwrap();
    assert!(
        status != 0 && out.contains("terminal client is turned off"),
        "{status}: {out}"
    );
    assert!(!sftp_served(address, "john.doe", "moqui"), "no SFTP");
    #[cfg(feature = "git")]
    assert_eq!(
        exec(address, "john.doe", "moqui", "git-upload-pack 'nothing.git'"),
        Err("channel request refused".to_string())
    );
    set(&factory, "ssh-server@shell-enabled", "true");
    wait_until(Duration::from_secs(10), "the shell on again", || shell_answers(address));
    for flag in ["tui-enabled", "sftp-enabled", "git-enabled"] {
        set(&factory, &format!("ssh-server@{flag}"), "true");
    }

    set(&factory, "ssh-server@access", "key");
    wait_until(Duration::from_secs(10), "key's methods", || {
        offered(address) == [russh::MethodKind::PublicKey]
    });
    assert!(sftp(&john_app).is_err(), "no app password under key");
    assert!(by_key(address, "john.doe", &user_key), "a registered key under key");
    let cluster_key = moqui_host::ssh::cluster::cluster_login_key().unwrap();
    assert!(
        by_key(address, "moqui-cluster", &cluster_key),
        "the cluster's key under key"
    );
}
