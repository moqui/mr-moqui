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

//! On a seeded server: the DAV routes over HTTP (the well-known redirect, a login by app password, plain HTTP refused
//! from another address is the DAV tests' in moqui-core), and co-editing over the notification socket: two browser
//! sessions join a wiki page, one types (acknowledged, the other gets it), the presence goes between them, a save writes
//! the page and tells both, and a viewer's update is refused.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use base64::Engine;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, ClientRequestBuilder, Message, WebSocket};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, Transact, Update};

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn exchange(address: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    (status, headers, body.to_string())
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

fn login(address: SocketAddr) -> String {
    let (status, headers, body) = exchange(
        address,
        &request(
            "POST",
            "/rest/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            "username=john.doe&password=moqui",
        ),
    );
    assert_eq!(status, 200, "{body}");
    headers
        .iter()
        .filter(|(name, _)| name == "set-cookie")
        .find_map(|(_, value)| {
            value
                .split(';')
                .next()?
                .trim()
                .strip_prefix("moqui.session=")
                .map(str::to_string)
        })
        .expect("a session cookie")
}

fn open_socket(address: SocketAddr, cookie: &str) -> Ws {
    let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
    let (socket, _) =
        connect(ClientRequestBuilder::new(uri).with_header("Cookie", format!("moqui.session={cookie}"))).unwrap();
    socket
}

/// The next frame of `kind` for `client` on the socket (other frames are skipped), within `wait`.
fn frame(socket: &mut Ws, kind: &str, wait: Duration) -> Option<Json> {
    if let MaybeTlsStream::Plain(tcp) = socket.get_mut() {
        tcp.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    }
    let until = Instant::now() + wait;
    while Instant::now() < until {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let value: Json = serde_json::from_str(&text).unwrap_or(Json::Null);
                if value.get("type").and_then(Json::as_str) == Some(kind) {
                    return Some(value);
                }
                eprintln!("skipped frame: {text}");
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(err) => panic!("socket read: {err}"),
        }
    }
    None
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn send(socket: &mut Ws, message: Json) {
    socket.send(Message::Text(message.to_string().into())).unwrap();
}

#[test]
fn dav_routes_and_coediting_over_the_socket() {
    let gate = crate::common::start("collab");
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());

    // a page of john.doe's and an app password, as john.doe
    let (page_id, password) = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let call = |name: &str, parameters: &[(&str, Value)]| {
            let parameters = parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect();
            let out = moqui_core::service::sync(name, &parameters).unwrap();
            moqui_core::context::ExecutionContext::with_current(|ec| {
                assert!(ec.message().errors().is_empty(), "{name}: {:?}", ec.message().errors())
            });
            out
        };
        let _authz = context.disable_authz();
        call(
            "org.moqui.impl.WikiServices.create#WikiSpace",
            &[
                ("wikiSpaceId", Value::string("COLLAB")),
                ("description", Value::string("Co-editing")),
            ],
        );
        let page = call(
            "org.moqui.impl.WikiServices.update#WikiPage",
            &[
                ("wikiSpaceId", Value::string("COLLAB")),
                ("pageName", Value::string("Notes")),
                ("pageText", Value::string("# Notes\n\nStart.\n")),
            ],
        );
        let password = call(
            "org.moqui.impl.UserServices.create#UserAppPassword",
            &[("name", Value::string("Gate"))],
        );
        (
            to_plain_string(page.get("wikiPageId").unwrap()),
            to_plain_string(password.get("password").unwrap()),
        )
    };

    // DAV: a calendar app's discovery and a login by app password (from loopback, so plain HTTP takes credentials)
    let (status, headers, _) = exchange(http, &request("PROPFIND", "/.well-known/caldav", &[], ""));
    assert_eq!(status, 301);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "location" && value == "/dav/"),
        "{headers:?}"
    );
    let (status, _, _) = exchange(http, &request("PROPFIND", "/dav/", &[("Depth", "0")], ""));
    assert_eq!(status, 401);
    let auth = format!("Basic {}", b64().encode(format!("john.doe:{password}")));
    let (status, _, body) = exchange(
        http,
        &request(
            "PROPFIND",
            "/dav/files/john.doe/",
            &[("Depth", "1"), ("Authorization", &auth)],
            "",
        ),
    );
    assert_eq!(status, 207, "{body}");
    assert!(body.contains("/dav/files/john.doe/My%20files/"), "{body}");

    // two sessions join the page
    let first = login(http);
    let second = login(http);
    factory.flush_sessions().unwrap();
    let mut alice = open_socket(http, &first);
    let mut bob = open_socket(http, &second);
    let doc = format!("WikiPage:{page_id}");
    let empty = Doc::new().transact().state_vector().encode_v1();
    send(
        &mut alice,
        json!({"collab": "join", "doc": doc, "client": "a1", "awareness": 11, "sv": b64().encode(&empty)}),
    );
    let joined = frame(&mut alice, "joined", Duration::from_secs(10)).expect("alice joined");
    assert_eq!(joined["canEdit"], json!(true));
    assert_eq!(joined["topic"], json!(format!("collab:{doc}")));
    let alice_doc = Doc::new();
    alice_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&b64().decode(joined["update"].as_str().unwrap()).unwrap()).unwrap())
        .unwrap();
    send(
        &mut bob,
        json!({"collab": "join", "doc": doc, "client": "b1", "awareness": 22, "sv": b64().encode(&empty)}),
    );
    let joined = frame(&mut bob, "joined", Duration::from_secs(10)).expect("bob joined");
    assert_eq!(joined["peers"][0]["client"], json!("a1"));
    let bob_doc = Doc::new();
    bob_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&b64().decode(joined["update"].as_str().unwrap()).unwrap()).unwrap())
        .unwrap();
    let peer = frame(&mut alice, "peer", Duration::from_secs(5)).expect("alice sees bob");
    assert_eq!(
        (peer["client"].clone(), peer["joined"].clone()),
        (json!("b1"), json!(true))
    );

    // alice types; her update is acknowledged and reaches bob
    let before = alice_doc.transact().state_vector();
    {
        let fragment = alice_doc.get_or_insert_xml_fragment("default");
        let mut txn = alice_doc.transact_mut();
        let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new("Typed together."));
    }
    let update = alice_doc.transact().encode_state_as_update_v1(&before);
    send(
        &mut alice,
        json!({"collab": "update", "doc": doc, "client": "a1", "id": 1, "update": b64().encode(&update)}),
    );
    let ack = frame(&mut alice, "ack", Duration::from_secs(5)).expect("acknowledged");
    assert_eq!(ack["id"], json!(1));
    let got = frame(&mut bob, "update", Duration::from_secs(5)).expect("bob gets it");
    assert_eq!(got["client"], json!("a1"));
    bob_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&b64().decode(got["update"].as_str().unwrap()).unwrap()).unwrap())
        .unwrap();
    assert!(moqui_core::collab::markdown::render(&bob_doc).contains("Typed together."));
    // presence goes to the other
    send(
        &mut alice,
        json!({"collab": "awareness", "doc": doc, "client": "a1", "update": "AQ=="}),
    );
    assert_eq!(
        frame(&mut bob, "awareness", Duration::from_secs(5)).expect("presence")["update"],
        json!("AQ==")
    );

    // a save writes the page and tells both
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let version = moqui_core::collab::save(&doc, Some("Saved from the gate")).unwrap();
        assert!(version.is_some());
    }
    let saved = frame(&mut bob, "saved", Duration::from_secs(5)).expect("bob told of the save");
    assert!(saved["versionName"].is_string(), "{saved}");
    assert!(frame(&mut alice, "saved", Duration::from_secs(5)).is_some());

    // an update before a join is refused; a leave tells the other
    send(
        &mut bob,
        json!({"collab": "update", "doc": "WikiPage:nosuch", "client": "b1", "id": 9, "update": b64().encode(&update)}),
    );
    assert_eq!(
        frame(&mut bob, "error", Duration::from_secs(5)).expect("refused")["code"],
        json!(409)
    );
    send(&mut alice, json!({"collab": "leave", "doc": doc, "client": "a1"}));
    let left = frame(&mut bob, "peer", Duration::from_secs(5)).expect("bob sees alice leave");
    assert_eq!(
        (left["client"].clone(), left["joined"].clone()),
        (json!("a1"), json!(false))
    );
    drop(alice);
    drop(bob);
}
