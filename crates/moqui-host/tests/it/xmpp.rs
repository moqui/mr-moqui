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

//! XMPP for local users on a real server (plans/Comms-Framework.md §7.1 C3a, docs/interfaces/XMPP.md).
//!
//! Ann's client is tokio-xmpp: SCRAM-SHA-256-PLUS with tls-exporter channel binding over direct TLS (RFC 6120 SASL),
//! a room joined, a message to it read by Ben in the web's way (`get#Timeline`) and his web message back, the room's
//! archive paged, a file uploaded over HTTP and fetched back, and an OMEMO device list published. Ben's client is a raw
//! SASL2 stream (tokio-xmpp has no SASL2 or FAST): SCRAM-PLUS with Bind 2, stream management and a FAST token; Ann's
//! device list read over PEP; then his connection drops, Ann writes to him, and he comes back with FAST and resumes the
//! stream inside SASL2, getting what he missed.

#![cfg(all(unix, feature = "embedded-postgres", feature = "xmpp"))]

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::StreamExt;
use moqui_lib::object::{to_plain_string, Value};
use moqui_xmpp::framing::{Frame, StreamReader};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_xmpp::connect::ServerConnector;
use tokio_xmpp::minidom::Element;
use tokio_xmpp::parsers::iq::Iq;
use tokio_xmpp::parsers::message::Message;
use tokio_xmpp::parsers::presence::Presence;
use tokio_xmpp::xmlstream::{initiate_stream, PendingFeaturesRecv, StreamHeader, Timeouts};
use tokio_xmpp::{Client, Event, Stanza};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="XMPP_ANN" username="xmpp.ann" userFullName="Ann Xmpp"/>
    <moqui.security.UserAccount userId="XMPP_BEN" username="xmpp.ben" userFullName="Ben Xmpp"/>
</entity-facade-xml>"#;

const WAIT: Duration = Duration::from_secs(5);

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let mut message = ec.message();
        let errors = message.errors().to_vec();
        message.clear_errors();
        errors
    })
    .unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

fn as_user<T>(
    factory: &Arc<moqui_core::context::ExecutionContextFactory>,
    username: &str,
    body: impl FnOnce() -> T,
) -> T {
    // as the web's screens run them: their transitions grant the services
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user(username).unwrap();
    body()
}

fn get(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

/// Any server certificate: the test's own self-signed one.
#[derive(Debug)]
struct AnyCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

async fn tls(address: SocketAddr) -> (tokio_rustls::client::TlsStream<TcpStream>, Vec<u8>) {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"xmpp-client".to_vec()];
    let tcp = TcpStream::connect(address).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("moqui.local").unwrap();
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .unwrap();
    let exporter = stream
        .get_ref()
        .1
        .export_keying_material(vec![0_u8; 32], b"EXPORTER-Channel-Binding", None)
        .unwrap();
    (stream, exporter)
}

/// tokio-xmpp over direct TLS (XEP-0368) to the test server, with tls-exporter channel binding.
#[derive(Clone, Debug)]
struct DirectTls(SocketAddr);

impl ServerConnector for DirectTls {
    type Stream = tokio::io::BufStream<tokio_rustls::client::TlsStream<TcpStream>>;

    fn connect(
        &self,
        jid: &tokio_xmpp::jid::Jid,
        ns: &'static str,
        timeouts: Timeouts,
    ) -> impl std::future::Future<
        Output = Result<(PendingFeaturesRecv<Self::Stream>, sasl::common::ChannelBinding), tokio_xmpp::Error>,
    > + Send {
        let (address, domain) = (self.0, jid.domain().to_string());
        async move {
            let (stream, exporter) = tls(address).await;
            let header = StreamHeader {
                to: Some(Cow::Owned(domain)),
                from: None,
                id: None,
            };
            let pending = initiate_stream(tokio::io::BufStream::new(stream), ns, header, timeouts).await?;
            Ok((pending, sasl::common::ChannelBinding::TlsExporter(exporter)))
        }
    }
}

fn stanza(xml: &str) -> Stanza {
    let element: Element = xml.parse().unwrap_or_else(|err| panic!("{xml}: {err}"));
    match element.name() {
        "message" => Stanza::Message(Message::try_from(element).unwrap()),
        "presence" => Stanza::Presence(Presence::try_from(element).unwrap()),
        _ => Stanza::Iq(Iq::try_from(element).unwrap()),
    }
}

fn element(stanza: Stanza) -> Element {
    match stanza {
        Stanza::Message(message) => message.into(),
        Stanza::Presence(presence) => presence.into(),
        Stanza::Iq(iq) => iq.into(),
    }
}

/// The next stanza the client gets that `wanted` takes.
async fn next_where(client: &mut Client, wanted: impl Fn(&Element) -> bool) -> Element {
    let until = tokio::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        let Ok(event) = tokio::time::timeout_at(until, client.next()).await else {
            panic!("not in time; got {seen:#?}");
        };
        match event.expect("an event") {
            Event::Stanza(stanza) => {
                let element = element(stanza);
                if wanted(&element) {
                    return element;
                }
                seen.push(String::from(&element));
            }
            other => seen.push(format!("{other:?}")),
        }
    }
}

/// A SASL2 client over raw TLS, reading the server's stream with moqui-xmpp's own reader.
struct Raw {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    reader: StreamReader,
    exporter: Vec<u8>,
    got: Vec<Element>,
}

impl Raw {
    async fn open(address: SocketAddr) -> Raw {
        let (stream, exporter) = tls(address).await;
        let mut raw = Raw {
            stream,
            reader: StreamReader::new(1 << 20),
            exporter,
            got: Vec::new(),
        };
        raw.write("<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' to='moqui.local' version='1.0'>").await;
        raw.until(|el| el.name() == "features").await;
        raw
    }

    async fn write(&mut self, text: &str) {
        self.stream.write_all(text.as_bytes()).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    async fn until(&mut self, wanted: impl Fn(&Element) -> bool) -> Element {
        let until = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(at) = self.got.iter().position(&wanted) {
                return self.got.remove(at);
            }
            let mut buf = [0_u8; 8192];
            let n = tokio::time::timeout_at(until, self.stream.read(&mut buf))
                .await
                .expect("in time")
                .unwrap();
            assert!(n > 0, "the server closed the stream; got {:?}", self.got);
            for frame in self.reader.feed(&buf[..n]).unwrap() {
                if let Frame::Element(element) = frame {
                    self.got.push(element);
                }
            }
        }
    }
}

#[test]
fn xmpp_clients_sign_in_chat_with_the_web_page_the_archive_upload_publish_and_resume() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let issued = rcgen::generate_simple_self_signed(vec!["moqui.local".to_string()]).unwrap();
    let pem = |label: &str, der: &[u8]| {
        let body = STANDARD.encode(der);
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
    };
    let cert = pem("CERTIFICATE", issued.cert.der());
    let key = pem("PRIVATE KEY", &issued.signing_key.serialize_der());
    let gate = crate::common::start_with_files(
        "xmpp",
        Some(DATA),
        &[
            ("xmpp_domain", "moqui.local"),
            ("xmpp_tls_port", "0"),
            ("xmpp_tls_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    let http = gate.server.address();
    let (_, xmpp_tls) = gate.server.xmpp_addresses();
    let xmpp_tls = xmpp_tls.expect("the direct TLS listener is up");
    let factory = Arc::clone(gate.server.factory());

    // app passwords (made with their SCRAM verifiers) and a room of the two
    let ann_password = as_user(&factory, "xmpp.ann", || {
        get(
            &call(
                "org.moqui.impl.UserServices.create#UserAppPassword",
                &[("name", Value::string("phone"))],
            ),
            "password",
        )
    });
    let ben_password = as_user(&factory, "xmpp.ben", || {
        get(
            &call(
                "org.moqui.impl.UserServices.create#UserAppPassword",
                &[("name", Value::string("laptop"))],
            ),
            "password",
        )
    });
    let room = as_user(&factory, "xmpp.ann", || {
        get(
            &call(
                "org.moqui.impl.ChatServices.create#Room",
                &[
                    ("roomTypeEnumId", Value::string("CrtPrivate")),
                    ("roomName", Value::string("Plans")),
                    ("memberUserIdList", Value::List(vec![Value::string("XMPP_BEN")])),
                ],
            ),
            "chatRoomId",
        )
    });
    let room_node = moqui_xmpp::jids::room_node(&room);
    let room_jid = format!("{room_node}@rooms.moqui.local");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // ---- Ann: tokio-xmpp, SCRAM-SHA-256-PLUS over tls-exporter ----
        let jid: tokio_xmpp::jid::Jid = "xmpp.ann@moqui.local".parse().unwrap();
        let mut ann = Client::new_with_connector(jid, ann_password.clone(), DirectTls(xmpp_tls), Timeouts::tight());
        let online = tokio::time::timeout(WAIT, ann.next()).await.expect("signed in in time").expect("an event");
        let Event::Online { bound_jid, .. } = online else {
            panic!("not online: {online:?}");
        };
        assert!(bound_jid.to_string().starts_with("xmpp.ann@moqui.local/"));
        ann.send_stanza(stanza("<presence xmlns='jabber:client'/>")).await.unwrap();

        // the room as a MUC room; a message to it reflected back with its id
        ann.send_stanza(stanza(&format!(
            "<presence xmlns='jabber:client' to='{room_jid}/ann'><x xmlns='http://jabber.org/protocol/muc'><history maxstanzas='0'/></x></presence>"
        )))
        .await
        .unwrap();
        next_where(&mut ann, |el| el.name() == "message" && el.has_child("subject", "jabber:client")).await;
        ann.send_stanza(stanza(&format!(
            "<message xmlns='jabber:client' type='groupchat' to='{room_jid}' id='hello-1'><body>Hello from XMPP</body></message>"
        )))
        .await
        .unwrap();
        let reflected = next_where(&mut ann, |el| el.name() == "message" && el.attr("id") == Some("hello-1")).await;
        assert_eq!(reflected.attr("from"), Some(format!("{room_jid}/xmpp.ann").as_str()));

        // Ben, on the web, reads it, and answers from the web
        let (factory_web, room_web) = (Arc::clone(&factory), room.clone());
        let read = tokio::task::spawn_blocking(move || {
            as_user(&factory_web, "xmpp.ben", || {
                let timeline = call("org.moqui.impl.ChatServices.get#Timeline", &[("chatRoomId", Value::string(&room_web))]);
                let bodies = format!("{:?}", timeline.get("eventList"));
                call(
                    "org.moqui.impl.ChatServices.send#Message",
                    &[("chatRoomId", Value::string(&room_web)), ("body", Value::string("<p>Hello from the web</p>"))],
                );
                bodies
            })
        })
        .await
        .unwrap();
        assert!(read.contains("Hello from XMPP"), "{read}");
        let answer = next_where(&mut ann, |el| {
            el.name() == "message" && el.get_child("body", "jabber:client").is_some_and(|b| b.text() == "Hello from the web")
        })
        .await;
        assert_eq!(answer.attr("from"), Some(format!("{room_jid}/xmpp.ben").as_str()));

        // the room's archive, the last page of one message, then the one before it
        ann.send_stanza(stanza(&format!(
            "<iq xmlns='jabber:client' type='set' id='mam1' to='{room_jid}'><query xmlns='urn:xmpp:mam:2' queryid='q1'>\
             <set xmlns='http://jabber.org/protocol/rsm'><max>1</max><before/></set></query></iq>"
        )))
        .await
        .unwrap();
        let result = next_where(&mut ann, |el| el.name() == "message" && el.has_child("result", "urn:xmpp:mam:2")).await;
        assert!(String::from(&result).contains("Hello from the web"));
        let fin = next_where(&mut ann, |el| el.name() == "iq" && el.attr("id") == Some("mam1")).await;
        let fin_text = String::from(&fin);
        assert!(!fin_text.contains("complete='true'"), "{fin_text}");
        let first = fin.get_child("fin", "urn:xmpp:mam:2").unwrap().get_child("set", "http://jabber.org/protocol/rsm").unwrap()
            .get_child("first", "http://jabber.org/protocol/rsm").unwrap().text();
        ann.send_stanza(stanza(&format!(
            "<iq xmlns='jabber:client' type='set' id='mam2' to='{room_jid}'><query xmlns='urn:xmpp:mam:2' queryid='q2'>\
             <set xmlns='http://jabber.org/protocol/rsm'><max>1</max><before>{first}</before></set></query></iq>"
        )))
        .await
        .unwrap();
        let earlier = next_where(&mut ann, |el| el.name() == "message" && el.has_child("result", "urn:xmpp:mam:2")).await;
        assert!(String::from(&earlier).contains("Hello from XMPP"));

        // an upload slot, the PUT and the GET over HTTP
        ann.send_stanza(stanza(
            "<iq xmlns='jabber:client' type='get' id='up1' to='upload.moqui.local'><request xmlns='urn:xmpp:http:upload:0' \
             filename='note.txt' size='11' content-type='text/plain'/></iq>",
        ))
        .await
        .unwrap();
        let slot = next_where(&mut ann, |el| el.name() == "iq" && el.attr("id") == Some("up1")).await;
        let put_url = slot.get_child("slot", "urn:xmpp:http:upload:0").unwrap_or_else(|| panic!("{}", String::from(&slot)))
            .get_child("put", "urn:xmpp:http:upload:0").unwrap().attr("url").unwrap().to_string();
        let path = put_url.split_once("/xmpp-upload/").map(|(_, rest)| format!("/xmpp-upload/{rest}")).unwrap();
        let (put_path, get_path) = (path.clone(), path);
        let fetched = tokio::task::spawn_blocking(move || {
            let put = crate::common::exchange(
                http,
                &format!("PUT {put_path} HTTP/1.1\r\nHost: moqui.local\r\nContent-Type: text/plain\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world"),
            );
            assert_eq!(put.0, 201, "{put:?}");
            crate::common::exchange(http, &format!("GET {get_path} HTTP/1.1\r\nHost: moqui.local\r\nConnection: close\r\n\r\n"))
        })
        .await
        .unwrap();
        assert_eq!((fetched.0, fetched.2.as_str()), (200, "hello world"));

        // an OMEMO device list (the legacy namespace Conversations uses), open to read
        ann.send_stanza(stanza(
            "<iq xmlns='jabber:client' type='set' id='pep1'><pubsub xmlns='http://jabber.org/protocol/pubsub'>\
             <publish node='eu.siacs.conversations.axolotl.devicelist'><item id='current'>\
             <list xmlns='eu.siacs.conversations.axolotl'><device id='4242'/></list></item></publish>\
             <publish-options><x xmlns='jabber:x:data' type='submit'><field var='FORM_TYPE' type='hidden'>\
             <value>http://jabber.org/protocol/pubsub#publish-options</value></field>\
             <field var='pubsub#access_model'><value>open</value></field></x></publish-options></pubsub></iq>",
        ))
        .await
        .unwrap();
        let published = next_where(&mut ann, |el| el.name() == "iq" && el.attr("id") == Some("pep1")).await;
        assert_eq!(published.attr("type"), Some("result"), "{}", String::from(&published));

        // ---- Ben: SASL2 with SCRAM-PLUS, Bind 2, stream management and a FAST token ----
        let mut ben = Raw::open(xmpp_tls).await;
        let (mut scram, first) = moqui_xmpp::scram::ClientExchange::new("xmpp.ben", &ben_password, Some(ben.exporter.clone()));
        ben.write(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='SCRAM-SHA-256-PLUS'><initial-response>{}</initial-response>\
             <user-agent id='3f1e0b8a-ben'/><bind xmlns='urn:xmpp:bind:0'><tag>test</tag>\
             <enable xmlns='urn:xmpp:sm:3' resume='true'/></bind>\
             <request-token xmlns='urn:xmpp:fast:0' mechanism='HT-SHA-256-EXPR'/></authenticate>",
            STANDARD.encode(first)
        ))
        .await;
        let challenge = ben.until(|el| el.name() == "challenge").await;
        let last = scram.respond(&STANDARD.decode(challenge.text()).unwrap()).unwrap();
        ben.write(&format!("<response xmlns='urn:xmpp:sasl:2'>{}</response>", STANDARD.encode(last))).await;
        let success = ben.until(|el| el.name() == "success" || el.name() == "failure").await;
        assert_eq!(success.name(), "success", "{}", String::from(&success));
        let additional = STANDARD.decode(success.get_child("additional-data", "urn:xmpp:sasl:2").unwrap().text()).unwrap();
        assert!(scram.verify(&additional));
        let token = success.get_child("token", "urn:xmpp:fast:0").unwrap().attr("token").unwrap().to_string();
        let sm_id = success.get_child("bound", "urn:xmpp:bind:0").unwrap().get_child("enabled", "urn:xmpp:sm:3").unwrap()
            .attr("id").unwrap().to_string();
        ben.write("<presence/>").await;
        // Ann's device list, read by Ben over PEP
        ben.write(
            "<iq type='get' id='dl' to='xmpp.ann@moqui.local'><pubsub xmlns='http://jabber.org/protocol/pubsub'>\
             <items node='eu.siacs.conversations.axolotl.devicelist'/></pubsub></iq>",
        )
        .await;
        let list = ben.until(|el| el.name() == "iq" && el.attr("id") == Some("dl")).await;
        assert!(String::from(&list).contains("4242"), "{}", String::from(&list));
        ben.write("<r xmlns='urn:xmpp:sm:3'/>").await;
        let ack = ben.until(|el| el.name() == "a").await;
        let h = ack.attr("h").unwrap().to_string();

        // the connection drops; Ann writes; Ben is back with FAST and resumes inside SASL2
        drop(ben);
        tokio::time::sleep(Duration::from_millis(200)).await;
        ann.send_stanza(stanza(
            "<message xmlns='jabber:client' type='chat' to='xmpp.ben@moqui.local' id='away-1'><body>Are you back?</body></message>",
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut ben = Raw::open(xmpp_tls).await;
        let initial = moqui_xmpp::fast::initial_response("xmpp.ben", &token, &ben.exporter);
        ben.write(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='HT-SHA-256-EXPR'><initial-response>{}</initial-response>\
             <user-agent id='3f1e0b8a-ben'/><resume xmlns='urn:xmpp:sm:3' previd='{sm_id}' h='{h}'/></authenticate>",
            STANDARD.encode(initial)
        ))
        .await;
        let success = ben.until(|el| el.name() == "success" || el.name() == "failure").await;
        assert_eq!(success.name(), "success", "{}", String::from(&success));
        assert!(success.has_child("resumed", "urn:xmpp:sm:3"), "{}", String::from(&success));
        let missed = ben
            .until(|el| el.name() == "message" && el.get_child("body", "jabber:client").is_some_and(|b| b.text() == "Are you back?"))
            .await;
        assert_eq!(missed.attr("from"), Some("xmpp.ann@moqui.local"));
    });
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
}

/// A PUT's request line and headers (and the start of its body) on a connection of its own, left open.
fn put_open(http: SocketAddr, path: &str, headers: &str, body: &[u8]) -> std::net::TcpStream {
    use std::io::Write as _;
    let mut stream = std::net::TcpStream::connect(http).unwrap();
    stream
        .write_all(format!("PUT {path} HTTP/1.1\r\nHost: moqui.local\r\n{headers}Connection: close\r\n\r\n").as_bytes())
        .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
    stream
}

/// The status of the response on `stream` within `wait`, or None when none came.
fn status_within(stream: &mut std::net::TcpStream, wait: Duration) -> Option<u16> {
    use std::io::Read as _;
    stream.set_read_timeout(Some(wait)).unwrap();
    let mut got = Vec::new();
    let mut chunk = [0_u8; 512];
    while !got.windows(2).any(|pair| pair == b"\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => got.extend_from_slice(&chunk[..read]),
        }
    }
    String::from_utf8_lossy(&got)
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
}

#[test]
fn an_upload_put_is_checked_before_its_body_kept_to_its_size_and_deadline_and_put_once() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let gate = crate::common::start_with_files(
        "xmpp-upload",
        Some(DATA),
        &[("xmpp_domain", "moqui.local"), ("xmpp_upload_seconds", "2")],
        &[],
    );
    let http = gate.server.address();
    let factory = Arc::clone(gate.server.factory());
    let slot = |name: &str| {
        let out = as_user(&factory, "xmpp.ann", || {
            call(
                "org.moqui.impl.XmppServices.create#UploadSlot",
                &[
                    ("fileName", Value::string(name)),
                    ("fileSize", Value::I64(11)),
                    ("contentType", Value::string("text/plain")),
                ],
            )
        });
        format!("/xmpp-upload/{}/{}", get(&out, "token"), get(&out, "urlName"))
    };
    let headers = "Content-Type: text/plain\r\nContent-Length: 11\r\n";

    // an unknown token is answered before its body is sent
    let mut unknown = put_open(http, "/xmpp-upload/notatoken/a.txt", headers, b"");
    assert_eq!(status_within(&mut unknown, Duration::from_secs(5)), Some(404));
    // a declared length that is not the slot's, likewise
    let first = slot("first.txt");
    let mut wrong = put_open(http, &first, "Content-Type: text/plain\r\nContent-Length: 12\r\n", b"");
    assert_eq!(status_within(&mut wrong, Duration::from_secs(5)), Some(400));
    // a body over the slot's size (chunked, so no length declares it) is refused at the slot's size, and leaves the slot open
    let mut long = put_open(
        http,
        &first,
        "Content-Type: text/plain\r\nTransfer-Encoding: chunked\r\n",
        b"15\r\nhello world, and more\r\n0\r\n\r\n",
    );
    assert_eq!(status_within(&mut long, Duration::from_secs(5)), Some(413));
    // a body that stops short past the deadline (xmpp_upload_seconds 2) is 408, and the slot stays open
    let mut slow = put_open(http, &first, headers, b"hello");
    assert_eq!(status_within(&mut slow, Duration::from_secs(5)), Some(408));

    // two PUTs at once: while the first's body comes (spooled, holding no request worker), the second is 409 at once;
    // then the first goes in, and a third finds the slot done
    let mut one = put_open(http, &first, headers, b"hello");
    std::thread::sleep(Duration::from_millis(300));
    let mut two = put_open(http, &first, headers, b"other words");
    assert_eq!(status_within(&mut two, Duration::from_secs(5)), Some(409));
    {
        use std::io::Write as _;
        one.write_all(b" world").unwrap();
    }
    assert_eq!(status_within(&mut one, Duration::from_secs(5)), Some(201));
    let mut three = put_open(http, &first, headers, b"other words");
    assert_eq!(status_within(&mut three, Duration::from_secs(5)), Some(409));
    let fetched = crate::common::exchange(
        http,
        &format!("GET {first} HTTP/1.1\r\nHost: moqui.local\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!((fetched.0, fetched.2.as_str()), (200, "hello world"));
    // nothing of the PUTs refused or stored is left spooled
    let spooled: Vec<_> = std::fs::read_dir(factory.runtime_dir().join("tmp").join("upload"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("xmpp-"))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    assert!(spooled.is_empty(), "{spooled:?}");
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
}
