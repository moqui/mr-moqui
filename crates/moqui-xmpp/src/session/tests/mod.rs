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

//! Sessions driven with recorded stanzas (the shapes Conversations, Monal, Dino and Gajim send, from their XEPs'
//! examples), against the in-memory backend of `fake.rs`.

mod fake;

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use minidom::Element;
use tokio::sync::mpsc;

use super::{Out, Session, Transport};
use crate::framing::Frame;
use crate::registry::Inbound;
use crate::scram::ClientExchange;
use crate::{fast, ns, Config, Server};
use fake::{Fake, JANE_PASSWORD, JOHN_PASSWORD};

const EXPORTER: [u8; 32] = [9; 32];

fn server() -> (Arc<Server>, Arc<Fake>) {
    let fake = Fake::new();
    let mut config = Config::for_domain("moqui.local");
    config.software_version = "test".to_string();
    let server = Server::new(config, Arc::clone(&fake) as Arc<dyn crate::backend::Backend>);
    let _ = fake.server.set(Arc::downgrade(&server));
    (server, fake)
}

/// A stanza as a client writes it in its stream (default namespace jabber:client).
fn stanza(xml: &str) -> Element {
    let wrapped: Element = format!("<s xmlns='jabber:client'>{xml}</s>").parse().expect("test XML");
    wrapped.children().next().cloned().expect("one element")
}

fn peer() -> IpAddr {
    "192.0.2.10".parse().unwrap()
}

struct Client {
    session: Session,
    rx: Option<mpsc::Receiver<Inbound>>,
}

impl Client {
    fn new(server: &Arc<Server>, transport: Transport, binding: Option<Vec<u8>>) -> Client {
        Client {
            session: Session::new(Arc::clone(server), peer(), transport, binding),
            rx: None,
        }
    }

    async fn open(&mut self) -> Vec<Element> {
        let header = BTreeMap::from([("to".to_string(), "moqui.local".to_string())]);
        self.session.on_frame(Frame::Open(header)).await;
        self.out()
    }

    async fn send(&mut self, xml: &str) -> Vec<Element> {
        self.session.on_frame(Frame::Element(stanza(xml))).await;
        self.out()
    }

    /// What the session wrote; its pushes taken first.
    fn out(&mut self) -> Vec<Element> {
        if self.rx.is_none() {
            self.rx = self.session.take_receiver();
        }
        self.session
            .take_out()
            .into_iter()
            .filter_map(|out| match out {
                Out::Element(element) => Some(element),
                _ => None,
            })
            .collect()
    }

    /// Handle the pushes waiting for the session, and return what it wrote.
    async fn pump(&mut self) -> Vec<Element> {
        let mut out = self.out();
        while let Some(inbound) = self.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
            self.session.on_inbound(inbound).await;
            out.extend(self.out());
        }
        out
    }
}

fn find<'a>(elements: &'a [Element], name: &str, namespace: &str) -> Option<&'a Element> {
    elements.iter().find(|element| element.is(name, namespace))
}

fn text_of(elements: &[Element]) -> String {
    elements
        .iter()
        .map(crate::framing::write)
        .collect::<Vec<_>>()
        .join("\n")
}

/// SASL2 with SCRAM (PLUS when the client has the exporter), Bind 2 with carbons and resumable stream management, and a
/// FAST token asked for. Returns the client and its token.
async fn sign_in(server: &Arc<Server>, username: &str, password: &str, tag: &str) -> (Client, String) {
    let mut client = Client::new(server, Transport::Tcp { tls: true }, Some(EXPORTER.to_vec()));
    let features = client.open().await;
    let auth = find(&features, "features", ns::STREAM).expect("features");
    assert!(
        crate::framing::write(auth).contains("SCRAM-SHA-256-PLUS"),
        "{}",
        text_of(&features)
    );
    let (mut scram, first) = ClientExchange::new(username, password, Some(EXPORTER.to_vec()));
    let challenge = client
        .send(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='SCRAM-SHA-256-PLUS'><initial-response>{}</initial-response>\
             <user-agent id='d4565fa7-4d72-4749-b3d3-740edbf87770'><software>Conversations</software></user-agent>\
             <bind xmlns='urn:xmpp:bind:0'><tag>{tag}</tag><enable xmlns='urn:xmpp:carbons:2'/>\
             <enable xmlns='urn:xmpp:sm:3' resume='true'/></bind>\
             <request-token xmlns='urn:xmpp:fast:0' mechanism='HT-SHA-256-EXPR'/></authenticate>",
            STANDARD.encode(first)
        ))
        .await;
    let challenge = find(&challenge, "challenge", ns::SASL2).expect("a challenge");
    let last = scram.respond(&STANDARD.decode(challenge.text()).unwrap()).unwrap();
    let out = client
        .send(&format!(
            "<response xmlns='urn:xmpp:sasl:2'>{}</response>",
            STANDARD.encode(last)
        ))
        .await;
    let success = find(&out, "success", ns::SASL2).unwrap_or_else(|| panic!("success: {}", text_of(&out)));
    let additional = STANDARD
        .decode(success.get_child("additional-data", ns::SASL2).unwrap().text())
        .unwrap();
    assert!(scram.verify(&additional), "the server proved it has the verifier");
    let bound = success.get_child("bound", ns::BIND2).expect("bound");
    assert!(bound.has_child("enabled", ns::SM));
    let token = success
        .get_child("token", ns::FAST)
        .expect("a FAST token")
        .attr("token")
        .unwrap()
        .to_string();
    (client, token)
}

#[tokio::test]
async fn starttls_comes_first_on_the_plain_port() {
    let (server, _) = server();
    let mut client = Client::new(&server, Transport::Tcp { tls: false }, None);
    let features = client.open().await;
    let features = find(&features, "features", ns::STREAM).expect("features");
    assert!(features
        .get_child("starttls", ns::TLS)
        .unwrap()
        .has_child("required", ns::TLS));
    assert!(!features.has_child("mechanisms", ns::SASL), "no mechanism before TLS");
    let refused = client
        .send("<iq type='get' id='1'><ping xmlns='urn:xmpp:ping'/></iq>")
        .await;
    assert!(
        find(&refused, "error", ns::STREAM).is_some(),
        "a stanza before TLS ends the stream"
    );

    let mut client = Client::new(&server, Transport::Tcp { tls: false }, None);
    client.open().await;
    client
        .session
        .on_frame(Frame::Element(
            "<starttls xmlns='urn:ietf:params:xml:ns:xmpp-tls'/>".parse().unwrap(),
        ))
        .await;
    let outs = client.session.take_out();
    assert!(matches!(&outs[0], Out::Element(proceed) if proceed.is("proceed", ns::TLS)));
    assert!(matches!(outs[1], Out::StartTls));
    client.session.tls_started(Some(EXPORTER.to_vec()));
    let features = client.open().await;
    let text = text_of(&features);
    assert!(
        text.contains("SCRAM-SHA-256-PLUS") && text.contains("tls-exporter") && text.contains("PLAIN"),
        "{text}"
    );
    assert!(
        text.contains("HT-SHA-256-EXPR") && text.contains("urn:xmpp:bind:0"),
        "{text}"
    );
}

#[tokio::test]
async fn scram_plus_with_bind_2_then_fast_with_its_binding() {
    let (server, fake) = server();
    let (client, token) = sign_in(&server, "john.doe", JOHN_PASSWORD, "Conversations").await;
    assert!(client
        .session
        .my_full()
        .starts_with("john.doe@moqui.local/Conversations."));

    // FAST: one round trip, bound to the TLS session; the server proves it had the token
    let mut again = Client::new(&server, Transport::Tcp { tls: true }, Some(EXPORTER.to_vec()));
    again.open().await;
    let initial = fast::initial_response("john.doe", &token, &EXPORTER);
    let out = again
        .send(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='HT-SHA-256-EXPR'><initial-response>{}</initial-response>\
             <user-agent id='d4565fa7-4d72-4749-b3d3-740edbf87770'/><fast xmlns='urn:xmpp:fast:0' count='1'/>\
             <bind xmlns='urn:xmpp:bind:0'><tag>Conversations</tag></bind></authenticate>",
            STANDARD.encode(initial)
        ))
        .await;
    let success = find(&out, "success", ns::SASL2).unwrap_or_else(|| panic!("{}", text_of(&out)));
    let additional = STANDARD
        .decode(success.get_child("additional-data", ns::SASL2).unwrap().text())
        .unwrap();
    assert_eq!(additional, fast::responder(&token, &EXPORTER));

    // the same token from another TLS session (no matching exporter) is refused, and counted against the source
    let mut stolen = Client::new(&server, Transport::Tcp { tls: true }, Some(vec![1; 32]));
    stolen.open().await;
    let out = stolen
        .send(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='HT-SHA-256-EXPR'><initial-response>{}</initial-response>\
             <user-agent id='x'/></authenticate>",
            STANDARD.encode(fast::initial_response("john.doe", &token, &EXPORTER))
        ))
        .await;
    assert!(find(&out, "failure", ns::SASL2).is_some(), "{}", text_of(&out));
    assert_eq!(
        fake.state.lock().unwrap().fast.len(),
        1,
        "the account's token stays; nothing is locked"
    );
}

#[tokio::test]
async fn failed_sign_ins_block_the_source_not_the_account() {
    let (server, _) = server();
    for attempt in 0..6 {
        let mut client = Client::new(&server, Transport::Tcp { tls: true }, None);
        let features = client.open().await;
        if attempt == 5 {
            assert!(
                find(&features, "error", ns::STREAM).is_some(),
                "blocked: {}",
                text_of(&features)
            );
            break;
        }
        let plain = STANDARD.encode(b"\0john.doe\0wrong-password");
        let out = client
            .send(&format!(
                "<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{plain}</auth>"
            ))
            .await;
        assert!(find(&out, "failure", ns::SASL).is_some());
    }
    // the account still signs in from another address
    let mut other = Client::new(&server, Transport::Tcp { tls: true }, None);
    other.session.peer = "198.51.100.7".parse().unwrap();
    other.open().await;
    let plain = STANDARD.encode(format!("\0john.doe\0{JOHN_PASSWORD}"));
    let out = other
        .send(&format!(
            "<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{plain}</auth>"
        ))
        .await;
    assert!(find(&out, "success", ns::SASL).is_some(), "{}", text_of(&out));
    // RFC 6120: a new stream, then resource binding
    other.open().await;
    let out = other
        .send("<iq type='set' id='b1'><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'><resource>gajim</resource></bind></iq>")
        .await;
    assert!(
        text_of(&out).contains("john.doe@moqui.local/gajim"),
        "{}",
        text_of(&out)
    );
}

#[tokio::test]
async fn a_room_message_reaches_the_other_member_and_reflects_with_its_id() {
    let (server, _) = server();
    let (mut john, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    for (client, nick) in [(&mut john, "john"), (&mut jane, "jane")] {
        let out = client
            .send(&format!(
                "<presence to='r1@rooms.moqui.local/{nick}'><x xmlns='http://jabber.org/protocol/muc'><history maxstanzas='0'/></x></presence>"
            ))
            .await;
        let text = text_of(&out);
        assert!(text.contains("code='110'"), "self-presence: {text}");
        assert!(
            text.contains("code='210'") == (nick == "john"),
            "john's nick is his username: {text}"
        );
        assert!(text.contains("<subject"), "{text}");
    }
    let out = john
        .send(
            "<message type='groupchat' to='r1@rooms.moqui.local' id='msg-1'><body>Hello &amp; welcome</body>\
               <origin-id xmlns='urn:xmpp:sid:0' id='msg-1'/></message>",
        )
        .await;
    assert!(out.is_empty(), "{}", text_of(&out));
    let reflected = john.pump().await;
    let reflected = find(&reflected, "message", ns::CLIENT).expect("the reflection");
    assert_eq!(reflected.attr("id"), Some("msg-1"));
    assert_eq!(reflected.attr("from"), Some("r1@rooms.moqui.local/john.doe"));
    let got = jane.pump().await;
    let got = find(&got, "message", ns::CLIENT).expect("jane gets it");
    assert_eq!(got.get_child("body", ns::CLIENT).unwrap().text(), "Hello & welcome");
    assert_eq!(got.get_child("stanza-id", ns::SID).unwrap().attr("id"), Some("1"));

    // a correction, a reaction and a reply, by stanza-id
    john.send(
        "<message type='groupchat' to='r1@rooms.moqui.local' id='msg-2'><body>Hello all</body>\
               <replace xmlns='urn:xmpp:message-correct:0' id='msg-1'/></message>",
    )
    .await;
    jane.send(
        "<message type='groupchat' to='r1@rooms.moqui.local' id='r-1'><reactions xmlns='urn:xmpp:reactions:0' id='1'>\
               <reaction>👍</reaction></reactions><store xmlns='urn:xmpp:hints'/></message>",
    )
    .await;
    let changes = text_of(&jane.pump().await);
    assert!(
        changes.contains("message-correct:0") && changes.contains("Hello all"),
        "{changes}"
    );
    assert!(changes.contains("<reaction>👍</reaction>"), "{changes}");

    // MAM over the room: two messages, paged one at a time from the end
    let out = jane
        .send(
            "<iq type='set' id='q1' to='r1@rooms.moqui.local'><query xmlns='urn:xmpp:mam:2' queryid='f27'>\
               <set xmlns='http://jabber.org/protocol/rsm'><max>1</max><before/></set></query></iq>",
        )
        .await;
    let results: Vec<&Element> = out.iter().filter(|el| el.is("message", ns::CLIENT)).collect();
    assert_eq!(results.len(), 1, "{}", text_of(&out));
    let fin = text_of(&out);
    assert!(
        fin.contains("<first>1</first>") || fin.contains("<first>2</first>"),
        "{fin}"
    );
}

#[tokio::test]
async fn a_direct_message_goes_to_the_other_user_and_to_my_other_session_as_a_carbon() {
    let (server, _) = server();
    let (mut phone, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut laptop, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "laptop").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    for client in [&mut phone, &mut laptop, &mut jane] {
        client.send("<presence/>").await;
    }
    phone
        .send("<message type='chat' to='jane@moqui.local' id='d-1'><body>Lunch?</body><active xmlns='http://jabber.org/protocol/chatstates'/></message>")
        .await;
    let got = jane.pump().await;
    let got = find(&got, "message", ns::CLIENT).expect("jane gets it");
    assert_eq!(got.attr("from"), Some("john.doe@moqui.local"));
    assert_eq!(got.attr("id"), Some("d-1"));
    assert!(got.has_child("markable", ns::MARKERS));
    assert!(phone.pump().await.is_empty(), "not back to the session that sent it");
    let carbon = laptop.pump().await;
    let carbon = find(&carbon, "message", ns::CLIENT).expect("a carbon");
    assert!(carbon.has_child("sent", ns::CARBONS));

    // the roster now has the direct room's person
    let roster = text_of(
        &jane
            .send("<iq type='get' id='r'><query xmlns='jabber:iq:roster'/></iq>")
            .await,
    );
    assert!(roster.contains("john.doe@moqui.local"), "{roster}");
}

#[tokio::test]
async fn omemo_is_kept_as_it_came_and_moqui_s_encrypted_rooms_are_refused() {
    let (server, fake) = server();
    let (mut john, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    jane.send("<presence/>").await;
    let omemo = "<encrypted xmlns='eu.siacs.conversations.axolotl'><header sid='27183'><key rid='31415'>BASE64</key>\
                 <iv>BASE64IV</iv></header><payload>CIPHERTEXT</payload></encrypted>";
    john.send(&format!(
        "<message type='chat' to='jane@moqui.local' id='o-1'>{omemo}\
                        <body>I sent you an OMEMO encrypted message</body></message>"
    ))
    .await;
    let got = jane.pump().await;
    let got = find(&got, "message", ns::CLIENT).expect("delivered");
    let payload = got
        .get_child("encrypted", ns::OMEMO_LEGACY)
        .expect("the ciphertext as it came");
    assert_eq!(
        payload.get_child("payload", ns::OMEMO_LEGACY).unwrap().text(),
        "CIPHERTEXT"
    );
    assert!(got.has_child("encryption", ns::EME));
    assert!(fake
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|call| call.ends_with("store#EncryptedMessage")));
    assert!(
        !fake
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|call| call.ends_with("send#Message")),
        "no plaintext path"
    );

    // a room with a bridge does not take it
    john.send("<presence to='bridged@rooms.moqui.local/john'><x xmlns='http://jabber.org/protocol/muc'/></presence>")
        .await;
    let out = john
        .send(&format!(
            "<message type='groupchat' to='bridged@rooms.moqui.local' id='o-2'>{omemo}</message>"
        ))
        .await;
    assert!(text_of(&out).contains("not-acceptable"), "{}", text_of(&out));

    // an end-to-end encrypted Moqui room is refused with its reason
    let out = john
        .send("<presence to='x-534543524554@rooms.moqui.local/john'><x xmlns='http://jabber.org/protocol/muc'/></presence>")
        .await;
    let text = text_of(&out);
    assert!(
        text.contains("not-acceptable") && text.contains("end-to-end encrypted"),
        "{text}"
    );
}

#[tokio::test]
async fn pep_keeps_omemo_device_lists_in_both_namespaces_and_notifies_who_asked() {
    let (server, _) = server();
    let (mut john, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    // jane's client says it wants device list notifications (XEP-0115, resolved by asking her client)
    let asked = jane
        .send("<presence><c xmlns='http://jabber.org/protocol/caps' hash='sha-1' node='https://dino.im' ver='abc='/></presence>")
        .await;
    let query = find(&asked, "iq", ns::CLIENT).expect("the server asks what the hash means");
    let id = query.attr("id").unwrap().to_string();
    jane.send(&format!(
        "<iq type='result' id='{id}' to='jane@moqui.local'><query xmlns='http://jabber.org/protocol/disco#info'>\
         <feature var='eu.siacs.conversations.axolotl.devicelist+notify'/><feature var='urn:xmpp:omemo:2:devices+notify'/></query></iq>"
    ))
    .await;
    for (node, payload) in [
        (
            "eu.siacs.conversations.axolotl.devicelist",
            "<list xmlns='eu.siacs.conversations.axolotl'><device id='27183'/></list>",
        ),
        (
            "urn:xmpp:omemo:2:devices",
            "<devices xmlns='urn:xmpp:omemo:2'><device id='27183'/></devices>",
        ),
    ] {
        let out = john
            .send(&format!(
                "<iq type='set' id='p-{node}'><pubsub xmlns='http://jabber.org/protocol/pubsub'><publish node='{node}'>\
                 <item id='current'>{payload}</item></publish><publish-options><x xmlns='jabber:x:data' type='submit'>\
                 <field var='FORM_TYPE' type='hidden'><value>http://jabber.org/protocol/pubsub#publish-options</value></field>\
                 <field var='pubsub#access_model'><value>open</value></field></x></publish-options></pubsub></iq>"
            ))
            .await;
        assert!(text_of(&out).contains("type='result'"), "{}", text_of(&out));
        let event = text_of(&jane.pump().await);
        assert!(
            event.contains("pubsub#event") && event.contains("27183"),
            "{node}: {event}"
        );
        let fetched = jane
            .send(&format!(
                "<iq type='get' id='g' to='john.doe@moqui.local'><pubsub xmlns='http://jabber.org/protocol/pubsub'>\
                 <items node='{node}'/></pubsub></iq>"
            ))
            .await;
        assert!(text_of(&fetched).contains("device id='27183'"), "{}", text_of(&fetched));
    }
    // a publish whose options differ from the node's is refused
    let out = john
        .send("<iq type='set' id='p2'><pubsub xmlns='http://jabber.org/protocol/pubsub'><publish node='urn:xmpp:omemo:2:devices'>\
               <item id='current'><devices xmlns='urn:xmpp:omemo:2'/></item></publish><publish-options>\
               <x xmlns='jabber:x:data' type='submit'><field var='FORM_TYPE' type='hidden'>\
               <value>http://jabber.org/protocol/pubsub#publish-options</value></field>\
               <field var='pubsub#access_model'><value>whitelist</value></field></x></publish-options></pubsub></iq>")
        .await;
    assert!(text_of(&out).contains("precondition-not-met"), "{}", text_of(&out));
}

#[tokio::test]
async fn every_advertised_feature_answers() {
    let (server, _) = server();
    let (mut john, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    // the probe of each feature the server and the account advertise; none may answer service-unavailable
    let probes: &[(&str, &str)] = &[
        (ns::DISCO_INFO, "<iq type='get' id='a'><query xmlns='http://jabber.org/protocol/disco#info'/></iq>"),
        (ns::DISCO_ITEMS, "<iq type='get' id='a'><query xmlns='http://jabber.org/protocol/disco#items'/></iq>"),
        (ns::PING, "<iq type='get' id='a'><ping xmlns='urn:xmpp:ping'/></iq>"),
        (ns::VERSION, "<iq type='get' id='a'><query xmlns='jabber:iq:version'/></iq>"),
        (ns::CARBONS, "<iq type='set' id='a'><enable xmlns='urn:xmpp:carbons:2'/></iq>"),
        (ns::BLOCKING, "<iq type='get' id='a'><blocklist xmlns='urn:xmpp:blocking'/></iq>"),
        (ns::EXTDISCO, "<iq type='get' id='a'><services xmlns='urn:xmpp:extdisco:2'/></iq>"),
        (ns::PUSH, "<iq type='set' id='a'><enable xmlns='urn:xmpp:push:0' jid='push.example.org' node='n'/></iq>"),
        (ns::ROSTER, "<iq type='get' id='a'><query xmlns='jabber:iq:roster'/></iq>"),
        (ns::VCARD, "<iq type='get' id='a'><vCard xmlns='vcard-temp'/></iq>"),
        (ns::MAM, "<iq type='set' id='a'><query xmlns='urn:xmpp:mam:2'/></iq>"),
        (ns::PUBSUB, "<iq type='set' id='a'><pubsub xmlns='http://jabber.org/protocol/pubsub'><publish node='urn:xmpp:avatar:metadata'>\
            <item id='x'><metadata xmlns='urn:xmpp:avatar:metadata'/></item></publish></pubsub></iq>"),
        (ns::MUC, "<iq type='get' id='a' to='rooms.moqui.local'><query xmlns='http://jabber.org/protocol/disco#items'/></iq>"),
        (ns::UPLOAD, "<iq type='get' id='a' to='upload.moqui.local'><request xmlns='urn:xmpp:http:upload:0' filename='a.jpg' size='1000' content-type='image/jpeg'/></iq>"),
    ];
    let advertised: Vec<&str> = crate::features::SERVER
        .iter()
        .chain(crate::features::ACCOUNT)
        .chain(crate::features::ROOMS)
        .chain(crate::features::UPLOAD)
        .copied()
        .filter(|var| {
            !var.starts_with("http://jabber.org/protocol/pubsub#") && *var != ns::MAM_EXTENDED && *var != ns::SID
        })
        .collect();
    for var in advertised {
        let Some((_, probe)) = probes.iter().find(|(probed, _)| *probed == var) else {
            panic!("no probe for the advertised feature {var}");
        };
        let out = john.send(probe).await;
        let text = text_of(&out);
        assert!(text.contains("type='result'"), "{var}: {text}");
    }
    let info = text_of(&john.send("<iq type='get' id='i' to='upload.moqui.local'><query xmlns='http://jabber.org/protocol/disco#info'/></iq>").await);
    assert!(info.contains("max-file-size"), "{info}");
    let ice = text_of(
        &john
            .send("<iq type='get' id='e'><services xmlns='urn:xmpp:extdisco:2'/></iq>")
            .await,
    );
    assert!(
        ice.contains("turn.moqui.local") && ice.contains("restricted='true'"),
        "{ice}"
    );
    let slot = text_of(&john.send("<iq type='get' id='u' to='upload.moqui.local'><request xmlns='urn:xmpp:http:upload:0' filename='a.jpg' size='1000'/></iq>").await);
    assert!(slot.contains("https://moqui.local/xmpp-upload/tok123/a.jpg"), "{slot}");
    let big = text_of(&john.send("<iq type='get' id='u2' to='upload.moqui.local'><request xmlns='urn:xmpp:http:upload:0' filename='a.bin' size='999999999'/></iq>").await);
    assert!(big.contains("file-too-large"), "{big}");
}

/// Stanzas for other servers, kept.
struct Recorder(Arc<std::sync::Mutex<Vec<Element>>>);

impl crate::Outbound for Recorder {
    fn send(&self, stanza: Element) -> Result<(), String> {
        self.0.lock().unwrap().push(stanza);
        Ok(())
    }
}

#[tokio::test]
async fn a_detached_session_pushes_a_text_free_notice_to_the_app_server() {
    let fake = Fake::new();
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = Server::with_outbound(
        Config::for_domain("moqui.local"),
        Arc::clone(&fake) as Arc<dyn crate::backend::Backend>,
        Box::new(Recorder(Arc::clone(&sent))),
    );
    let _ = fake.server.set(Arc::downgrade(&server));
    let (mut john, _) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    john.send("<presence/>").await;
    let enabled = john
        .send("<iq type='set' id='push1'><enable xmlns='urn:xmpp:push:0' jid='p2.siacs.eu' node='eeaeb5b1'>\
               <x xmlns='jabber:x:data' type='submit'><field var='FORM_TYPE'><value>http://jabber.org/protocol/pubsub#publish-options</value></field>\
               <field var='secret'><value>s3cr3t</value></field></x></enable></iq>")
        .await;
    assert!(text_of(&enabled).contains("type='result'"), "{}", text_of(&enabled));
    assert!(john.session.detach(), "resumable, so it waits");
    jane.send("<message type='chat' to='john.doe@moqui.local' id='p-1'><body>The secret plan</body></message>")
        .await;
    jane.send("<message type='chat' to='john.doe@moqui.local' id='p-2'><body>Another</body></message>")
        .await;
    john.pump().await;
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "one notice a minute");
    let notice = crate::framing::write(&sent[0]);
    assert!(
        notice.contains("to='p2.siacs.eu'") && notice.contains("node='eeaeb5b1'") && notice.contains("s3cr3t"),
        "{notice}"
    );
    assert!(
        notice.contains("urn:xmpp:push:summary") && !notice.contains("secret plan") && !notice.contains("jane"),
        "{notice}"
    );
}

#[tokio::test]
async fn stream_management_counts_and_resumes_inside_sasl2() {
    let (server, _) = server();
    let (mut john, token) = sign_in(&server, "john.doe", JOHN_PASSWORD, "phone").await;
    let (mut jane, _) = sign_in(&server, "jane", JANE_PASSWORD, "laptop").await;
    john.send("<presence/>").await;
    jane.send("<presence/>").await;
    let ack = text_of(&john.send("<r xmlns='urn:xmpp:sm:3'/>").await);
    assert!(ack.contains("h='1'"), "one stanza handled: {ack}");
    let id = john.session.resumable().expect("resumable");
    // the connection drops; the session waits, detached, taking its pushes as unacknowledged stanzas
    assert!(john.session.detach());
    let old = std::mem::replace(
        &mut john.session,
        Session::new(Arc::clone(&server), peer(), Transport::Tcp { tls: true }, None),
    );
    let mut old = old;
    if let Some(rx) = john.rx.take() {
        old.give_receiver(rx);
    }
    let (handover_tx, handover_rx) = tokio::sync::oneshot::channel();
    server.registry.detach(&id, "U1", handover_tx);
    jane.send("<message type='chat' to='john.doe@moqui.local' id='while-away'><body>Still there?</body></message>")
        .await;
    let waiter = tokio::spawn(async move {
        let mut old = old;
        let mut rx = old.take_receiver().unwrap();
        let inbound = rx.recv().await.unwrap();
        old.on_inbound(inbound).await;
        old.take_out();
        old.give_receiver(rx);
        let reply: tokio::sync::oneshot::Sender<Box<Session>> = handover_rx.await.unwrap();
        let _ = reply.send(Box::new(old));
    });
    // a new connection: FAST, and resume in the same round trip
    let mut again = Client::new(&server, Transport::Tcp { tls: true }, Some(EXPORTER.to_vec()));
    again.open().await;
    let initial = fast::initial_response("john.doe", &token, &EXPORTER);
    let out = again
        .send(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='HT-SHA-256-EXPR'><initial-response>{}</initial-response>\
             <user-agent id='d4565fa7-4d72-4749-b3d3-740edbf87770'/><resume xmlns='urn:xmpp:sm:3' previd='{id}' h='0'/></authenticate>",
            STANDARD.encode(initial)
        ))
        .await;
    assert!(out.is_empty(), "the resumed session answers");
    waiter.await.unwrap();
    let resumed = again.session.auth.swap.take().expect("the detached session");
    let mut resumed = *resumed;
    let out: Vec<Element> = resumed
        .take_out()
        .into_iter()
        .filter_map(|out| match out {
            Out::Element(element) => Some(element),
            _ => None,
        })
        .collect();
    let text = text_of(&out);
    assert!(text.contains("resumed") && text.contains("Still there?"), "{text}");
}
