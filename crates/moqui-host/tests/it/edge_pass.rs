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

//! The PASS authority over gRPC (plans/archive/Edge-Node.md §5.5, E4), with a stub authority service in a test component written
//! into each runtime (`pass_authority_service`), so no Mantle is needed: Call, Download (several chunks) and Upload
//! round trips, and an account flow's session handle,
//! - **direct**, on the local server's gRPC listener (`pass_authority_grpc_enabled`, plain h2c with no certificate), the
//!   client named by `pass_client_id` with its secret; a wrong secret is refused before the authority runs;
//! - **on the link**, from the edge node down to its local node (`edge:`), where the link is the client
//!   (`pass_edge_client`); with the link down a call fails at once with "local node offline".
//!
//! Calls go through the scripts' binding (`ec.service.passAuthority`) in a test service, and an upload through the
//! host's sender, which the binding wraps. A forwarded request never reaching the service is `edge_link::down`'s test.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_core::pass_authority::{Call, UploadFile};
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const NODE: &str = "local-pass";
const SITE: &str = "PASS_GRPC";

/// The stub authority and the frontend's test service. The authority echoes what it was given, keeps a count in the
/// authority session (an account action), answers an error envelope, a file to download, and an upload's size.
const SERVICES: &str = r#"<services>
    <service verb="handle" noun="Call" authenticate="anonymous-all" transaction="ignore">
        <in-parameters>
            <parameter name="operation" required="true"/>
            <parameter name="siteId"/>
            <parameter name="input" type="Map"/>
            <parameter name="delegation"/>
            <parameter name="clientId"/>
            <parameter name="clientVerified" type="Boolean"/>
            <parameter name="visitorAddress"/>
            <parameter name="operationId"/>
            <parameter name="file" type="FileItem"/>
        </in-parameters>
        <out-parameters>
            <parameter name="result" type="Map"/>
            <parameter name="download" type="Map"/>
            <parameter name="error" type="Map"/>
        </out-parameters>
        <actions><script><![CDATA[
            if (operation === 'echo') {
                result = {operation, siteId, input, clientId, clientVerified, visitorAddress, delegation}
            } else if (operation === 'account.count' || operation === 'account.rotate') {
                if (operation === 'account.rotate') ec.web.rotateSession()
                const count = Number(ec.web.sessionAttributes.passTestCount || 0) + 1
                ec.web.sessionAttributes.passTestCount = count
                result = {count}
            } else if (operation === 'account.login') {
                ec.user.internalLoginUser('john.doe')
                result = {credential: 'c1', user: ec.user.username}
            } else if (operation === 'account.whoami') {
                result = {user: ec.user.username || null}
            } else if (operation === 'fail') {
                error = {code: 'Conflict', message: 'State changed; reload and compare', fieldErrors: [], correlationId: 'c1', status: 409}
            } else if (operation === 'getAttachment') {
                result = {attachment: {filename: 'big.bin'}}
                download = {location: input.location, contentType: 'application/octet-stream', filename: 'big "one".bin', disposition: 'attachment'}
            } else if (operation === 'getShopDownload') {
                // as Mantle's: the file's size and the range asked, or the range refused
                result = {file: {filename: 'big.bin'}}
                download = {location: input.location, contentType: 'application/octet-stream', filename: 'big.bin',
                    disposition: 'attachment', size: 200000, range: input.range || null}
            } else if (operation === 'getShopProductImage') {
                result = {file: {filename: 'big.bin'}}
                download = {location: input.location, contentType: 'image/png', filename: 'big.png', disposition: 'inline',
                    cacheControl: input.cacheControl}
            } else if (operation === 'getShopThemeAsset') {
                result = {file: {filename: 'hero.png'}}
                download = {location: input.location, contentType: 'image/png', filename: 'hero.png', disposition: 'inline',
                    cacheControl: input.cacheControl}
            } else if (operation === 'stageAttachment') {
                result = {size: file.size, name: file.name, contentType: file.contentType, purpose: input.purpose}
            } else {
                error = {code: 'NotFound', message: 'Resource not found', status: 404}
            }
        ]]></script></actions>
    </service>
    <service verb="call" noun="Client" authenticate="false">
        <in-parameters>
            <parameter name="endpoint" required="true"/>
            <parameter name="kind"/>
            <parameter name="operation" required="true"/>
            <parameter name="siteId"/>
            <parameter name="input" type="Map"/>
            <parameter name="sessionHandle"/>
        </in-parameters>
        <out-parameters><parameter name="answer" type="Map"/></out-parameters>
        <actions><script><![CDATA[
            const client = ec.service.passAuthority(endpoint)
            const call = {operation, siteId, input: input || {}, sessionHandle, visitorAddress: '203.0.113.9', delegation: 'd1'}
            if (kind === 'download') {
                const response = client.download(call)
                const bytes = response.bytes()
                let sum = 0
                if (typeof bytes !== 'string' && bytes) for (let i = 0; i < bytes.length; i++) sum = (sum + bytes[i]) % 1000000007
                const header = (name) => (response.headerPairs.find(([key]) => key === name) || [])[1] || ''
                answer = {status: response.statusCode, length: bytes ? bytes.length : 0, sum, error: response.error,
                    disposition: header('Content-Disposition'), cacheControl: header('Cache-Control'),
                    acceptRanges: header('Accept-Ranges'), contentRange: header('Content-Range')}
            } else {
                answer = client.call(call)
            }
        ]]></script></actions>
    </service>
</services>"#;

/// A file of several download chunks (64 KiB each), its bytes known.
fn big_file() -> String {
    (0..200_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect()
}

fn checksum(text: &str) -> i64 {
    text.bytes()
        .fold(0_i64, |sum, byte| (sum + i64::from(byte)) % 1_000_000_007)
}

fn files<'a>(big: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut files = vec![
        (
            "components/addon/passtest/component.xml",
            r#"<component name="passtest"/>"#,
        ),
        (
            "components/addon/passtest/service/passtest/PassTestServices.xml",
            SERVICES,
        ),
        ("pass-test/big.bin", big),
    ];
    files.extend_from_slice(extra);
    files
}

fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// A test authority's root, and a `localhost` certificate and key (PEM) it signed.
fn certs() -> (CertificateDer<'static>, String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let root = ca.self_signed(&ca_key).unwrap().der().clone();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    (
        root,
        pem("CERTIFICATE", cert.der()),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
}

/// A service on `gate`'s server as john.doe, with authorization off: its result, and the errors it added.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, Vec<String>) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| {
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().add_error(err.message().to_string()));
        BTreeMap::new()
    });
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    (out, errors)
}

/// The binding's answer to a call (`kind` call or download) from a script on `gate`'s server, as JSON.
fn client(
    gate: &Gate,
    endpoint: &str,
    kind: &str,
    operation: &str,
    input: serde_json::Value,
    handle: &str,
) -> serde_json::Value {
    client_at(gate, endpoint, kind, operation, SITE, input, handle)
}

/// [`client`] for the site `site`.
fn client_at(
    gate: &Gate,
    endpoint: &str,
    kind: &str,
    operation: &str,
    site: &str,
    input: serde_json::Value,
    handle: &str,
) -> serde_json::Value {
    let (out, errors) = call(
        gate,
        "passtest.PassTestServices.call#Client",
        &[
            ("endpoint", Value::string(endpoint)),
            ("kind", Value::string(kind)),
            ("operation", Value::string(operation)),
            ("siteId", Value::string(site)),
            ("input", moqui_lib::json::from_json(&input)),
            ("sessionHandle", Value::string(handle)),
        ],
    );
    assert!(errors.is_empty(), "{operation} at {endpoint}: {errors:?}");
    moqui_lib::json::to_json(out.get("answer").unwrap_or(&Value::Null))
}

/// An upload through the host's sender: the reply as a script sees it.
fn upload(endpoint: &str, bytes: Vec<u8>) -> Result<serde_json::Value, String> {
    let sender = moqui_core::pass_authority::sender().map_err(|err| err.message().to_string())?;
    let call = Call {
        operation: "stageAttachment".to_string(),
        site_id: SITE.to_string(),
        input: serde_json::json!({"purpose": "post"}),
        ..Call::default()
    };
    let file = UploadFile {
        filename: "notes.txt".to_string(),
        content_type: "text/plain".to_string(),
        bytes,
    };
    sender
        .upload(endpoint, call, file)
        .map(|reply| moqui_core::pass_authority::reply_json(&reply))
        .map_err(|err| err.message().to_string())
}

/// Call, Download and Upload round trips at `endpoint`, with `client_id` the client the authority saw.
fn round_trips(gate: &Gate, endpoint: &str, client_id: &str, big: &str) -> String {
    let echo = client(
        gate,
        endpoint,
        "call",
        "echo",
        serde_json::json!({"title": "Hello", "n": 3}),
        "",
    );
    let result = &echo["result"];
    assert_eq!(result["operation"], "echo", "{echo}");
    assert_eq!(result["siteId"], SITE);
    assert_eq!(result["input"], serde_json::json!({"title": "Hello", "n": 3}));
    assert_eq!(result["clientId"], client_id);
    assert_eq!(result["clientVerified"], true);
    assert_eq!(result["visitorAddress"], "203.0.113.9");
    assert_eq!(result["delegation"], "d1");
    assert_eq!(echo["sessionHandle"], "", "only an account action has a session");

    let refused = client(gate, endpoint, "call", "fail", serde_json::json!({}), "");
    assert_eq!(refused["error"]["code"], "Conflict", "{refused}");
    assert_eq!(refused["error"]["status"], 409);
    assert_eq!(refused["error"]["correlationId"], "c1");
    assert!(
        refused.get("result").is_none_or(serde_json::Value::is_null),
        "{refused}"
    );

    // the authority session an account action carries: the count goes on with the handle, and starts again without
    let first = client(gate, endpoint, "call", "account.count", serde_json::json!({}), "");
    let handle = first["sessionHandle"].as_str().unwrap_or_default().to_string();
    assert_eq!(first["result"]["count"], 1, "{first}");
    assert!(handle.contains('.'), "{first}");
    let second = client(gate, endpoint, "call", "account.count", serde_json::json!({}), &handle);
    assert_eq!(second["result"]["count"], 2, "{second}");
    let rotated = client(gate, endpoint, "call", "account.rotate", serde_json::json!({}), &handle);
    assert_eq!(rotated["result"]["count"], 3, "{rotated}");
    let new_handle = rotated["sessionHandle"].as_str().unwrap_or_default();
    assert!(
        !new_handle.is_empty() && new_handle != handle,
        "the rotated session's handle: {rotated}"
    );
    let after = client(
        gate,
        endpoint,
        "call",
        "account.count",
        serde_json::json!({}),
        new_handle,
    );
    assert_eq!(after["result"]["count"], 4, "{after}");
    let fresh = client(gate, endpoint, "call", "account.count", serde_json::json!({}), "");
    assert_eq!(fresh["result"]["count"], 1, "{fresh}");
    // a handle is good for its client and site only: another site's call gets a new session
    let other = client_at(
        gate,
        endpoint,
        "call",
        "account.count",
        "PASS_OTHER",
        serde_json::json!({}),
        new_handle,
    );
    assert_eq!(other["result"]["count"], 1, "{other}");
    // and it is never a web session's cookie, nor is the session id and MAC in it
    for value in [new_handle, new_handle.trim_start_matches("p1.")] {
        assert_eq!(
            moqui_core::user::session::verify_session_cookie(value).unwrap(),
            None,
            "{value}"
        );
    }
    // once a delegation is issued the session keeps no user: a handle carries only a sign-in in progress
    let login = client(gate, endpoint, "call", "account.login", serde_json::json!({}), "");
    assert_eq!(login["result"]["user"], "john.doe", "{login}");
    let signed_in = login["sessionHandle"].as_str().unwrap_or_default().to_string();
    let who = client(
        gate,
        endpoint,
        "call",
        "account.whoami",
        serde_json::json!({}),
        &signed_in,
    );
    assert!(who["result"]["user"].is_null(), "{who}");

    let download = client(
        gate,
        endpoint,
        "download",
        "getAttachment",
        serde_json::json!({"location": "runtime://pass-test/big.bin"}),
        "",
    );
    assert_eq!(download["status"], 200, "{download}");
    assert_eq!(download["length"], big.len(), "{download}");
    assert_eq!(download["sum"], checksum(big));
    assert_eq!(download["disposition"], "attachment; filename=\"big _one_.bin\"");
    let missing = client(
        gate,
        endpoint,
        "download",
        "getAttachment",
        serde_json::json!({"location": "runtime://pass-test/none.bin"}),
        "",
    );
    assert_eq!(missing["status"], 503, "{missing}");
    assert_eq!(missing["error"]["code"], "Unavailable");
    // a ranged file (Shop F88): only the range is sent, with 206; outside the file 416; whole with Accept-Ranges
    let ranged = |range: serde_json::Value| {
        client(
            gate,
            endpoint,
            "download",
            "getShopDownload",
            serde_json::json!({"location": "runtime://pass-test/big.bin", "range": range}),
            "",
        )
    };
    let part = ranged(serde_json::json!({"start": 70_000, "end": 150_001}));
    assert_eq!(part["status"], 206, "{part}");
    assert_eq!(part["length"], 80_002, "{part}");
    assert_eq!(part["sum"], checksum(&big[70_000..=150_001]));
    assert_eq!(part["contentRange"], "bytes 70000-150001/200000");
    assert_eq!(part["acceptRanges"], "bytes");
    assert_eq!(part["cacheControl"], "no-store");
    let outside = ranged(serde_json::json!({"unsatisfiable": true, "size": 200_000}));
    assert_eq!(
        (&outside["status"], &outside["length"], &outside["contentRange"]),
        (
            &serde_json::json!(416),
            &serde_json::json!(0),
            &serde_json::json!("bytes */200000")
        ),
        "{outside}"
    );
    let whole = ranged(serde_json::Value::Null);
    assert_eq!(
        (&whole["status"], &whole["length"]),
        (&serde_json::json!(200), &serde_json::json!(big.len()))
    );
    assert_eq!(
        (whole["acceptRanges"].as_str(), whole["contentRange"].as_str()),
        (Some("bytes"), Some(""))
    );
    // a public image's Cache-Control is forwarded (Shop F118), only that exact value; a theme asset's immutable one (F55)
    // only on the theme asset download
    let cached = |operation: &str, cache_control: &str| {
        client(
            gate,
            endpoint,
            "download",
            operation,
            serde_json::json!({"location": "runtime://pass-test/big.bin", "cacheControl": cache_control}),
            "",
        )["cacheControl"]
            .clone()
    };
    let immutable = "public, max-age=31536000, immutable";
    assert_eq!(
        cached("getShopProductImage", "public, max-age=60"),
        "public, max-age=60"
    );
    assert_eq!(cached("getShopProductImage", "public, max-age=31536000"), "no-store");
    assert_eq!(cached("getShopProductImage", immutable), "no-store");
    assert_eq!(cached("getShopThemeAsset", immutable), immutable);
    assert_eq!(cached("getShopThemeAsset", "public, max-age=31536000"), "no-store");
    assert_eq!(download["cacheControl"], "no-store");

    let staged = upload(endpoint, vec![b'x'; 150_000]).unwrap();
    assert_eq!(staged["result"]["size"], 150_000, "{staged}");
    assert_eq!(staged["result"]["name"], "notes.txt");
    assert_eq!(staged["result"]["contentType"], "text/plain");
    assert_eq!(staged["result"]["purpose"], "post");
    new_handle.to_string()
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Whether the edge node has the local node's down connection.
fn down(edge: &Gate) -> bool {
    let (out, errors) = call(edge, "org.moqui.edge.EdgeServices.get#EdgeClients", &[]);
    assert!(errors.is_empty(), "{errors:?}");
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter().any(|row| {
        let Value::Map(row) = row else { return false };
        to_plain_string(&row["pairedNodeId"]) == NODE && row["down"] == Value::Bool(true)
    })
}

#[test]
fn pass_authority_calls_round_trip_direct_and_on_the_edge_link() {
    if !crate::common::own_process() {
        return;
    }
    let big = big_file();
    let properties = [
        ("pass_authority_grpc_enabled", "true"),
        ("pass_authority_grpc_port", "0"),
        ("pass_authority_service", "passtest.PassTestServices.handle#Call"),
        ("pass_edge_client", "edgefront"),
        ("pass_client_front_secret", "front-secret"),
        ("pass_client_front_sites", SITE),
        ("pass_client_id", "front"),
        ("pass_client_secret", "front-secret"),
    ];
    let local = crate::common::start_with_conf_files("edge-pass-local", None, "", &properties, &files(&big, &[]));
    let (root, cert, key) = certs();
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-pass-edge",
        "",
        &[("node_role", "edge"), ("webapp_https_listen_port", &edge_port_text)],
        &files(&big, &[("cert/default.crt", &cert), ("cert/default.key", &key)]),
    );

    // direct: the local server's gRPC listener, which binds after the HTTP listener reports ready
    wait_until(Duration::from_secs(10), "the gRPC listener", || {
        local.server.grpc_address().is_some()
    });
    let address = local.server.grpc_address().unwrap();
    let direct = format!("grpc://127.0.0.1:{}", address.port());
    let direct_handle = round_trips(&local, &direct, "front", &big);
    // a file over the limit is refused by the authority, as the REST API's multipart limit
    let large = upload(&direct, vec![b'x'; 10 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(large["error"]["status"], 422, "{large}");
    // a wrong secret is refused before the authority runs; the binding answers the REST adapter's 503
    moqui_lib::env::set_property("pass_client_secret", "not-the-secret".to_string());
    let refused = upload(&direct, b"x".to_vec()).unwrap_err();
    assert!(refused.contains("Unauthenticated"), "{refused}");
    // an upload is refused on its call, before its file is read: a large one as soon
    let started = Instant::now();
    let refused = upload(&direct, vec![b'x'; 8 * 1024 * 1024]).unwrap_err();
    assert!(refused.contains("Unauthenticated"), "{refused}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let answer = client(&local, &direct, "call", "echo", serde_json::json!({}), "");
    assert_eq!(answer["error"]["code"], "Unavailable", "{answer}");
    moqui_lib::env::set_property("pass_client_secret", "front-secret".to_string());

    // on the link: pair, link, and call down from the edge node
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let (out, errors) = call(&edge, "org.moqui.edge.EdgeServices.create#PairingCode", &[]);
    assert!(errors.is_empty(), "{errors:?}");
    let mut pairing = PairConfig::new(&url, &to_plain_string(&out["code"]), NODE);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_until(Duration::from_secs(10), "the link up", || {
        dialer.status().state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || down(&edge));
    round_trips(&edge, "edge:", "edgefront", &big);
    // the direct client's session handle on the link, whose client is another: a new session
    let crossed = client(
        &edge,
        "edge:",
        "call",
        "account.count",
        serde_json::json!({}),
        &direct_handle,
    );
    assert_eq!(crossed["result"]["count"], 1, "{crossed}");

    // with the link down a call fails at once
    dialer.stop();
    drop(dialer);
    wait_until(Duration::from_secs(5), "the link down", || !down(&edge));
    let started = Instant::now();
    let offline = client(&edge, "edge:", "call", "echo", serde_json::json!({}), "");
    assert_eq!(offline["error"]["code"], "Unavailable", "{offline}");
    assert_eq!(offline["error"]["message"], "local node offline", "{offline}");
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(edge);
    drop(local);
}
