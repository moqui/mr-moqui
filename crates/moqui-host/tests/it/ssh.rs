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
//! user gets the terminal client (`apps`), and a client with no key gets its login dialog.

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

#[test]
fn sftp_and_the_admin_shell_run_on_the_built_in_server() {
    let user_key = key(USER_SEED);
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh",
        Some(&data(&user_key)),
        r#"<ssh-server sftp-root="dbresource://sftp"/>"#,
        &[("ssh_port", "0"), ("ssh_host_key", &seed), ("cluster_transport", "ssh")],
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

    // Connections that never log in do not take the ssh-server max-sessions (8) slots from one that does.
    let idle: Vec<std::net::TcpStream> = (0..8).map(|_| std::net::TcpStream::connect(address).unwrap()).collect();
    let mut past_idle = SftpClient::new(&host, "john.doe", None).unwrap();
    past_idle.password("moqui").connect().unwrap();
    assert_eq!(past_idle.ls_files("/drop").unwrap(), ["s.txt"]);
    past_idle.close();
    drop(idle);

    // Key login with the user's UserSshKey, and a key that is not one is refused.
    let pem = user_key.to_openssh(LineEnding::LF).unwrap().to_string();
    let mut keyed = SftpClient::new(&host, "john.doe", None).unwrap();
    keyed.public_key_pkcs8(&pem, None).connect().unwrap();
    assert_eq!(keyed.ls_files("/drop").unwrap(), ["s.txt"]);
    keyed.close();
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
        &[("name", Value::string("Script"))],
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
    let (drives_out, _) = sync("org.moqui.impl.FileServices.get#UserDrives", &[]);
    let drive_id = to_plain_string(drives_out.get("personalDriveId").unwrap());
    let (trash, errors) = sync(
        "org.moqui.impl.FileServices.list#Trash",
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
    let refused = exec(address, "sftp.only", "moqui", "status");
    assert_eq!(refused, Err("channel request refused".to_string()));
    let mut sftp_only = SftpClient::new(&host, "sftp.only", None).unwrap();
    sftp_only.password("moqui").connect().unwrap();
    sftp_only.close();
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
    // An unknown topic is refused by the peer, and a key that is not the cluster key cannot log in as the cluster.
    assert!(sender.post(&base_url, "no-such-topic", payload).is_err());
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
    use apps::{connect, exit_status, pty, read_until, send, text};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh_apps",
        Some(APPS_DATA),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[("ssh_port", "0"), ("ssh_host_key", &seed)],
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
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "ssh_agent",
        Some(APPS_DATA),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[("ssh_port", "0"), ("ssh_host_key", &seed)],
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
    use apps::{connect_for_dialog, exit_status, pty, read_until, send, text};

    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties(
        "ssh_dialog",
        Some(APPS_DATA),
        &[("ssh_port", "0"), ("ssh_host_key", &seed)],
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
