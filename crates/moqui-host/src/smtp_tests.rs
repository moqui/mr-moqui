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
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Accounts by address, and every message delivered; the store fails for account `fail@`, and account `gone@`
/// is removed by the time the message comes.
#[derive(Default)]
struct Fake {
    delivered: Mutex<Vec<Message>>,
}

impl Backend for Arc<Fake> {
    async fn recipient(&self, address: String) -> RcptAnswer {
        match address.to_ascii_lowercase().as_str() {
            "sales@example.com" | "fail@example.com" | "gone@example.com" => {
                RcptAnswer::Local(address.to_ascii_lowercase())
            }
            "help@example.com" => RcptAnswer::Local("sales@example.com".to_string()),
            "busy@example.com" => RcptAnswer::TempFail("database down".to_string()),
            _ => RcptAnswer::Unknown,
        }
    }

    async fn deliver(&self, message: Message) -> DataAnswer {
        let accounts = message
            .recipients
            .iter()
            .map(|group| {
                let result = match group.email_account_id.as_str() {
                    "fail@example.com" => AccountResult::Failed,
                    "gone@example.com" => AccountResult::Gone,
                    _ => AccountResult::Stored,
                };
                (group.email_account_id.clone(), result)
            })
            .collect();
        let bad = !message.raw.windows(2).any(|pair| pair == b": ");
        self.delivered.lock().unwrap().push(message);
        if bad {
            DataAnswer::Rejected("the message has no header fields".to_string())
        } else {
            DataAnswer::Stored {
                accounts,
                summary: "ok".to_string(),
            }
        }
    }
}

fn config(allowed: &str) -> SmtpConfig {
    SmtpConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        hostname: "mx.test".to_string(),
        allowed: allowed.split(',').map(|net| AllowedNet::parse(net).unwrap()).collect(),
        max_message: 1000,
        max_recipients: 2,
        max_connections: 2,
        command_timeout: Duration::from_secs(5),
        session_timeout: Duration::from_secs(30),
        proxy_protocol: false,
        plus_separator: "+".to_string(),
        spam_headers: Vec::new(),
        authserv_id: String::new(),
        lmtp: false,
        starttls: false,
        require_tls: false,
        tls_cert_file: String::new(),
        tls_key_file: String::new(),
        chunking: true,
        dsn: false,
        internet: None,
    }
}

async fn start(config: SmtpConfig) -> (SocketAddr, Arc<Fake>, tokio::sync::watch::Sender<bool>) {
    start_tls(config, None).await
}

async fn start_tls(
    config: SmtpConfig,
    tls: Option<TlsAcceptor>,
) -> (SocketAddr, Arc<Fake>, tokio::sync::watch::Sender<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fake = Arc::new(Fake::default());
    let (stop, watch) = tokio::sync::watch::channel(false);
    tokio::spawn(serve(listener, config, tls, Arc::clone(&fake), watch));
    (address, fake, stop)
}

struct Client {
    stream: BufReader<Box<dyn Io>>,
}

impl Client {
    async fn connect(address: SocketAddr) -> Self {
        Self::from(TcpStream::connect(address).await.unwrap())
    }

    fn from(stream: TcpStream) -> Self {
        Self {
            stream: BufReader::new(Box::new(stream)),
        }
    }

    /// TLS on the connection, as a client after `220` to STARTTLS.
    async fn upgrade(&mut self, connector: &tokio_rustls::TlsConnector) {
        let placeholder: Box<dyn Io> = Box::new(tokio::io::duplex(1).0);
        let plain = std::mem::replace(&mut self.stream, BufReader::new(placeholder)).into_inner();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let secured = connector.connect(name, plain).await.expect("the TLS handshake");
        self.stream = BufReader::new(Box::new(secured));
    }

    /// One reply, its lines joined with `|`; empty at the end of the stream.
    async fn reply(&mut self) -> String {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            if self.stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end().to_string();
            let last = line.as_bytes().get(3) != Some(&b'-');
            lines.push(line);
            if last {
                break;
            }
        }
        lines.join("|")
    }

    async fn send(&mut self, text: &str) {
        self.send_bytes(text.as_bytes()).await;
    }

    async fn send_bytes(&mut self, bytes: &[u8]) {
        self.stream.get_mut().write_all(bytes).await.unwrap();
    }

    async fn command(&mut self, line: &str) -> String {
        self.send(&format!("{line}\r\n")).await;
        self.reply().await
    }
}

#[tokio::test]
async fn a_session_takes_mail_for_known_recipients_only() {
    let (address, fake, _stop) = start(config("127.0.0.1")).await;
    let mut client = Client::connect(address).await;
    assert_eq!(client.reply().await, "220 mx.test ESMTP mr-moqui");
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "503 5.5.1 Send EHLO first");
    let ehlo = client.command("EHLO gw.example.com").await;
    assert_eq!(
        ehlo,
        "250-mx.test|250-PIPELINING|250-SIZE 1000|250-8BITMIME|250-SMTPUTF8|250-CHUNKING|250 ENHANCEDSTATUSCODES"
    );
    assert_eq!(
        client.command("RCPT TO:<sales@example.com>").await,
        "503 5.5.1 Need MAIL command"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> SIZE=2000").await,
        "552 5.3.4 Message size exceeds fixed maximum message size"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> BODY=8BITMIME SIZE=100").await,
        "250 2.1.0 Ok"
    );
    assert_eq!(
        client.command("RCPT TO:<nobody@example.com>").await,
        "550 5.1.1 <nobody@example.com>: Recipient address rejected: User unknown"
    );
    assert_eq!(
        client.command("RCPT TO:<busy@example.com>").await,
        "451 4.3.0 <busy@example.com>: Temporary lookup failure, try again later"
    );
    assert_eq!(client.command("RCPT TO:<Sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(
        client.command("RCPT TO:<@relay.example:sales@example.com>").await,
        "250 2.1.5 Ok"
    );
    assert_eq!(
        client.command("RCPT TO:<sales@example.com>").await,
        "452 4.5.3 Too many recipients"
    );
    assert_eq!(
        client.command("RCPT TO:<sales@example.com> NOTIFY=NEVER").await,
        "555 5.5.4 Unsupported option: NOTIFY=NEVER"
    );
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client
        .send("Subject: Hi\r\n\r\n..leading dot\r\nbare\nline\n.\nstill data\r\n.\r\n")
        .await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok: stored as "));
    {
        let delivered = fake.delivered.lock().unwrap();
        assert_eq!(delivered.len(), 1);
        let message = &delivered[0];
        assert_eq!(
            String::from_utf8_lossy(&message.raw),
            "Subject: Hi\r\n\r\n.leading dot\r\nbare\nline\n.\nstill data\r\n"
        );
        assert_eq!(message.envelope.mail_from, "a@x.org");
        assert_eq!(message.envelope.helo, "gw.example.com");
        assert_eq!(message.envelope.client_ip, "127.0.0.1");
        assert_eq!(message.envelope.protocol, "ESMTP");
        assert_eq!(message.recipients.len(), 1, "one account");
        assert_eq!(message.recipients[0].email_account_id, "sales@example.com");
        assert_eq!(
            message.recipients[0].addresses,
            ["Sales@example.com", "sales@example.com"]
        );
    }
    // pipelined, a store that fails, then too large, then no header fields
    client
        .send("MAIL FROM:<>\r\nRCPT TO:<fail@example.com>\r\nDATA\r\n")
        .await;
    assert_eq!(client.reply().await, "250 2.1.0 Ok");
    assert_eq!(client.reply().await, "250 2.1.5 Ok");
    assert_eq!(client.reply().await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("Subject: x\r\n\r\nbody\r\n.\r\n").await;
    assert_eq!(client.reply().await, "451 4.3.0 Message not stored, try again later");
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client
        .send(&format!("Subject: big\r\n\r\n{}\r\n.\r\n", "x".repeat(2000)))
        .await;
    assert_eq!(
        client.reply().await,
        "552 5.3.4 Message size exceeds fixed maximum message size"
    );
    assert_eq!(client.command("DATA").await, "503 5.5.1 Need MAIL command");
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RSET").await, "250 2.0.0 Ok");
    assert_eq!(client.command("DATA").await, "503 5.5.1 Need MAIL command");
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("DATA").await, "554 5.5.1 No valid recipients");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("no header\r\n.\r\n").await;
    assert_eq!(client.reply().await, "554 5.6.0 the message has no header fields");
    assert_eq!(
        client.command("AUTH PLAIN abc").await,
        "502 5.5.1 Command not implemented"
    );
    assert_eq!(client.command("STARTTLS").await, "502 5.5.1 Command not implemented");
    assert_eq!(client.command("VRFY sales").await, "252 2.5.0 Cannot VRFY user");
    assert_eq!(client.command("NOOP").await, "250 2.0.0 Ok");
    assert_eq!(client.command("QUIT").await, "221 2.0.0 Bye");
    assert_eq!(client.reply().await, "");
    assert_eq!(
        fake.delivered.lock().unwrap().len(),
        3,
        "the 552 message never reached the store"
    );
}

#[tokio::test]
async fn addresses_outside_the_allow_list_are_refused_at_connect() {
    let (address, fake, _stop) = start(config("10.0.0.0/8, ::1")).await;
    let mut client = Client::connect(address).await;
    assert_eq!(
        client.reply().await,
        "554 5.7.1 mx.test does not take mail from this address"
    );
    assert_eq!(client.reply().await, "");
    assert!(fake.delivered.lock().unwrap().is_empty());
    // 127.0.0.2 is not 127.0.0.1; macOS routes only 127.0.0.1 unless lo0 has an alias
    let (address, _fake, _stop) = start(config("127.0.0.1/32")).await;
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    match socket.bind("127.0.0.2:0".parse().unwrap()) {
        Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => {
            eprintln!("skipping the 127.0.0.2 connect: this host does not route 127.0.0.2 ({err})");
        }
        bound => {
            bound.unwrap();
            let mut other = Client::from(socket.connect(address).await.unwrap());
            assert_eq!(
                other.reply().await,
                "554 5.7.1 mx.test does not take mail from this address"
            );
        }
    }
    let mut allowed = Client::connect(address).await;
    assert_eq!(allowed.reply().await, "220 mx.test ESMTP mr-moqui");
}

#[tokio::test]
async fn connections_over_the_limit_wait_and_idle_ones_time_out() {
    let mut short = config("127.0.0.0/8");
    short.command_timeout = Duration::from_millis(300);
    let (address, _fake, stop) = start(short).await;
    let mut one = Client::connect(address).await;
    let mut two = Client::connect(address).await;
    assert_eq!(one.reply().await, "220 mx.test ESMTP mr-moqui");
    assert_eq!(two.reply().await, "220 mx.test ESMTP mr-moqui");
    let mut three = Client::connect(address).await;
    assert_eq!(three.reply().await, "421 4.7.0 Too many connections, try again later");
    assert_eq!(one.reply().await, "421 4.4.2 Timeout, closing the connection");
    assert_eq!(one.reply().await, "");
    drop(two);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut four = Client::connect(address).await;
    assert_eq!(four.reply().await, "220 mx.test ESMTP mr-moqui");
    // stopping answers the session in progress
    stop.send(true).unwrap();
    assert_eq!(four.command("NOOP").await, "421 4.3.2 Server stopping, try again later");
}

#[tokio::test]
async fn a_proxy_header_names_the_client_which_must_be_allowed_too() {
    let mut proxied = config("127.0.0.1, 192.0.2.10");
    proxied.proxy_protocol = true;
    proxied.max_connections = 10;
    let (address, fake, _stop) = start(proxied).await;
    let mut v1 = Client::connect(address).await;
    v1.send("PROXY TCP4 192.0.2.10 127.0.0.1 40000 25\r\n").await;
    assert_eq!(v1.reply().await, "220 mx.test ESMTP mr-moqui");
    v1.command("EHLO gw").await;
    v1.command("MAIL FROM:<a@x.org>").await;
    v1.command("RCPT TO:<sales@example.com>").await;
    v1.command("DATA").await;
    v1.send("Subject: proxied\r\n\r\nhi\r\n.\r\n").await;
    assert!(v1.reply().await.starts_with("250 2.0.0"));
    assert_eq!(fake.delivered.lock().unwrap()[0].envelope.client_ip, "192.0.2.10");
    let mut stranger = Client::connect(address).await;
    stranger.send("PROXY TCP4 198.51.100.7 127.0.0.1 40000 25\r\n").await;
    assert_eq!(
        stranger.reply().await,
        "554 5.7.1 mx.test does not take mail from this address"
    );
    let mut v2 = Client::connect(address).await;
    let mut header = crate::proxy_header::V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x21, 0x11, 0, 12, 192, 0, 2, 10, 127, 0, 0, 1, 0x9C, 0x40, 0, 25]);
    v2.send_bytes(&header).await;
    assert_eq!(v2.reply().await, "220 mx.test ESMTP mr-moqui");
    let mut local = Client::connect(address).await;
    let mut header = crate::proxy_header::V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x20, 0x00, 0, 0]);
    local.send_bytes(&header).await;
    assert_eq!(local.reply().await, "220 mx.test ESMTP mr-moqui");
    let mut none = Client::connect(address).await;
    none.send("EHLO gw\r\n").await;
    assert_eq!(none.reply().await, "");
}

/// A STARTTLS acceptor on a generated `localhost` certificate in a runtime directory of its own, and a client
/// that trusts it.
fn tls_pair(label: &str) -> (TlsAcceptor, tokio_rustls::TlsConnector, std::path::PathBuf) {
    let runtime = crate::tls::tests::temp_dir(label);
    crate::tls::ensure_dev_cert(&runtime.join("cert")).unwrap();
    let server = crate::tls::smtp_server_config(&runtime, "localhost", "", "").unwrap();
    let connector = crate::tls::tests::client_trusting(&runtime.join("cert").join("localhost.crt"));
    (TlsAcceptor::from(Arc::new(server)), connector, runtime)
}

#[tokio::test]
async fn starttls_starts_the_session_over_and_require_tls_waits_for_it() {
    let (acceptor, connector, runtime) = tls_pair("smtp-starttls");
    let mut secure = config("127.0.0.1");
    secure.starttls = true;
    secure.require_tls = true;
    let (address, fake, _stop) = start_tls(secure, Some(acceptor)).await;
    let mut client = Client::connect(address).await;
    assert_eq!(client.reply().await, "220 mx.test ESMTP mr-moqui");
    assert_eq!(
        client.command("EHLO gw").await,
        "250-mx.test|250-PIPELINING|250-SIZE 1000|250-8BITMIME|250-SMTPUTF8|250-STARTTLS|250-CHUNKING|250 ENHANCEDSTATUSCODES"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org>").await,
        "530 5.7.0 Must issue a STARTTLS command first"
    );
    assert_eq!(client.command("STARTTLS now").await, "501 5.5.4 Syntax: STARTTLS");
    assert_eq!(client.command("STARTTLS").await, "220 2.0.0 Ready to start TLS");
    client.upgrade(&connector).await;
    assert_eq!(
        client.command("MAIL FROM:<a@x.org>").await,
        "503 5.5.1 Send EHLO first",
        "the session starts over"
    );
    assert_eq!(
        client.command("EHLO gw").await,
        "250-mx.test|250-PIPELINING|250-SIZE 1000|250-8BITMIME|250-SMTPUTF8|250-CHUNKING|250 ENHANCEDSTATUSCODES",
        "no STARTTLS after it"
    );
    assert_eq!(client.command("STARTTLS").await, "503 5.5.1 TLS already active");
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("Subject: secure\r\n\r\nhi\r\n.\r\n").await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok: stored as "));
    assert_eq!(client.command("MAIL FROM:<a@x.org> SMTPUTF8").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("Subject: ünïcode\r\n\r\nhi\r\n.\r\n").await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok: stored as "));
    assert_eq!(client.command("QUIT").await, "221 2.0.0 Bye");
    {
        let delivered = fake.delivered.lock().unwrap();
        assert_eq!(delivered[0].envelope.protocol, "ESMTPS");
        assert_eq!(delivered[1].envelope.protocol, "UTF8SMTPS");
    }
    // a command pipelined after STARTTLS would run as if it came over TLS: the session ends instead
    let mut eager = Client::connect(address).await;
    eager.reply().await;
    eager.command("EHLO gw").await;
    eager.send("STARTTLS\r\nMAIL FROM:<a@x.org>\r\n").await;
    assert_eq!(
        eager.reply().await,
        "554 5.5.0 Commands pipelined after STARTTLS, closing the connection"
    );
    assert_eq!(eager.reply().await, "");
    assert_eq!(fake.delivered.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(runtime);
}

#[test]
fn starttls_with_no_certificate_does_not_start() {
    let runtime = crate::tls::tests::temp_dir("smtp-no-cert");
    let err = crate::tls::smtp_server_config(&runtime, "mx.test", "", "").unwrap_err();
    assert!(
        err.to_string().contains("no certificate for mx.test or default"),
        "{err}"
    );
    crate::tls::ensure_dev_cert(&runtime.join("dev")).unwrap();
    assert!(
        crate::tls::smtp_server_config(&runtime, "mx.test", "dev/localhost.crt", "").is_err(),
        "the files go together"
    );
    assert!(crate::tls::smtp_server_config(&runtime, "mx.test", "dev/none.crt", "dev/none.key").is_err());
    let config = crate::tls::smtp_server_config(
        &runtime,
        "mx.test",
        "runtime://dev/localhost.crt",
        &runtime.join("dev/localhost.key").display().to_string(),
    )
    .unwrap();
    assert!(config.alpn_protocols.is_empty());
    let _ = std::fs::remove_dir_all(runtime);
}

#[tokio::test]
async fn lmtp_answers_each_recipient_by_its_account() {
    let mut lmtp = config("127.0.0.1");
    lmtp.lmtp = true;
    lmtp.max_recipients = 10;
    let (address, fake, _stop) = start(lmtp).await;
    let mut client = Client::connect(address).await;
    assert_eq!(client.reply().await, "220 mx.test LMTP mr-moqui");
    assert_eq!(
        client.command("EHLO gw").await,
        "500 5.5.1 This is an LMTP server: use LHLO"
    );
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "503 5.5.1 Send LHLO first");
    assert_eq!(
        client.command("LHLO gw").await,
        "250-mx.test|250-PIPELINING|250-SIZE 1000|250-8BITMIME|250-SMTPUTF8|250-CHUNKING|250 ENHANCEDSTATUSCODES"
    );
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    for to in ["sales", "fail", "gone", "help"] {
        assert_eq!(
            client.command(&format!("RCPT TO:<{to}@example.com>")).await,
            "250 2.1.5 Ok"
        );
    }
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("Subject: x\r\n\r\nhi\r\n.\r\n").await;
    let sales = client.reply().await;
    assert!(
        sales.starts_with("250 2.1.5 <sales@example.com> Ok: stored as "),
        "{sales}"
    );
    assert_eq!(
        client.reply().await,
        "451 4.3.0 <fail@example.com>: Message not stored, try again later"
    );
    assert_eq!(client.reply().await, "550 5.1.1 <gone@example.com>: Mailbox is gone");
    let help = client.reply().await;
    assert!(
        help.starts_with("250 2.1.5 <help@example.com> Ok: stored as "),
        "{help}"
    );
    {
        let delivered = fake.delivered.lock().unwrap();
        assert_eq!(delivered[0].envelope.protocol, "LMTP");
        let accounts: Vec<&str> = delivered[0]
            .recipients
            .iter()
            .map(|group| group.email_account_id.as_str())
            .collect();
        assert_eq!(accounts, ["sales@example.com", "fail@example.com", "gone@example.com"]);
        assert_eq!(
            delivered[0].recipients[0].addresses,
            ["sales@example.com", "help@example.com"]
        );
    }
    // a whole-message answer is given once for each recipient
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("RCPT TO:<gone@example.com>").await, "250 2.1.5 Ok");
    client.send(&format!("BDAT 1001 LAST\r\n{}", "x".repeat(1001))).await;
    for _ in 0..2 {
        assert_eq!(
            client.reply().await,
            "552 5.3.4 Message size exceeds fixed maximum message size"
        );
    }
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("RCPT TO:<help@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("no header\r\n.\r\n").await;
    for _ in 0..2 {
        assert_eq!(client.reply().await, "554 5.6.0 the message has no header fields");
    }
    assert_eq!(client.command("QUIT").await, "221 2.0.0 Bye");
}

#[tokio::test]
async fn bdat_chunks_are_taken_as_they_are() {
    let (address, fake, _stop) = start(config("127.0.0.1")).await;
    let mut client = Client::connect(address).await;
    client.reply().await;
    client.command("EHLO gw").await;
    client.send("BDAT 3 LAST\r\nabc").await;
    assert_eq!(client.reply().await, "503 5.5.1 Need MAIL command");
    assert_eq!(client.command("NOOP").await, "250 2.0.0 Ok", "the chunk was read past");
    assert_eq!(client.command("BDAT x").await, "501 5.5.4 Syntax: BDAT size [LAST]");
    assert_eq!(
        client.command("BDAT 1 FIRST").await,
        "501 5.5.4 Syntax: BDAT size [LAST]"
    );
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    let first = "Subject: chunks\r\n\r\n.lead\r\n";
    client.send(&format!("BDAT {}\r\n{first}", first.len())).await;
    assert_eq!(
        client.reply().await,
        format!("250 2.0.0 {} octets received", first.len())
    );
    assert_eq!(client.command("DATA").await, "503 5.5.1 DATA after BDAT");
    let second = "x\r\n.\r\nafter the dot line\r\n";
    client.send(&format!("BDAT {} last\r\n{second}", second.len())).await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok: stored as "));
    assert_eq!(
        String::from_utf8_lossy(&fake.delivered.lock().unwrap()[0].raw),
        format!("{first}{second}"),
        "no dot-unstuffing, and a dot line does not end it"
    );
    // over the limit: every chunk is read, and the last is answered 552
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    client.send(&format!("BDAT 600\r\n{}", "x".repeat(600))).await;
    assert_eq!(client.reply().await, "250 2.0.0 600 octets received");
    client.send(&format!("BDAT 600 LAST\r\n{}", "x".repeat(600))).await;
    assert_eq!(
        client.reply().await,
        "552 5.3.4 Message size exceeds fixed maximum message size"
    );
    assert_eq!(client.command("NOOP").await, "250 2.0.0 Ok");
    // an empty last chunk ends the message
    assert_eq!(client.command("MAIL FROM:<a@x.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    let whole = "Subject: z\r\n\r\nz\r\n";
    client.send(&format!("BDAT {}\r\n{whole}", whole.len())).await;
    client.reply().await;
    assert!(client
        .command("BDAT 0 LAST")
        .await
        .starts_with("250 2.0.0 Ok: stored as "));
    assert_eq!(
        fake.delivered.lock().unwrap().len(),
        2,
        "the 552 message never reached the store"
    );
    assert_eq!(client.command("QUIT").await, "221 2.0.0 Bye");
    // CHUNKING off
    let mut plain = config("127.0.0.1");
    plain.chunking = false;
    let (address, _fake, _stop) = start(plain).await;
    let mut client = Client::connect(address).await;
    client.reply().await;
    assert!(!client.command("EHLO gw").await.contains("CHUNKING"));
    assert_eq!(client.command("BDAT 0 LAST").await, "502 5.5.1 Command not implemented");
}

#[tokio::test]
async fn dsn_parameters_are_checked_and_kept() {
    let mut dsn = config("127.0.0.1");
    dsn.dsn = true;
    dsn.max_recipients = 10;
    let (address, fake, _stop) = start(dsn).await;
    let mut client = Client::connect(address).await;
    client.reply().await;
    assert!(client.command("EHLO gw").await.contains("|250-DSN|"));
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> RET=BOGUS").await,
        "501 5.5.4 Bad RET parameter"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> ENVID=bad+zz").await,
        "501 5.5.4 Bad ENVID parameter"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> ENVID=a=b").await,
        "501 5.5.4 Bad ENVID parameter"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> RET=hdrs RET=FULL").await,
        "501 5.5.4 Duplicate RET parameter"
    );
    assert_eq!(
        client.command("MAIL FROM:<a@x.org> RET=hdrs ENVID=QQ+2B1").await,
        "250 2.1.0 Ok"
    );
    assert_eq!(
        client.command("RCPT TO:<sales@example.com> NOTIFY=NEVER,SUCCESS").await,
        "501 5.5.4 Bad NOTIFY parameter"
    );
    assert_eq!(
        client
            .command("RCPT TO:<sales@example.com> NOTIFY=SUCCESS,SUCCESS")
            .await,
        "501 5.5.4 Bad NOTIFY parameter"
    );
    assert_eq!(
        client.command("RCPT TO:<sales@example.com> ORCPT=rfc822").await,
        "501 5.5.4 Bad ORCPT parameter"
    );
    assert_eq!(
        client
            .command("RCPT TO:<sales@example.com> NOTIFY=DELAY NOTIFY=FAILURE")
            .await,
        "501 5.5.4 Duplicate NOTIFY parameter"
    );
    assert_eq!(
        client
            .command("RCPT TO:<sales@example.com> NOTIFY=success,failure ORCPT=rfc822;Sales+2Bnews@example.com")
            .await,
        "250 2.1.5 Ok"
    );
    assert_eq!(client.command("RCPT TO:<help@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client.send("Subject: dsn\r\n\r\nhi\r\n.\r\n").await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok: stored as "));
    let delivered = fake.delivered.lock().unwrap();
    let message = &delivered[0];
    assert_eq!(message.envelope.dsn_ret, "HDRS");
    assert_eq!(message.envelope.dsn_envid, "QQ+1");
    assert_eq!(
        message.recipients[0].addresses,
        ["sales@example.com", "help@example.com"]
    );
    assert_eq!(
        message.recipients[0].dsn,
        [RecipientDsn {
            address: "sales@example.com".to_string(),
            notify: "SUCCESS,FAILURE".to_string(),
            original: "rfc822;Sales+news@example.com".to_string(),
        }]
    );
}

#[test]
fn protocols_and_dsn_values_parse() {
    assert_eq!(received_protocol(false, false, false, false), "SMTP");
    assert_eq!(received_protocol(false, true, false, false), "ESMTP");
    assert_eq!(received_protocol(false, true, false, true), "ESMTPS");
    assert_eq!(received_protocol(false, true, true, false), "UTF8SMTP");
    assert_eq!(received_protocol(true, true, false, false), "LMTP");
    assert_eq!(received_protocol(true, true, true, true), "UTF8LMTPS");
    assert_eq!(xtext("a+2Bb+3Dc", false).as_deref(), Some("a+b=c"));
    assert_eq!(xtext("a+2b", false), None, "upper-case hex only");
    assert_eq!(xtext("a+0A", false), None, "no control characters");
    assert_eq!(xtext("a b", false), None);
    assert_eq!(xtext("ü@x.org", false), None);
    assert_eq!(xtext("ü@x.org", true).as_deref(), Some("ü@x.org"));
    assert_eq!(parse_notify("never").as_deref(), Some("NEVER"));
    assert_eq!(parse_notify("Delay,success").as_deref(), Some("DELAY,SUCCESS"));
    assert_eq!(parse_notify(""), None);
    assert_eq!(parse_notify("SUCCESS,"), None);
    assert_eq!(parse_orcpt("RFC822;a+2Bb@x.org").as_deref(), Some("rfc822;a+b@x.org"));
    assert_eq!(parse_orcpt("utf-8;ü@x.org").as_deref(), Some("utf-8;ü@x.org"));
    assert_eq!(parse_orcpt(";a@x.org"), None);
    assert_eq!(parse_orcpt("rfc822;"), None);
}

#[test]
fn allow_list_entries_and_paths_parse() {
    let net = AllowedNet::parse("10.1.0.0/16").unwrap();
    assert!(net.contains("10.1.200.3".parse().unwrap()));
    assert!(net.contains("::ffff:10.1.0.9".parse().unwrap()));
    assert!(!net.contains("10.2.0.1".parse().unwrap()));
    assert!(AllowedNet::parse("0.0.0.0/0")
        .unwrap()
        .contains("8.8.8.8".parse().unwrap()));
    assert!(AllowedNet::parse("fd00::/8")
        .unwrap()
        .contains("fd12::1".parse().unwrap()));
    assert!(AllowedNet::parse("[::1]").unwrap().contains("::1".parse().unwrap()));
    assert!(AllowedNet::parse("10.0.0.0/33").is_err());
    assert!(AllowedNet::parse("gateway.example.com").is_err());
    assert_eq!(parse_path("<a@x.org> SIZE=1"), Some(("a@x.org".to_string(), " SIZE=1")));
    assert_eq!(parse_path("<>"), Some((String::new(), "")));
    assert_eq!(
        parse_path("<\"odd>name\"@x.org>"),
        Some(("\"odd>name\"@x.org".to_string(), ""))
    );
    assert_eq!(parse_path("a@x.org"), None);
    assert_eq!(parse_path("<a@x.org>junk"), None);
    assert_eq!(strip_keyword("from: <a@x.org>", "FROM:"), Some("<a@x.org>"));
}

/// A fake Rspamd: `/checkv2` answers "add header" for a message whose text has `Buy now`, "reject" for `Wire money`,
/// "no action" for the rest.
async fn fake_rspamd() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut length = 0_usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0_u8; length];
                tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
                    .await
                    .unwrap();
                let text = String::from_utf8_lossy(&body);
                let action = if text.contains("Buy now") {
                    "add header"
                } else if text.contains("Wire money") {
                    "reject"
                } else {
                    "no action"
                };
                let json = format!(r#"{{"action":"{action}","score":7.5,"required_score":6.0}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
                let _ = reader.get_mut().write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{address}")
}

/// Internet mode (E5b): any address, its own Authentication-Results (a forged one with its authserv-id dropped), the
/// spam fields its own (Rspamd's verdict), a DMARC reject refused with 550, and the per-address connection limit.
#[tokio::test]
async fn internet_mode_checks_the_mail_and_limits_each_address() {
    let records = moqui_mail::dns::Records::offline();
    records.fix_txt("example.org", "v=spf1 ip4:127.0.0.1 -all").unwrap();
    records.fix_txt("gw.example.org", "v=spf1 ip4:127.0.0.1 -all").unwrap();
    records.fix_txt("reject.test", "v=spf1 -all").unwrap();
    records.fix_txt("_dmarc.reject.test", "v=DMARC1; p=reject").unwrap();
    let mut config = config("10.9.9.9");
    config.allowed = Vec::new();
    config.max_message = 100_000;
    config.authserv_id = "mx.test".to_string();
    config.spam_headers = vec!["X-Spam-Flag".to_string(), "X-Spam-Status".to_string()];
    config.internet = Some(Internet {
        max_per_ip: 1,
        per_minute: 30,
        rspamd_url: fake_rspamd().await,
        spam_fields: config.spam_headers.clone(),
        records: Some(Arc::new(records)),
    });
    let (address, fake, _stop) = start(config).await;
    let mut client = Client::connect(address).await;
    assert_eq!(client.reply().await, "220 mx.test ESMTP mr-moqui");
    // a second connection from the same address while this one is open: over max-connections-per-ip
    let mut second = Client::connect(address).await;
    assert_eq!(
        second.reply().await,
        "421 4.7.0 Too many connections from your address, try again later"
    );
    assert!(client.command("EHLO gw.example.org").await.starts_with("250-mx.test"));
    let send = |from: &'static str, body: &'static str| (from, body);
    for (from, body) in [send("ann@example.org", "Hello"), send("ann@example.org", "Buy now")] {
        assert_eq!(client.command(&format!("MAIL FROM:<{from}>")).await, "250 2.1.0 Ok");
        assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
        assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
        client
            .send(&format!(
                "Authentication-Results: MX.test; dkim=pass\r\nAuthentication-Results: other.example; spf=pass\r\n\
                 X-Spam-Flag: NO\r\nFrom: {from}\r\nSubject: Hi\r\n\r\n{body}\r\n.\r\n"
            ))
            .await;
        assert!(client.reply().await.starts_with("250 2.0.0 Ok"), "{body}");
    }
    {
        let delivered = fake.delivered.lock().unwrap();
        let first = String::from_utf8_lossy(&delivered[0].raw).to_string();
        assert!(first.starts_with("Authentication-Results: mx.test;"), "{first}");
        assert!(first.contains("spf=pass") && first.contains("dmarc=none"), "{first}");
        assert!(
            first.contains("\r\nX-Spam-Flag: NO\r\nX-Spam-Status: No, score=7.5 required=6\r\n"),
            "{first}"
        );
        assert!(
            !first.contains("MX.test; dkim=pass"),
            "the forged result is dropped: {first}"
        );
        assert!(
            first.contains("Authentication-Results: other.example; spf=pass"),
            "{first}"
        );
        assert_eq!(first.matches("X-Spam-Flag").count(), 1, "{first}");
        let second = String::from_utf8_lossy(&delivered[1].raw).to_string();
        assert!(
            second.contains("\r\nX-Spam-Flag: YES\r\nX-Spam-Status: Yes, score=7.5 required=6\r\n"),
            "{second}"
        );
    }
    // Rspamd's reject, and a DMARC reject policy that fails: 550 after the data, nothing delivered
    for (from, body, reply) in [
        ("ann@example.org", "Wire money", "550 5.7.1 Rejected as spam"),
        (
            "x@reject.test",
            "Hello",
            "550 5.7.1 Rejected by the DMARC policy of reject.test",
        ),
    ] {
        assert_eq!(client.command(&format!("MAIL FROM:<{from}>")).await, "250 2.1.0 Ok");
        assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
        assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
        client
            .send(&format!("From: {from}\r\nSubject: Hi\r\n\r\n{body}\r\n.\r\n"))
            .await;
        assert_eq!(client.reply().await, reply);
    }
    assert_eq!(fake.delivered.lock().unwrap().len(), 2);
    assert_eq!(client.command("QUIT").await, "221 2.0.0 Bye");
}

/// Internet mode with Rspamd not reached: the mail is taken (fail open, logged), as not spam.
#[tokio::test]
async fn internet_mode_takes_the_mail_when_rspamd_is_not_reached() {
    let records = moqui_mail::dns::Records::offline();
    records.fix_txt("example.org", "v=spf1 ip4:127.0.0.1 -all").unwrap();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut config = config("10.9.9.9");
    config.allowed = Vec::new();
    config.authserv_id = "mx.test".to_string();
    config.internet = Some(Internet {
        max_per_ip: 5,
        per_minute: 30,
        rspamd_url: format!("http://{closed}"),
        spam_fields: vec!["X-Spam-Flag".to_string()],
        records: Some(Arc::new(records)),
    });
    let (address, fake, _stop) = start(config).await;
    let mut client = Client::connect(address).await;
    assert_eq!(client.reply().await, "220 mx.test ESMTP mr-moqui");
    assert!(client.command("EHLO gw.example.org").await.starts_with("250-mx.test"));
    assert_eq!(client.command("MAIL FROM:<ann@example.org>").await, "250 2.1.0 Ok");
    assert_eq!(client.command("RCPT TO:<sales@example.com>").await, "250 2.1.5 Ok");
    assert_eq!(client.command("DATA").await, "354 End data with <CR><LF>.<CR><LF>");
    client
        .send("From: ann@example.org\r\nSubject: Hi\r\n\r\nHello\r\n.\r\n")
        .await;
    assert!(client.reply().await.starts_with("250 2.0.0 Ok"));
    let delivered = String::from_utf8_lossy(&fake.delivered.lock().unwrap()[0].raw).to_string();
    assert!(delivered.contains("\r\nX-Spam-Flag: NO\r\n"), "{delivered}");
}

#[test]
fn ipv6_addresses_are_limited_by_their_64() {
    let a: IpAddr = "2001:db8:1:2::5".parse().unwrap();
    let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
    let c: IpAddr = "2001:db8:1:3::5".parse().unwrap();
    assert_eq!(limit_key(a), limit_key(b));
    assert_ne!(limit_key(a), limit_key(c));
    let v4: IpAddr = "192.0.2.7".parse().unwrap();
    assert_eq!(limit_key(v4), v4);
    assert_eq!(limit_key("::ffff:192.0.2.7".parse().unwrap()), v4);
}
