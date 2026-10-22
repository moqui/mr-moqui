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

//! The inbound SMTP listener for hosted and shared mail accounts, behind the edge gateway (the `inbound-smtp` feature;
//! docs/Mail-Inbound.md). Off unless `smtp_listen_port` is set; it listens on `smtp_listen_host` (default `127.0.0.1`).
//!
//! It is the last hop of a gateway (Postfix with Rspamd, or another MTA) that does the internet side: SPF, DKIM, DMARC,
//! spam and virus filtering, queueing and retries. So it is a small RFC 5321 server for that one client: EHLO or HELO,
//! MAIL FROM (SIZE, BODY=8BITMIME, SMTPUTF8), RCPT TO, DATA, RSET, NOOP, QUIT, VRFY (252), HELP, with PIPELINING and
//! ENHANCEDSTATUSCODES; no AUTH, no relay. A connection from an address outside `smtp_allowed_addresses` (IPs and
//! CIDRs) gets 554 at connect and is closed; with `smtp-server` `proxy-protocol` each connection starts with a PROXY v1
//! or v2 header and the address it names must be allowed too. An empty allow-list does not start the listener.
//!
//! The `smtp-server` options:
//! - `protocol="lmtp"` (RFC 2033): LHLO instead of EHLO, and after the data one reply per accepted recipient, by how
//!   its account took the message (stored 250, gone 550, failed 451), so a store that fails for one account does not
//!   make the gateway send the message to all of them again.
//! - `starttls` (RFC 3207), with `require-tls` refusing MAIL before it (530). The certificate is `tls-cert-file` and
//!   `tls-key-file`, else `runtime/cert` by the name asked for or `hostname` (`tls.rs` `smtp_server_config`); with
//!   none the listener does not start. Bytes pipelined after STARTTLS end the session (554), the session starts over
//!   after the handshake, and STARTTLS is not offered again.
//! - `chunking` (RFC 3030, on): `BDAT n [LAST]`, the chunk read as it is (no dot-stuffing); DATA after BDAT is 503.
//! - `dsn` (RFC 3461, off): RET and ENVID on MAIL, NOTIFY and ORCPT on RCPT, checked (501 when they do not parse) and
//!   kept on the envelope; delivery adds `Original-Recipient` and sends a "delivered" report for NOTIFY=SUCCESS.
//!
//! The Received field's `with` is the protocol spoken: SMTP, ESMTP, ESMTPS, LMTP, LMTPS, UTF8SMTP, UTF8LMTPS and so on.
//!
//! `RCPT TO` is answered from the accounts ([`moqui_core::service::mail_inbound::find_recipient`]): an address of a
//! hosted or shared account is accepted, any other refused with 550 there, so nothing is accepted that would bounce
//! later. After `DATA` the message is stored once per account ([`moqui_core::service::mail_inbound::deliver`]); a store
//! that fails, or no request worker free, answers 451 so the gateway tries again, and the accounts that took it the
//! first time do not store it twice. Both run on a request worker with authorization disabled.
//!
//! Limits: `smtp-server` `max-message-size` bytes (advertised as SIZE; a larger message is read to its end and refused with
//! 552), `smtp-server` `max-recipients` a message (452 after), `smtp-server` `max-connections` at once (421 after), `smtp-server` `command-timeout`
//! seconds for each command line or data chunk, `smtp-server` `session-timeout` seconds for a whole connection. Each session's
//! outcome is logged, with its addresses and sizes, never the message.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::service::mail_inbound::{self as inbound, Envelope, Options, Outcome, RecipientDsn, Recipients};
use moqui_lib::error::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The longest command line, with its CRLF. RFC 5321 asks for 512; extensions make it longer.
const MAX_COMMAND: usize = 4096;
/// A data line is read in chunks of at most this many bytes.
const DATA_CHUNK: usize = 64 * 1024;
/// Commands answered with a syntax or sequence error before the connection is closed (an unknown recipient is not one).
const MAX_ERRORS: usize = 20;
/// The PROXY v2 signature.
const PROXY_V2: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

/// A connection's bytes: TCP, then TLS after STARTTLS.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// The address the listener is on, kept on the factory for `Server::smtp_address`.
#[derive(Default)]
pub struct SmtpAddress(pub std::sync::Mutex<Option<SocketAddr>>);

/// An allowed address or network: `10.0.0.5`, `10.0.0.0/8`, `fd00::/8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllowedNet {
    network: IpAddr,
    prefix: u8,
}

impl AllowedNet {
    /// # Errors
    ///
    /// Returns why when `text` is not an address or a network.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let network: IpAddr = address
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| format!("{text} is not an IP address or network"))?;
        let network = network.to_canonical();
        let width = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= width)
                .ok_or_else(|| format!("{text} has no valid prefix length"))?,
            None => width,
        };
        Ok(Self { network, prefix })
    }

    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

/// The listener's settings, from the `smtp_*` properties.
#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    /// The name in the greeting, EHLO and the Received field.
    pub hostname: String,
    pub allowed: Vec<AllowedNet>,
    pub max_message: usize,
    pub max_recipients: usize,
    pub max_connections: usize,
    pub command_timeout: Duration,
    pub session_timeout: Duration,
    pub proxy_protocol: bool,
    /// The plus-addressing characters; empty for none.
    pub plus_separator: String,
    pub spam_headers: Vec<String>,
    pub authserv_id: String,
    /// LMTP (RFC 2033) instead of SMTP: LHLO, and a reply per recipient after the data.
    pub lmtp: bool,
    /// STARTTLS is offered.
    pub starttls: bool,
    /// MAIL before STARTTLS is refused.
    pub require_tls: bool,
    /// The certificate and key files; both empty for `runtime/cert`.
    pub tls_cert_file: String,
    pub tls_key_file: String,
    /// CHUNKING (RFC 3030, BDAT).
    pub chunking: bool,
    /// DSN (RFC 3461).
    pub dsn: bool,
}

fn property(name: &str) -> String {
    moqui_lib::env::get_prop_or_env(name).trim().to_string()
}

impl SmtpConfig {
    /// The settings of the conf's `smtp-server` when its `listen-port` (`smtp_listen_port`) is set, else none. An
    /// attribute the conf leaves out, or that expands to nothing, is its property when it has one.
    ///
    /// # Errors
    ///
    /// Returns why the listener cannot start: a port that does not parse, an allow-list that is empty or does not
    /// parse, a protocol other than smtp or lmtp, `require-tls` without `starttls`.
    pub fn from_conf(factory: &ExecutionContextFactory) -> Result<Option<Self>, Error> {
        let setting = |name: &str| factory.conf_setting("smtp-server", name);
        let setting_or_property = |name: &str, property_name: &str| {
            Some(setting(name))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| property(property_name))
        };
        let number = |name: &str, default: u64| {
            setting(name)
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        let count = |name: &str, default: usize| usize::try_from(number(name, default as u64)).unwrap_or(default);
        let flag = |name: &str, default: bool| match setting(name).trim().to_ascii_lowercase().as_str() {
            "true" | "y" | "yes" | "1" => true,
            "false" | "n" | "no" | "0" => false,
            _ => default,
        };
        let port_text = setting_or_property("listen-port", "smtp_listen_port");
        if port_text.is_empty() {
            return Ok(None);
        }
        let port: u16 = port_text
            .parse()
            .map_err(|_| Error::new(format!("smtp_listen_port {port_text} is not a port number")))?;
        let allowed_text = setting_or_property("allowed-addresses", "smtp_allowed_addresses");
        let allowed = allowed_text
            .split([',', ' '])
            .filter(|entry| !entry.trim().is_empty())
            .map(AllowedNet::parse)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|text| Error::new(format!("smtp_allowed_addresses: {text}")))?;
        if allowed.is_empty() {
            return Err(Error::new(
                "smtp_allowed_addresses is empty: name the edge gateway's addresses (127.0.0.1 for a gateway on this host)",
            ));
        }
        let hostname = match setting_or_property("hostname", "smtp_hostname") {
            name if !name.is_empty() => name,
            _ => match property("webapp_http_host") {
                name if !name.is_empty() => name,
                _ => "localhost".to_string(),
            },
        };
        let host = match setting_or_property("listen-host", "smtp_listen_host") {
            host if !host.is_empty() => host,
            _ => "127.0.0.1".to_string(),
        };
        let lmtp = match setting("protocol").trim().to_ascii_lowercase().as_str() {
            "" | "smtp" => false,
            "lmtp" => true,
            other => return Err(Error::new(format!("smtp-server protocol {other} is not smtp or lmtp"))),
        };
        let starttls = flag("starttls", false);
        let require_tls = flag("require-tls", false);
        if require_tls && !starttls {
            return Err(Error::new(
                "smtp-server require-tls is on but starttls is off: no client could send mail",
            ));
        }
        let list = |name: &str| -> Vec<String> {
            setting(name)
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        };
        Ok(Some(Self {
            host,
            port,
            hostname,
            allowed,
            max_message: count("max-message-size", 25 * 1024 * 1024),
            max_recipients: count("max-recipients", 100),
            max_connections: count("max-connections", 20),
            command_timeout: Duration::from_secs(number("command-timeout", 300)),
            session_timeout: Duration::from_secs(number("session-timeout", 1800)),
            proxy_protocol: flag("proxy-protocol", false),
            plus_separator: match setting("plus-separator") {
                none if none.eq_ignore_ascii_case("none") => String::new(),
                characters => characters
                    .chars()
                    .filter(|ch| !ch.is_whitespace() && *ch != '@')
                    .collect(),
            },
            spam_headers: list("spam-headers"),
            authserv_id: setting_or_property("authserv-id", "smtp_authserv_id"),
            lmtp,
            starttls,
            require_tls,
            tls_cert_file: setting_or_property("tls-cert-file", "smtp_tls_cert_file"),
            tls_key_file: setting_or_property("tls-key-file", "smtp_tls_key_file"),
            chunking: flag("chunking", true),
            dsn: flag("dsn", false),
        }))
    }

    fn allows(&self, address: IpAddr) -> bool {
        self.allowed.iter().any(|net| net.contains(address))
    }
}

/// A running listener. Drop stops it; a session in progress is answered 421 at its next command.
pub struct SmtpServer {
    address: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl SmtpServer {
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for SmtpServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Start the listener when `smtp_listen_port` is set (`0` binds an ephemeral port).
///
/// # Errors
///
/// Returns an error when the settings or the bind fail, or STARTTLS is on with no certificate; the host logs it and
/// starts without the listener.
pub fn start(factory: &Arc<ExecutionContextFactory>) -> Result<Option<SmtpServer>, Error> {
    let Some(config) = SmtpConfig::from_conf(factory)? else {
        return Ok(None);
    };
    let tls = if config.starttls {
        let server_config = crate::tls::smtp_server_config(
            factory.runtime_dir(),
            &config.hostname,
            &config.tls_cert_file,
            &config.tls_key_file,
        )
        .map_err(|err| Error::new(format!("the SMTP listener has starttls on, but {err}")))?;
        Some(TlsAcceptor::from(Arc::new(server_config)))
    } else {
        None
    };
    let bind = if config.host.contains(':') && !config.host.starts_with('[') {
        format!("[{}]:{}", config.host, config.port)
    } else {
        format!("{}:{}", config.host, config.port)
    };
    let bind_text = bind.clone();
    let listener = crate::edge::block_on(async move { TcpListener::bind(bind).await })?
        .map_err(|err| Error::new(format!("could not bind the SMTP listener to {bind_text}: {err}")))?;
    let address = listener
        .local_addr()
        .map_err(|err| Error::new("the SMTP listener has no address").with_source(err))?;
    if config.authserv_id.is_empty() {
        tracing::warn!(
            "smtp_authserv_id is empty: Authentication-Results fields are kept as they came, the gateway's not told \
             from a forged one"
        );
    }
    let backend = FactoryBackend {
        factory: Arc::downgrade(factory),
        delimiters: config.plus_separator.clone(),
        options: Options {
            spam_headers: config.spam_headers.clone(),
            authserv_id: config.authserv_id.clone(),
        },
    };
    let (shutdown, watch) = tokio::sync::watch::channel(false);
    tracing::info!(
        "{} listener on {address} for hosted and shared accounts, from {}{}{}",
        if config.lmtp { "LMTP" } else { "SMTP" },
        config
            .allowed
            .iter()
            .map(|net| format!("{}/{}", net.network, net.prefix))
            .collect::<Vec<_>>()
            .join(", "),
        if config.proxy_protocol { " through PROXY" } else { "" },
        match (config.starttls, config.require_tls) {
            (true, true) => ", STARTTLS required",
            (true, false) => ", STARTTLS offered",
            _ => "",
        }
    );
    crate::edge::handle().spawn(serve(listener, config, tls, backend, watch));
    *factory
        .extension(|_| SmtpAddress::default())
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(address);
    Ok(Some(SmtpServer { address, shutdown }))
}

/// The answer to `RCPT TO`.
pub(crate) enum RcptAnswer {
    /// The account it goes to.
    Local(String),
    Unknown,
    TempFail(String),
}

/// What became of a message in one account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountResult {
    /// Stored, or it had it already.
    Stored,
    /// The account was removed after `RCPT TO`.
    Gone,
    /// The store failed: 451, and the gateway tries again.
    Failed,
}

/// The answer after `DATA` or the last `BDAT`.
pub(crate) enum DataAnswer {
    /// Delivered to the accounts, each with its result; the text is for the session log.
    Stored {
        accounts: Vec<(String, AccountResult)>,
        summary: String,
    },
    /// Not delivered at all (no request worker free): 451 for every recipient.
    TempFail(String),
    Rejected(String),
}

/// A message after `DATA`.
pub(crate) struct Message {
    pub(crate) raw: Vec<u8>,
    pub(crate) envelope: Envelope,
    pub(crate) recipients: Vec<Recipients>,
}

/// Where the listener finds accounts and stores mail; the factory's, or a test's.
pub(crate) trait Backend: Send + Sync + 'static {
    fn recipient(&self, address: String) -> impl Future<Output = RcptAnswer> + Send;
    fn deliver(&self, message: Message) -> impl Future<Output = DataAnswer> + Send;
}

struct FactoryBackend {
    factory: Weak<ExecutionContextFactory>,
    delimiters: String,
    options: Options,
}

impl Backend for FactoryBackend {
    async fn recipient(&self, address: String) -> RcptAnswer {
        let delimiters = self.delimiters.clone();
        let found = crate::ssh::on_pool_without_authz(&self.factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| CoreError::new("the entity facade is not ready"))?;
            inbound::find_recipient(&facade, &address, &delimiters).map_err(CoreError::from)
        })
        .await;
        match found {
            Ok(Some(account)) => RcptAnswer::Local(account),
            Ok(None) => RcptAnswer::Unknown,
            Err(err) => RcptAnswer::TempFail(err.message().to_string()),
        }
    }

    async fn deliver(&self, message: Message) -> DataAnswer {
        let options = self.options.clone();
        let delivered = crate::ssh::on_pool_without_authz(&self.factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| CoreError::new("the entity facade is not ready"))?;
            Ok(inbound::deliver(
                &facade,
                &message.raw,
                &message.envelope,
                &message.recipients,
                &options,
            ))
        })
        .await;
        match delivered {
            Err(err) => DataAnswer::TempFail(err.message().to_string()),
            Ok(Err(text)) => DataAnswer::Rejected(text),
            Ok(Ok(report)) => {
                let accounts: Vec<String> = report
                    .deliveries
                    .iter()
                    .map(|delivery| match &delivery.result {
                        Ok(Outcome::Stored { email_message_id, junk }) => format!(
                            "account {} stored it as {email_message_id}{}",
                            delivery.email_account_id,
                            if *junk { " in Junk" } else { "" }
                        ),
                        Ok(Outcome::Duplicate) => format!("account {} had it", delivery.email_account_id),
                        Ok(Outcome::Gone) => format!("account {} is gone", delivery.email_account_id),
                        Err(_) => format!("account {} failed", delivery.email_account_id),
                    })
                    .collect();
                let summary = format!(
                    "{}; spam {}; {}",
                    accounts.join(", "),
                    if report.spam { "yes" } else { "no" },
                    if report.authentication.is_empty() {
                        "no Authentication-Results"
                    } else {
                        report.authentication.as_str()
                    }
                );
                let accounts = report
                    .deliveries
                    .iter()
                    .map(|delivery| {
                        let result = match &delivery.result {
                            Ok(Outcome::Stored { .. } | Outcome::Duplicate) => AccountResult::Stored,
                            Ok(Outcome::Gone) => AccountResult::Gone,
                            Err(_) => AccountResult::Failed,
                        };
                        (delivery.email_account_id.clone(), result)
                    })
                    .collect();
                DataAnswer::Stored { accounts, summary }
            }
        }
    }
}

struct Shared<B> {
    config: SmtpConfig,
    tls: Option<TlsAcceptor>,
    backend: B,
    active: AtomicUsize,
    sessions: AtomicU64,
}

/// A place among `smtp-server` `max-connections`, given back on drop.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accept connections until `shutdown` says stop. `tls` is there when STARTTLS is on.
pub(crate) async fn serve<B: Backend>(
    listener: TcpListener,
    config: SmtpConfig,
    tls: Option<TlsAcceptor>,
    backend: B,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let shared = Arc::new(Shared {
        config,
        tls,
        backend,
        active: AtomicUsize::new(0),
        sessions: AtomicU64::new(0),
    });
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let shared = Arc::clone(&shared);
                    let shutdown = shutdown.clone();
                    tokio::spawn(async move {
                        connection(Box::new(stream), peer, &shared, shutdown).await;
                    });
                }
                Err(err) => {
                    tracing::warn!("SMTP accept failed: {err}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    tracing::info!("SMTP listener stopped");
}

fn session_id(shared: &Shared<impl Backend>) -> String {
    let number = shared.sessions.fetch_add(1, Ordering::Relaxed);
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    format!("{seconds:X}{:04X}", number & 0xFFFF)
}

async fn connection<B: Backend>(
    mut stream: Box<dyn Io>,
    peer: SocketAddr,
    shared: &Shared<B>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let id = session_id(shared);
    let peer_ip = peer.ip().to_canonical();
    let config = &shared.config;
    if !config.allows(peer_ip) {
        tracing::warn!("SMTP connection {id} from {peer_ip} refused: not in smtp_allowed_addresses");
        let _ = reply_now(
            &mut stream,
            config.command_timeout,
            &format!("554 5.7.1 {} does not take mail from this address", config.hostname),
        )
        .await;
        let _ = stream.shutdown().await;
        return;
    }
    if shared.active.fetch_add(1, Ordering::SeqCst) >= config.max_connections {
        shared.active.fetch_sub(1, Ordering::SeqCst);
        tracing::warn!("SMTP connection {id} from {peer_ip} refused: smtp-server max-connections are open");
        let _ = reply_now(
            &mut stream,
            config.command_timeout,
            "421 4.7.0 Too many connections, try again later",
        )
        .await;
        let _ = stream.shutdown().await;
        return;
    }
    let _slot = Slot(&shared.active);
    let started = Instant::now();
    let mut session = Session {
        shared,
        id,
        client_ip: peer_ip,
        stream: BufReader::new(stream),
        shutdown,
        deadline: started + config.session_timeout,
        helo: None,
        esmtp: false,
        tls: false,
        transaction: None,
        errors: 0,
        messages: 0,
        accepted: 0,
        refused: 0,
    };
    let end = session.run().await;
    let _ = session.stream.get_mut().shutdown().await;
    let level_error = matches!(end, End::Io(_) | End::Tls(_) | End::Pipelining);
    let text = format!(
        "{} session {} from {}{} ({}{}): {} messages, {} recipients accepted, {} refused, {} in {}ms",
        if config.lmtp { "LMTP" } else { "SMTP" },
        session.id,
        session.client_ip,
        if session.client_ip == peer_ip {
            String::new()
        } else {
            format!(" through {peer_ip}")
        },
        session.helo.as_deref().unwrap_or("no EHLO"),
        if session.tls { ", TLS" } else { "" },
        session.messages,
        session.accepted,
        session.refused,
        end.text(),
        started.elapsed().as_millis()
    );
    if level_error {
        tracing::warn!("{text}");
    } else {
        tracing::info!("{text}");
    }
}

/// Write one reply now, within `wait`; several lines are joined with CRLF.
async fn reply_now<W: AsyncWrite + Unpin + ?Sized>(writer: &mut W, wait: Duration, line: &str) -> Result<(), End> {
    let mut bytes = Vec::with_capacity(line.len() + 2);
    bytes.extend_from_slice(line.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    match tokio::time::timeout(wait, writer.write_all(&bytes)).await {
        Err(_) => Err(End::Timeout),
        Ok(Err(err)) => Err(End::Io(err.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

/// Why a session ended.
enum End {
    Quit,
    Closed,
    Timeout,
    Shutdown,
    TooLong,
    TooManyErrors,
    /// Bytes after STARTTLS, before the handshake.
    Pipelining,
    Tls(String),
    Proxy(String),
    Io(String),
}

impl End {
    fn text(&self) -> String {
        match self {
            Self::Quit => "quit".to_string(),
            Self::Closed => "closed by the client".to_string(),
            Self::Timeout => "timed out".to_string(),
            Self::Shutdown => "server stopping".to_string(),
            Self::TooLong => "line too long".to_string(),
            Self::TooManyErrors => "too many errors".to_string(),
            Self::Pipelining => "refused: commands pipelined after STARTTLS".to_string(),
            Self::Tls(text) => format!("TLS handshake failed: {text}"),
            Self::Proxy(text) => format!("refused: {text}"),
            Self::Io(text) => format!("connection error: {text}"),
        }
    }
}

/// A mail transaction, from `MAIL FROM`.
struct Transaction {
    mail_from: String,
    utf8: bool,
    /// The DSN `RET` and `ENVID` (decoded), empty for none.
    ret: String,
    envid: String,
    /// Each accepted `RCPT TO`.
    recipients: Vec<Rcpt>,
    /// The `BDAT` chunks so far; `Some` from the first, so `DATA` is refused after it.
    chunks: Option<Vec<u8>>,
    /// A chunk went over `max-message-size`: the others are read and dropped, and the last is answered 552.
    too_big: bool,
}

/// An accepted recipient.
struct Rcpt {
    address: String,
    /// The account it goes to.
    account: String,
    dsn: Option<RecipientDsn>,
}

struct Session<'a, B> {
    shared: &'a Shared<B>,
    id: String,
    client_ip: IpAddr,
    /// The connection, which STARTTLS replaces with its TLS stream.
    stream: BufReader<Box<dyn Io>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    deadline: Instant,
    helo: Option<String>,
    esmtp: bool,
    tls: bool,
    transaction: Option<Transaction>,
    errors: usize,
    messages: usize,
    accepted: usize,
    refused: usize,
}

impl<B: Backend> Session<'_, B> {
    fn config(&self) -> &SmtpConfig {
        &self.shared.config
    }

    /// The time left for one read: the command timeout, within the session's.
    fn wait(&self) -> Result<Duration, End> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(End::Timeout);
        }
        Ok(left.min(self.config().command_timeout))
    }

    async fn reply(&mut self, line: &str) -> Result<(), End> {
        let wait = self.wait()?;
        reply_now(self.stream.get_mut(), wait, line).await
    }

    /// A reply that counts as an error; too many of them close the connection.
    async fn error(&mut self, line: &str) -> Result<(), End> {
        self.errors += 1;
        if self.errors >= MAX_ERRORS {
            self.reply("421 4.7.0 Too many errors, closing the connection").await?;
            return Err(End::TooManyErrors);
        }
        self.reply(line).await
    }

    /// Read up to and with the next LF, at most `limit` bytes, into `buf`.
    async fn read_line(&mut self, limit: usize, buf: &mut Vec<u8>) -> Result<(), End> {
        buf.clear();
        let wait = self.wait()?;
        let mut limited = (&mut self.stream).take(limit as u64);
        let read = limited.read_until(b'\n', buf);
        tokio::select! {
            changed = self.shutdown.changed() => {
                let _ = changed;
                Err(End::Shutdown)
            }
            result = tokio::time::timeout(wait, read) => match result {
                Err(_) => Err(End::Timeout),
                Ok(Err(err)) => Err(End::Io(err.to_string())),
                Ok(Ok(0)) => Err(End::Closed),
                Ok(Ok(_)) => Ok(()),
            }
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), End> {
        let wait = self.wait()?;
        match tokio::time::timeout(wait, self.stream.read_exact(buf)).await {
            Err(_) => Err(End::Timeout),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => Err(End::Closed),
            Ok(Err(err)) => Err(End::Io(err.to_string())),
            Ok(Ok(_)) => Ok(()),
        }
    }

    async fn run(&mut self) -> End {
        match self.session().await {
            Ok(()) => End::Quit,
            Err(End::Timeout) => {
                let _ = reply_now(
                    self.stream.get_mut(),
                    Duration::from_secs(5),
                    "421 4.4.2 Timeout, closing the connection",
                )
                .await;
                End::Timeout
            }
            Err(End::Shutdown) => {
                let _ = reply_now(
                    self.stream.get_mut(),
                    Duration::from_secs(5),
                    "421 4.3.2 Server stopping, try again later",
                )
                .await;
                End::Shutdown
            }
            Err(end) => end,
        }
    }

    async fn session(&mut self) -> Result<(), End> {
        if self.config().proxy_protocol {
            let source = self.read_proxy().await?;
            if let Some(source) = source {
                let source = source.to_canonical();
                if !self.config().allows(source) {
                    let _ = self
                        .reply(&format!(
                            "554 5.7.1 {} does not take mail from this address",
                            self.config().hostname
                        ))
                        .await;
                    return Err(End::Proxy(format!("{source} is not in smtp_allowed_addresses")));
                }
                self.client_ip = source;
            }
        }
        let lmtp = self.config().lmtp;
        let greeting = format!(
            "220 {} {} mr-moqui",
            self.config().hostname,
            if lmtp { "LMTP" } else { "ESMTP" }
        );
        self.reply(&greeting).await?;
        let chunking = self.config().chunking;
        let starttls = self.shared.tls.is_some();
        let mut buf = Vec::with_capacity(512);
        loop {
            self.read_line(MAX_COMMAND, &mut buf).await?;
            if !buf.ends_with(b"\n") {
                self.reply("500 5.5.2 Line too long").await?;
                return Err(End::TooLong);
            }
            let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
            let (verb, rest) = match line.split_once(' ') {
                Some((verb, rest)) => (verb.to_ascii_uppercase(), rest.trim().to_string()),
                None => (line.trim().to_ascii_uppercase(), String::new()),
            };
            match verb.as_str() {
                "EHLO" | "HELO" if !lmtp => self.hello(&verb, &rest).await?,
                "LHLO" if lmtp => self.hello(&verb, &rest).await?,
                "EHLO" | "HELO" => self.error("500 5.5.1 This is an LMTP server: use LHLO").await?,
                "MAIL" => self.mail(&rest).await?,
                "RCPT" => self.rcpt(&rest).await?,
                "DATA" => self.data(&rest).await?,
                "BDAT" if chunking => self.bdat(&rest).await?,
                "STARTTLS" if starttls => self.starttls(&rest).await?,
                "RSET" => {
                    self.transaction = None;
                    self.reply("250 2.0.0 Ok").await?;
                }
                "NOOP" => self.reply("250 2.0.0 Ok").await?,
                "QUIT" => {
                    self.reply("221 2.0.0 Bye").await?;
                    return Ok(());
                }
                "VRFY" => self.reply("252 2.5.0 Cannot VRFY user").await?,
                "HELP" => self.reply("214 2.0.0 See RFC 5321").await?,
                "STARTTLS" | "AUTH" | "BDAT" | "EXPN" | "ETRN" | "TURN" | "ATRN" => {
                    self.error("502 5.5.1 Command not implemented").await?;
                }
                _ => self.error("500 5.5.2 Command not recognized").await?,
            }
        }
    }

    /// The PROXY v1 or v2 header: the client's address, or none for a proxy's own connection (LOCAL, UNKNOWN).
    async fn read_proxy(&mut self) -> Result<Option<IpAddr>, End> {
        let mut start = [0_u8; 6];
        self.read_exact(&mut start).await?;
        if &start == b"PROXY " {
            let mut line = Vec::with_capacity(108);
            self.read_line(102, &mut line).await?;
            if !line.ends_with(b"\r\n") {
                return Err(End::Proxy("a PROXY v1 header too long or not ended".to_string()));
            }
            let text = String::from_utf8_lossy(&line);
            let parts: Vec<&str> = text.split_whitespace().collect();
            return match parts.as_slice() {
                ["UNKNOWN", ..] => Ok(None),
                ["TCP4" | "TCP6", source, _, _, _] => source
                    .parse::<IpAddr>()
                    .map(Some)
                    .map_err(|_| End::Proxy(format!("PROXY v1 source {source} is not an address"))),
                _ => Err(End::Proxy("a PROXY v1 header that does not parse".to_string())),
            };
        }
        if start != PROXY_V2[..6] {
            return Err(End::Proxy("no PROXY header".to_string()));
        }
        let mut rest = [0_u8; 10];
        self.read_exact(&mut rest).await?;
        if rest[..6] != PROXY_V2[6..] || rest[6] >> 4 != 2 {
            return Err(End::Proxy("a PROXY v2 header that does not parse".to_string()));
        }
        let command = rest[6] & 0x0F;
        let family = rest[7] >> 4;
        let length = usize::from(u16::from_be_bytes([rest[8], rest[9]]));
        if length > 4096 {
            return Err(End::Proxy("a PROXY v2 header too long".to_string()));
        }
        let mut addresses = vec![0_u8; length];
        self.read_exact(&mut addresses).await?;
        if command == 0 {
            return Ok(None);
        }
        match family {
            1 if length >= 12 => Ok(Some(IpAddr::from([
                addresses[0],
                addresses[1],
                addresses[2],
                addresses[3],
            ]))),
            2 if length >= 36 => {
                let mut octets = [0_u8; 16];
                octets.copy_from_slice(&addresses[..16]);
                Ok(Some(IpAddr::from(octets)))
            }
            1 | 2 => Err(End::Proxy("a PROXY v2 address block too short".to_string())),
            _ => Ok(None),
        }
    }

    async fn hello(&mut self, verb: &str, rest: &str) -> Result<(), End> {
        let name = rest.split_whitespace().next().unwrap_or_default();
        if name.is_empty() {
            return self.error(&format!("501 5.5.4 Syntax: {verb} hostname")).await;
        }
        self.helo = Some(name.chars().take(255).collect());
        self.esmtp = verb != "HELO";
        self.transaction = None;
        let hostname = self.config().hostname.clone();
        if !self.esmtp {
            return self.reply(&format!("250 {hostname}")).await;
        }
        let size = self.config().max_message;
        let mut lines = vec![
            format!("250-{hostname}"),
            "250-PIPELINING".to_string(),
            format!("250-SIZE {size}"),
            "250-8BITMIME".to_string(),
            "250-SMTPUTF8".to_string(),
        ];
        if self.shared.tls.is_some() && !self.tls {
            lines.push("250-STARTTLS".to_string());
        }
        if self.config().chunking {
            lines.push("250-CHUNKING".to_string());
        }
        if self.config().dsn {
            lines.push("250-DSN".to_string());
        }
        lines.push("250 ENHANCEDSTATUSCODES".to_string());
        self.reply(&lines.join("\r\n")).await
    }

    /// STARTTLS: refuse bytes the client sent after it (they would be read as commands after the handshake, from
    /// before it: CVE-2011-0411), shake hands, and start the session over on the TLS stream.
    async fn starttls(&mut self, rest: &str) -> Result<(), End> {
        if !rest.is_empty() {
            return self.error("501 5.5.4 Syntax: STARTTLS").await;
        }
        if self.tls {
            return self.error("503 5.5.1 TLS already active").await;
        }
        let Some(acceptor) = self.shared.tls.clone() else {
            return self.error("502 5.5.1 Command not implemented").await;
        };
        if !self.stream.buffer().is_empty() {
            self.reply("554 5.5.0 Commands pipelined after STARTTLS, closing the connection")
                .await?;
            return Err(End::Pipelining);
        }
        self.reply("220 2.0.0 Ready to start TLS").await?;
        let wait = self.wait()?;
        let placeholder: Box<dyn Io> = Box::new(tokio::io::duplex(1).0);
        let plain = std::mem::replace(&mut self.stream, BufReader::new(placeholder)).into_inner();
        match tokio::time::timeout(wait, acceptor.accept(plain)).await {
            Err(_) => return Err(End::Tls("timed out".to_string())),
            Ok(Err(err)) => return Err(End::Tls(err.to_string())),
            Ok(Ok(secured)) => self.stream = BufReader::new(Box::new(secured)),
        }
        self.tls = true;
        self.helo = None;
        self.esmtp = false;
        self.transaction = None;
        Ok(())
    }

    async fn mail(&mut self, rest: &str) -> Result<(), End> {
        if self.helo.is_none() {
            let hello = if self.config().lmtp { "LHLO" } else { "EHLO" };
            return self.error(&format!("503 5.5.1 Send {hello} first")).await;
        }
        if self.config().require_tls && !self.tls {
            return self.error("530 5.7.0 Must issue a STARTTLS command first").await;
        }
        if self.transaction.is_some() {
            return self.error("503 5.5.1 Nested MAIL command").await;
        }
        let Some(path) = strip_keyword(rest, "FROM:") else {
            return self.error("501 5.5.4 Syntax: MAIL FROM:<address>").await;
        };
        let Some((address, parameters)) = parse_path(path) else {
            return self.error("501 5.1.7 Bad sender address syntax").await;
        };
        let dsn = self.esmtp && self.config().dsn;
        let mut utf8 = false;
        let mut ret = String::new();
        let mut envid = String::new();
        let mut seen: Vec<String> = Vec::new();
        for parameter in parameters.split_whitespace() {
            let (key, value) = parameter.split_once('=').unwrap_or((parameter, ""));
            let key = key.to_ascii_uppercase();
            if seen.contains(&key) {
                return self.error(&format!("501 5.5.4 Duplicate {key} parameter")).await;
            }
            seen.push(key.clone());
            match key.as_str() {
                "SIZE" if self.esmtp => {
                    let Ok(size) = value.parse::<u64>() else {
                        return self.error("501 5.5.4 Bad SIZE parameter").await;
                    };
                    if size > self.config().max_message as u64 {
                        return self
                            .error("552 5.3.4 Message size exceeds fixed maximum message size")
                            .await;
                    }
                }
                "BODY" if self.esmtp && matches!(value.to_ascii_uppercase().as_str(), "7BIT" | "8BITMIME") => {}
                "SMTPUTF8" if self.esmtp && value.is_empty() => utf8 = true,
                "RET" if dsn => match value.to_ascii_uppercase().as_str() {
                    word @ ("FULL" | "HDRS") => ret = word.to_string(),
                    _ => return self.error("501 5.5.4 Bad RET parameter").await,
                },
                "ENVID" if dsn => match xtext(value, false).filter(|text| !text.is_empty() && value.len() <= 100) {
                    Some(text) => envid = text,
                    None => return self.error("501 5.5.4 Bad ENVID parameter").await,
                },
                _ => {
                    return self
                        .error(&format!("555 5.5.4 Unsupported option: {}", clip(parameter)))
                        .await
                }
            }
        }
        self.transaction = Some(Transaction {
            mail_from: address,
            utf8,
            ret,
            envid,
            recipients: Vec::new(),
            chunks: None,
            too_big: false,
        });
        self.reply("250 2.1.0 Ok").await
    }

    async fn rcpt(&mut self, rest: &str) -> Result<(), End> {
        let Some(count) = self
            .transaction
            .as_ref()
            .map(|transaction| transaction.recipients.len())
        else {
            return self.error("503 5.5.1 Need MAIL command").await;
        };
        let Some(path) = strip_keyword(rest, "TO:") else {
            return self.error("501 5.5.4 Syntax: RCPT TO:<address>").await;
        };
        let Some((address, parameters)) = parse_path(path) else {
            return self.error("501 5.1.3 Bad recipient address syntax").await;
        };
        let dsn_on = self.esmtp && self.config().dsn;
        let mut dsn = RecipientDsn::default();
        let mut seen: Vec<String> = Vec::new();
        for parameter in parameters.split_whitespace() {
            let (key, value) = parameter.split_once('=').unwrap_or((parameter, ""));
            let key = key.to_ascii_uppercase();
            if seen.contains(&key) {
                return self.error(&format!("501 5.5.4 Duplicate {key} parameter")).await;
            }
            seen.push(key.clone());
            match key.as_str() {
                "NOTIFY" if dsn_on => match parse_notify(value) {
                    Some(notify) => dsn.notify = notify,
                    None => return self.error("501 5.5.4 Bad NOTIFY parameter").await,
                },
                "ORCPT" if dsn_on => match parse_orcpt(value) {
                    Some(original) => dsn.original = original,
                    None => return self.error("501 5.5.4 Bad ORCPT parameter").await,
                },
                _ => {
                    return self
                        .error(&format!("555 5.5.4 Unsupported option: {}", clip(parameter)))
                        .await
                }
            }
        }
        let well_formed = address
            .rsplit_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty());
        if !well_formed && !address.eq_ignore_ascii_case("postmaster") {
            return self.error("501 5.1.3 Bad recipient address syntax").await;
        }
        if count >= self.config().max_recipients {
            self.refused += 1;
            return self.reply("452 4.5.3 Too many recipients").await;
        }
        let answer = if well_formed {
            self.shared.backend.recipient(address.clone()).await
        } else {
            RcptAnswer::Unknown
        };
        match answer {
            RcptAnswer::Local(account) => {
                let dsn = (!seen.is_empty()).then(|| RecipientDsn {
                    address: address.clone(),
                    ..dsn
                });
                if let Some(transaction) = self.transaction.as_mut() {
                    transaction.recipients.push(Rcpt { address, account, dsn });
                }
                self.accepted += 1;
                self.reply("250 2.1.5 Ok").await
            }
            RcptAnswer::Unknown => {
                self.refused += 1;
                tracing::info!(
                    "SMTP session {} refused recipient {}: no hosted or shared account has it",
                    self.id,
                    clip(&address)
                );
                self.reply(&format!(
                    "550 5.1.1 <{}>: Recipient address rejected: User unknown",
                    clip(&address)
                ))
                .await
            }
            RcptAnswer::TempFail(text) => {
                self.refused += 1;
                tracing::error!(
                    "SMTP session {} could not look up recipient {}: {text}",
                    self.id,
                    clip(&address)
                );
                self.reply(&format!(
                    "451 4.3.0 <{}>: Temporary lookup failure, try again later",
                    clip(&address)
                ))
                .await
            }
        }
    }

    async fn data(&mut self, rest: &str) -> Result<(), End> {
        if !rest.is_empty() {
            return self.error("501 5.5.4 Syntax: DATA").await;
        }
        let Some(transaction) = self.transaction.as_ref() else {
            return self.error("503 5.5.1 Need MAIL command").await;
        };
        if transaction.chunks.is_some() {
            return self.error("503 5.5.1 DATA after BDAT").await;
        }
        if transaction.recipients.is_empty() {
            return self.error("554 5.5.1 No valid recipients").await;
        }
        self.reply("354 End data with <CR><LF>.<CR><LF>").await?;
        let raw = self.read_data().await?;
        let Some(transaction) = self.transaction.take() else {
            return self.reply("503 5.5.1 Need MAIL command").await;
        };
        self.finish_message(transaction, raw).await
    }

    /// `BDAT size [LAST]`: the chunk is read whatever the answer, so the next command is where the client expects.
    async fn bdat(&mut self, rest: &str) -> Result<(), End> {
        let mut words = rest.split_whitespace();
        let size = words
            .next()
            .filter(|word| word.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|word| word.parse::<u64>().ok());
        let last = match words.next() {
            None => Some(false),
            Some(word) if word.eq_ignore_ascii_case("LAST") => Some(true),
            Some(_) => None,
        };
        let (Some(size), Some(last), None) = (size, last, words.next()) else {
            return self.error("501 5.5.4 Syntax: BDAT size [LAST]").await;
        };
        let refusal = match self.transaction.as_ref() {
            None => Some("503 5.5.1 Need MAIL command"),
            Some(transaction) if transaction.recipients.is_empty() => Some("554 5.5.1 No valid recipients"),
            Some(_) => None,
        };
        self.read_chunk(size, refusal.is_none()).await?;
        if let Some(line) = refusal {
            return self.error(line).await;
        }
        if !last {
            return self.reply(&format!("250 2.0.0 {size} octets received")).await;
        }
        let Some(mut transaction) = self.transaction.take() else {
            return self.reply("503 5.5.1 Need MAIL command").await;
        };
        let raw = if transaction.too_big {
            None
        } else {
            Some(transaction.chunks.take().unwrap_or_default())
        };
        self.finish_message(transaction, raw).await
    }

    /// Read a `BDAT` chunk of `size` bytes as it is, onto the transaction's when `keep` (dropped once over the limit).
    async fn read_chunk(&mut self, size: u64, keep: bool) -> Result<(), End> {
        let max = self.config().max_message;
        if keep {
            if let Some(transaction) = self.transaction.as_mut() {
                transaction.chunks.get_or_insert_with(Vec::new);
            }
        }
        let mut piece = vec![0_u8; usize::try_from(size).map_or(DATA_CHUNK, |size| size.min(DATA_CHUNK))];
        let mut left = size;
        while left > 0 {
            let length = usize::try_from(left).map_or(piece.len(), |left| left.min(piece.len()));
            self.read_exact(&mut piece[..length]).await?;
            left -= length as u64;
            let Some(transaction) = self.transaction.as_mut().filter(|_| keep) else {
                continue;
            };
            if transaction.too_big {
                continue;
            }
            let chunks = transaction.chunks.get_or_insert_with(Vec::new);
            if chunks.len() + length > max {
                transaction.too_big = true;
                transaction.chunks = Some(Vec::new());
            } else {
                chunks.extend_from_slice(&piece[..length]);
            }
        }
        Ok(())
    }

    /// One reply, or in LMTP the same reply for each recipient.
    async fn reply_all(&mut self, transaction: &Transaction, line: &str) -> Result<(), End> {
        if !self.config().lmtp {
            return self.reply(line).await;
        }
        let lines = vec![line; transaction.recipients.len().max(1)];
        self.reply(&lines.join("\r\n")).await
    }

    /// The end of `DATA` or the last `BDAT`: store the message (none when it was over the limit) and answer.
    async fn finish_message(&mut self, transaction: Transaction, raw: Option<Vec<u8>>) -> Result<(), End> {
        let Some(raw) = raw else {
            tracing::warn!(
                "SMTP session {} refused a message from <{}>: larger than smtp-server max-message-size ({} bytes)",
                self.id,
                clip(&transaction.mail_from),
                self.config().max_message
            );
            return self
                .reply_all(
                    &transaction,
                    "552 5.3.4 Message size exceeds fixed maximum message size",
                )
                .await;
        };
        let size = raw.len();
        let mut grouped: Vec<Recipients> = Vec::new();
        for rcpt in &transaction.recipients {
            let group = match grouped.iter().position(|group| group.email_account_id == rcpt.account) {
                Some(at) => &mut grouped[at],
                None => {
                    grouped.push(Recipients {
                        email_account_id: rcpt.account.clone(),
                        ..Recipients::default()
                    });
                    let last = grouped.len() - 1;
                    &mut grouped[last]
                }
            };
            if !group.addresses.contains(&rcpt.address) {
                group.addresses.push(rcpt.address.clone());
            }
            if let Some(dsn) = &rcpt.dsn {
                if !group.dsn.iter().any(|seen| seen.address == dsn.address) {
                    group.dsn.push(dsn.clone());
                }
            }
        }
        let protocol = received_protocol(self.config().lmtp, self.esmtp, transaction.utf8, self.tls);
        let message = Message {
            raw,
            envelope: Envelope {
                mail_from: transaction.mail_from.clone(),
                helo: self.helo.clone().unwrap_or_default(),
                client_ip: self.client_ip.to_string(),
                protocol,
                session_id: self.id.clone(),
                hostname: self.config().hostname.clone(),
                dsn_ret: transaction.ret.clone(),
                dsn_envid: transaction.envid.clone(),
            },
            recipients: grouped,
        };
        let answer = self.shared.backend.deliver(message).await;
        let recipients = transaction
            .recipients
            .iter()
            .map(|rcpt| rcpt.address.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        match answer {
            DataAnswer::Stored { accounts, summary } => {
                let result = |account: &str| {
                    accounts
                        .iter()
                        .find(|(id, _)| id == account)
                        .map_or(AccountResult::Failed, |(_, result)| *result)
                };
                let failed: Vec<&str> = transaction
                    .recipients
                    .iter()
                    .filter(|rcpt| result(&rcpt.account) == AccountResult::Failed)
                    .map(|rcpt| rcpt.address.as_str())
                    .collect();
                let lmtp = self.config().lmtp;
                if !failed.is_empty() {
                    tracing::error!(
                        "SMTP session {} could not store a message of {size} bytes from <{}> for {}, answered 451{}: \
                         {summary}",
                        self.id,
                        clip(&transaction.mail_from),
                        failed.join(", "),
                        if lmtp { " for them" } else { "" }
                    );
                    if !lmtp {
                        return self.reply("451 4.3.0 Message not stored, try again later").await;
                    }
                }
                if failed.len() < transaction.recipients.len() {
                    self.messages += 1;
                    tracing::info!(
                        "SMTP session {} stored a message of {size} bytes from <{}> to {recipients}: {summary}",
                        self.id,
                        clip(&transaction.mail_from),
                    );
                }
                let stored_as = format!("{}.{}", self.id, self.messages);
                if !lmtp {
                    return self.reply(&format!("250 2.0.0 Ok: stored as {stored_as}")).await;
                }
                let lines: Vec<String> = transaction
                    .recipients
                    .iter()
                    .map(|rcpt| {
                        let address = clip(&rcpt.address);
                        match result(&rcpt.account) {
                            AccountResult::Stored => format!("250 2.1.5 <{address}> Ok: stored as {stored_as}"),
                            AccountResult::Gone => format!("550 5.1.1 <{address}>: Mailbox is gone"),
                            AccountResult::Failed => {
                                format!("451 4.3.0 <{address}>: Message not stored, try again later")
                            }
                        }
                    })
                    .collect();
                self.reply(&lines.join("\r\n")).await
            }
            DataAnswer::TempFail(text) => {
                tracing::error!(
                    "SMTP session {} could not store a message of {size} bytes from <{}> to {recipients}, answered \
                     451: {text}",
                    self.id,
                    clip(&transaction.mail_from),
                );
                self.reply_all(&transaction, "451 4.3.0 Message not stored, try again later")
                    .await
            }
            DataAnswer::Rejected(text) => {
                tracing::warn!(
                    "SMTP session {} refused a message of {size} bytes from <{}>: {text}",
                    self.id,
                    clip(&transaction.mail_from)
                );
                self.reply_all(&transaction, &format!("554 5.6.0 {}", clip(&text)))
                    .await
            }
        }
    }

    /// The message up to `CRLF.CRLF`, dot-unstuffed; none when it is larger than the limit (read to its end anyway).
    /// Only a `.` line after a CRLF ends it, so a bare LF cannot end the data early.
    async fn read_data(&mut self) -> Result<Option<Vec<u8>>, End> {
        let max = self.config().max_message;
        let mut out: Vec<u8> = Vec::new();
        let mut too_big = false;
        let mut line = Vec::with_capacity(1024);
        let mut line_start = true;
        loop {
            self.read_line(DATA_CHUNK, &mut line).await?;
            if line_start && line == b".\r\n" {
                break;
            }
            let content = if line_start && line.first() == Some(&b'.') {
                &line[1..]
            } else {
                &line[..]
            };
            if !too_big {
                if out.len() + content.len() > max {
                    too_big = true;
                    out = Vec::new();
                } else {
                    out.extend_from_slice(content);
                }
            }
            line_start = line.ends_with(b"\r\n");
        }
        Ok((!too_big).then_some(out))
    }
}

/// The Received field's `with` (RFC 3848, RFC 6531): SMTP after HELO, ESMTP after EHLO, LMTP; UTF8SMTP and UTF8LMTP
/// for SMTPUTF8; an `S` after STARTTLS.
fn received_protocol(lmtp: bool, esmtp: bool, utf8: bool, tls: bool) -> String {
    let base = match (lmtp, utf8) {
        (true, true) => "UTF8LMTP",
        (true, false) => "LMTP",
        (false, true) => "UTF8SMTP",
        (false, false) if esmtp || tls => "ESMTP",
        (false, false) => "SMTP",
    };
    format!("{base}{}", if tls { "S" } else { "" })
}

/// RFC 3461 xtext, decoded: `+XX` is the byte in upper-case hex, any other character from `!` to `~` but `+` and `=` is
/// itself (and with `utf8`, any non-ASCII character, RFC 6533). None when it does not parse or decodes to control
/// characters or bytes that are not UTF-8.
fn xtext(text: &str, utf8: bool) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '+' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() != 2 || !hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F')) {
                    return None;
                }
                out.push(u8::from_str_radix(&hex, 16).ok()?);
            }
            '!'..='~' if ch != '=' => out.push(ch as u8),
            _ if utf8 && !ch.is_ascii() => out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes()),
            _ => return None,
        }
    }
    String::from_utf8(out)
        .ok()
        .filter(|decoded| !decoded.chars().any(char::is_control))
}

/// `NOTIFY`: `NEVER` alone, or some of `SUCCESS`, `FAILURE` and `DELAY` once each; upper case, joined by commas.
fn parse_notify(value: &str) -> Option<String> {
    let words: Vec<String> = value.split(',').map(str::to_ascii_uppercase).collect();
    if words == ["NEVER"] {
        return Some("NEVER".to_string());
    }
    let mut kept: Vec<String> = Vec::new();
    for word in words {
        if !matches!(word.as_str(), "SUCCESS" | "FAILURE" | "DELAY") || kept.contains(&word) {
            return None;
        }
        kept.push(word);
    }
    Some(kept.join(","))
}

/// `ORCPT=addr-type;xtext`: the type (an atom such as `rfc822` or `utf-8`) and the decoded address, `rfc822;a@x.org`.
fn parse_orcpt(value: &str) -> Option<String> {
    if value.len() > 500 {
        return None;
    }
    let (kind, address) = value.split_once(';')?;
    if kind.is_empty() || !kind.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return None;
    }
    let kind = kind.to_ascii_lowercase();
    let address = xtext(address, kind == "utf-8").filter(|address| !address.is_empty())?;
    Some(format!("{kind};{address}"))
}

/// `rest` after a keyword such as `FROM:`, in any case, with the spaces some clients put after the colon.
fn strip_keyword<'a>(rest: &'a str, keyword: &str) -> Option<&'a str> {
    let head = rest.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    Some(rest[keyword.len()..].trim_start())
}

/// A `<path>` and the parameters after it. The source route (`@a,@b:`) is dropped; a quoted local part may hold `>`.
/// `<>` is the null path. Control characters are not allowed.
fn parse_path(text: &str) -> Option<(String, &str)> {
    let inner = text.strip_prefix('<')?;
    let mut quoted = false;
    let mut escaped = false;
    let mut end = None;
    for (index, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '>' if !quoted => {
                end = Some(index);
                break;
            }
            _ => {}
        }
    }
    let end = end?;
    let mut address = &inner[..end];
    if address.starts_with('@') {
        address = address.split_once(':').map(|(_, rest)| rest)?;
    }
    if address
        .chars()
        .any(|ch| ch.is_control() || (ch == ' ' && !address.contains('"')))
    {
        return None;
    }
    let rest = &inner[end + 1..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((address.to_string(), rest))
}

/// Text from the client for a reply or the log: at most 200 characters.
fn clip(text: &str) -> String {
    text.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
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
        let mut header = PROXY_V2.to_vec();
        header.extend_from_slice(&[0x21, 0x11, 0, 12, 192, 0, 2, 10, 127, 0, 0, 1, 0x9C, 0x40, 0, 25]);
        v2.send_bytes(&header).await;
        assert_eq!(v2.reply().await, "220 mx.test ESMTP mr-moqui");
        let mut local = Client::connect(address).await;
        let mut header = PROXY_V2.to_vec();
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
}
