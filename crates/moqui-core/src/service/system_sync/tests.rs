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

//! System messages on the in-memory database, with real service workers for the async sends and consumes. The
//! round trip over HTTP (`/rest/sm`) and entity sync, which reads a view entity, are `moqui-host/tests/system_sync.rs`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::service::ServicePools;

const MESSAGE: &str = "moqui.service.message.SystemMessage";

/// A consume service that leaves a row behind, and a send service that fails.
const FIXTURE: &str = r#"<services>
    <service verb="consume" noun="SmsgTestNote">
        <implements service="org.moqui.impl.SystemMessageServices.consume#SystemMessage"/>
        <actions>
            <entity-find-one entity-name="moqui.service.message.SystemMessage" value-field="systemMessage"/>
            <service-call name="create#moqui.test.TestEntity"
                    in-map="{testId:systemMessageId, testMedium:systemMessage.get('messageText')}"/>
        </actions>
    </service>
    <service verb="receive" noun="SmsgTestFail">
        <implements service="org.moqui.impl.SystemMessageServices.receive#SystemMessage"/>
        <actions><return error="true" message="Test receive failed"/></actions>
    </service>
    <service verb="send" noun="SmsgTestFail">
        <implements service="org.moqui.impl.SystemMessageServices.send#SystemMessage"/>
        <actions><return error="true" message="Test send of ${systemMessageId} failed"/></actions>
    </service>
</services>"#;

struct Setup {
    facade: EntityFacade,
    context: ExecutionContext,
    fixture: PathBuf,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.fixture);
    }
}

/// A factory on the in-memory database with two service workers, the system message services, and the fixture.
fn setup(label: &str) -> Setup {
    let facade = crate::testing::open();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(2, 16), crate::context::framework_dir());
    context.factory().set_entity(facade.clone());
    crate::testing::seed_status_flows(&facade);
    let fixture = std::env::temp_dir().join(format!("moqui-smsg-{label}-{}.xml", std::process::id()));
    std::fs::write(&fixture, FIXTURE).unwrap();
    Setup {
        facade,
        context,
        fixture,
    }
}

fn install(setup: &Setup) {
    let services = crate::context::framework_dir().join("service/org/moqui/impl/SystemMessageServices.xml");
    let failures = crate::service::lint(&[services, setup.fixture.clone()]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn seed(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).expect("make");
    for (name, value) in fields {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).expect("create");
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::string(*value)))
        .collect()
}

fn message_field(facade: &EntityFacade, id: &str, field: &str) -> String {
    let row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("systemMessageId", Value::string(id))
        .one()
        .unwrap();
    row.and_then(|row| row.get(field).map(to_plain_string))
        .unwrap_or_default()
}

/// Poll until `done`, for the work the service workers do.
fn wait_until(mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

fn errors(context: &ExecutionContext) -> String {
    context.message().errors().join("\n")
}

#[test]
fn a_queued_message_is_sent_received_and_consumed_by_the_direct_local_sender() {
    let setup = setup("direct");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let facade = &setup.facade;
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SmsgTestDirect"),
            (
                "sendServiceName",
                "org.moqui.impl.SystemMessageServices.send#SystemMessageDirectLocal",
            ),
            ("consumeServiceName", "consume#SmsgTestNote"),
        ],
    );
    seed(
        facade,
        "moqui.service.message.SystemMessageRemote",
        &[
            ("systemMessageRemoteId", "SmsgTestSelf"),
            ("description", "This system"),
        ],
    );

    let out = crate::service::sync(
        "org.moqui.impl.SystemMessageServices.queue#SystemMessage",
        &params(&[
            ("systemMessageTypeId", "SmsgTestDirect"),
            ("systemMessageRemoteId", "SmsgTestSelf"),
            ("messageText", "hello there"),
        ]),
    )
    .unwrap();
    assert_eq!(errors(&setup.context), "");
    let sent_id = out.get("systemMessageId").map(to_plain_string).expect("queued id");
    assert_eq!(message_field(facade, &sent_id, "isOutgoing"), "Y");

    // The send runs on a worker: SmsgProduced, SmsgSending, then SmsgSent with the received message as the remote ID.
    assert!(
        wait_until(|| message_field(facade, &sent_id, "statusId") == "SmsgSent"),
        "status {}",
        message_field(facade, &sent_id, "statusId")
    );
    let received_id = message_field(facade, &sent_id, "remoteMessageId");
    assert!(!received_id.is_empty());
    assert_ne!(received_id, sent_id);
    assert_eq!(message_field(facade, &received_id, "isOutgoing"), "N");
    assert_eq!(message_field(facade, &received_id, "remoteMessageId"), sent_id);
    assert_eq!(message_field(facade, &received_id, "messageText"), "hello there");

    // The receive started the consume on a worker, which ran the type's consume service.
    assert!(
        wait_until(|| message_field(facade, &received_id, "statusId") == "SmsgConsumed"),
        "status {}",
        message_field(facade, &received_id, "statusId")
    );
    let note = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(&received_id))
        .one()
        .unwrap()
        .expect("the consume service row");
    assert_eq!(
        note.get("testMedium").map(to_plain_string).as_deref(),
        Some("hello there")
    );
}

#[test]
fn a_failed_send_puts_the_status_back_and_records_the_error() {
    let setup = setup("fail");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let facade = &setup.facade;
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SmsgTestFailing"),
            ("sendServiceName", "send#SmsgTestFail"),
        ],
    );
    seed(
        facade,
        MESSAGE,
        &[
            ("systemMessageId", "SMF1"),
            ("systemMessageTypeId", "SmsgTestFailing"),
            ("statusId", "SmsgProduced"),
            ("isOutgoing", "Y"),
            ("messageText", "not going anywhere"),
        ],
    );

    crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &params(&[("systemMessageId", "SMF1")]),
    )
    .unwrap();
    // The send service's error was recorded and cleared, so this call itself did not fail.
    assert_eq!(errors(&setup.context), "");
    assert_eq!(message_field(facade, "SMF1", "statusId"), "SmsgProduced");
    assert_eq!(message_field(facade, "SMF1", "failCount"), "1");
    assert!(!message_field(facade, "SMF1", "lastAttemptDate").is_empty());
    let recorded = facade
        .find("moqui.service.message.SystemMessageError")
        .unwrap()
        .eq("systemMessageId", Value::string("SMF1"))
        .list()
        .unwrap();
    assert_eq!(recorded.len(), 1);
    let row = recorded.get(0).unwrap();
    assert_eq!(
        row.get("attemptedStatusId").map(to_plain_string).as_deref(),
        Some("SmsgSent")
    );
    let text = row.get("errorText").map(to_plain_string).unwrap_or_default();
    assert!(text.contains("Test send of SMF1 failed"), "{text}");

    // A second failure counts again.
    crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &params(&[("systemMessageId", "SMF1")]),
    )
    .unwrap();
    assert_eq!(message_field(facade, "SMF1", "failCount"), "2");
    assert_eq!(message_field(facade, "SMF1", "statusId"), "SmsgProduced");
}

#[test]
fn reset_puts_a_message_in_error_back_to_produced_or_received() {
    let setup = setup("reset");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let facade = &setup.facade;
    for (id, outgoing, status) in [
        ("SMR1", "Y", "SmsgError"),
        ("SMR2", "N", "SmsgError"),
        ("SMR3", "Y", "SmsgSent"),
    ] {
        seed(
            facade,
            MESSAGE,
            &[
                ("systemMessageId", id),
                ("statusId", status),
                ("isOutgoing", outgoing),
                ("failCount", "5"),
            ],
        );
        crate::service::sync(
            "org.moqui.impl.SystemMessageServices.reset#SystemMessageInError",
            &params(&[("systemMessageId", id)]),
        )
        .unwrap();
        assert_eq!(errors(&setup.context), "");
    }
    assert_eq!(message_field(facade, "SMR1", "statusId"), "SmsgProduced");
    assert_eq!(message_field(facade, "SMR1", "failCount"), "0");
    assert_eq!(message_field(facade, "SMR2", "statusId"), "SmsgReceived");
    assert_eq!(message_field(facade, "SMR2", "failCount"), "0");
    // Only a message in error is reset.
    assert_eq!(message_field(facade, "SMR3", "statusId"), "SmsgSent");
    assert_eq!(message_field(facade, "SMR3", "failCount"), "5");
}

#[test]
fn send_all_produced_gives_up_after_the_retry_limit_and_skips_a_recent_attempt() {
    let setup = setup("sendall");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let facade = &setup.facade;
    let now = to_plain_string(&super::stamp(super::now_millis()));
    seed(
        facade,
        MESSAGE,
        &[
            ("systemMessageId", "SMA1"),
            ("statusId", "SmsgProduced"),
            ("isOutgoing", "Y"),
            ("failCount", "30"),
        ],
    );
    seed(
        facade,
        MESSAGE,
        &[
            ("systemMessageId", "SMA2"),
            ("statusId", "SmsgProduced"),
            ("isOutgoing", "Y"),
            ("failCount", "30"),
            ("lastAttemptDate", &now),
        ],
    );
    crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#AllProducedSystemMessages",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(errors(&setup.context), "");
    assert_eq!(message_field(facade, "SMA1", "statusId"), "SmsgError");
    // Tried within retryMinutes, so not picked up.
    assert_eq!(message_field(facade, "SMA2", "statusId"), "SmsgProduced");
}

#[test]
fn the_json_rpc_sender_fails_clearly() {
    let setup = setup("jsonrpc");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let _ = crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#SystemMessageJsonRpc",
        &params(&[("systemMessageId", "SMJ1")]),
    );
    let text = errors(&setup.context);
    assert!(text.contains("JSON-RPC is not part of this port"), "{text}");
    assert!(text.contains("send#SystemMessageRest"), "{text}");
}

#[test]
fn the_rest_sender_posts_the_text_with_an_hmac_header_and_reads_the_remote_id() {
    let setup = setup("rest");
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install(&setup);
    let facade = &setup.facade;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || serve_one(&listener, r#"{"systemMessageIdList":["REMOTE9"]}"#));
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SmsgTestRest"),
            (
                "sendServiceName",
                "org.moqui.impl.SystemMessageServices.send#SystemMessageRest",
            ),
            ("contentType", "application/json"),
        ],
    );
    seed(
        facade,
        "moqui.service.message.SystemMessageRemote",
        &[
            ("systemMessageRemoteId", "SmsgTestPeer"),
            (
                "sendUrl",
                &format!("http://127.0.0.1:{port}/rest/sm/${{systemMessageTypeId}}/${{systemMessageRemoteId}}/${{remoteMessageId}}"),
            ),
            ("messageAuthEnumId", "SmatHmacSha256"),
            ("authHeaderName", "X-Test-Signature"),
            ("sharedSecret", "s3cret"),
        ],
    );
    let body = r#"{"note":"to the peer"}"#;
    seed(
        facade,
        MESSAGE,
        &[
            ("systemMessageId", "SMS1"),
            ("systemMessageTypeId", "SmsgTestRest"),
            ("systemMessageRemoteId", "SmsgTestPeer"),
            ("statusId", "SmsgProduced"),
            ("isOutgoing", "Y"),
            ("messageText", body),
        ],
    );

    crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &params(&[("systemMessageId", "SMS1")]),
    )
    .unwrap();
    assert_eq!(errors(&setup.context), "");
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST /rest/sm/SmsgTestRest/SmsgTestPeer/SMS1 "),
        "{request}"
    );
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("content-type: application/json"), "{request}");
    let signature = super::message::hmac_base64_for_test("s3cret", body);
    assert!(
        lower.contains(&format!("x-test-signature: {}", signature.to_ascii_lowercase())),
        "{request}"
    );
    assert!(request.ends_with(body), "{request}");
    assert_eq!(message_field(facade, "SMS1", "statusId"), "SmsgSent");
    assert_eq!(message_field(facade, "SMS1", "remoteMessageId"), "REMOTE9");
}

#[test]
fn an_hmac_receive_is_refused_when_the_remote_has_no_shared_secret() {
    let setup = setup("hmac-empty");
    let _entered = setup.context.enter();
    install(&setup);
    let facade = &setup.facade;
    {
        let _authz = setup.context.disable_authz();
        seed(
            facade,
            "moqui.service.message.SystemMessageType",
            &[("systemMessageTypeId", "SmsgTestIn")],
        );
        for (remote, secret) in [("SmsgNoSecret", ""), ("SmsgSecret", "s3cret")] {
            seed(
                facade,
                "moqui.service.message.SystemMessageRemote",
                &[
                    ("systemMessageRemoteId", remote),
                    ("messageAuthEnumId", "SmatHmacSha256"),
                    ("authHeaderName", "X-Test-Signature"),
                    ("sharedSecret", secret),
                ],
            );
        }
    }
    let body = r#"{"note":"from the peer"}"#;
    let receive = |remote: &str, secret: &str| {
        let mut request = crate::web::WebRequest::new("POST", format!("/rest/sm/SmsgTestIn/{remote}"));
        request.set_header("X-Test-Signature", super::message::hmac_base64_for_test(secret, body));
        request.body = body.as_bytes().to_vec();
        let _web = crate::web::install(BTreeMap::new());
        super::message::receive_rest_message(&request, &["SmsgTestIn".to_string(), remote.to_string()]).unwrap();
        crate::web::output_status()
    };
    // Anyone can sign with an empty secret, so the remote is not usable.
    assert_eq!(receive("SmsgNoSecret", ""), 403);
    assert_eq!(receive("SmsgSecret", "s3cret"), 200, "{}", errors(&setup.context));
}

#[test]
fn an_hmac_sha512_receive_checks_the_hex_signature_in_either_case_with_or_without_its_prefix() {
    let setup = setup("hmac-512");
    let _entered = setup.context.enter();
    install(&setup);
    let facade = &setup.facade;
    {
        let _authz = setup.context.disable_authz();
        seed(
            facade,
            "moqui.service.message.SystemMessageType",
            &[("systemMessageTypeId", "SmsgTestIn512")],
        );
        seed(
            facade,
            "moqui.service.message.SystemMessageRemote",
            &[
                ("systemMessageRemoteId", "SmsgAnet"),
                ("messageAuthEnumId", "SmatHmacSha512"),
                ("authHeaderName", "X-ANET-Signature"),
                ("sharedSecret", "s3cret"),
            ],
        );
    }
    let body = r#"{"notificationId":"n-1","eventType":"net.authorize.payment.authcapture.created"}"#;
    let signed = super::message::hmac_sha512_hex("s3cret", body);
    let receive = |signature: &str, sent: &str| {
        let mut request = crate::web::WebRequest::new("POST", "/rest/sm/SmsgTestIn512/SmsgAnet".to_string());
        request.set_header("X-ANET-Signature", signature.to_string());
        request.body = sent.as_bytes().to_vec();
        let _web = crate::web::install(BTreeMap::new());
        super::message::receive_rest_message(&request, &["SmsgTestIn512".to_string(), "SmsgAnet".to_string()]).unwrap();
        crate::web::output_status()
    };
    // Authorize.Net sends `sha512=` and upper-case hex
    let header = format!("sha512={}", signed.to_ascii_uppercase());
    assert_eq!(receive(&header, body), 200, "{}", errors(&setup.context));
    assert_eq!(receive(&signed, body), 200, "{}", errors(&setup.context));
    // another body, another key, an empty or a SHA-256 signature are refused before anything is kept
    assert_eq!(receive(&header, &body.replace("n-1", "n-2")), 403);
    let other_key = super::message::hmac_sha512_hex("other", body);
    assert_eq!(receive(&format!("sha512={other_key}"), body), 403);
    assert_eq!(receive("sha512=", body), 403);
    assert_eq!(
        receive(&super::message::hmac_base64_for_test("s3cret", body), body),
        403
    );
    assert!(!super::message::sha512_hex_matches_for_test(
        &signed[..64],
        "s3cret",
        body
    ));
}

#[test]
fn a_url_token_receive_needs_the_one_query_token_equal_to_the_shared_secret() {
    let setup = setup("url-token");
    let _entered = setup.context.enter();
    install(&setup);
    let facade = &setup.facade;
    {
        let _authz = setup.context.disable_authz();
        seed(
            facade,
            "moqui.service.message.SystemMessageType",
            &[("systemMessageTypeId", "SmsgTestInToken")],
        );
        for (remote, name, secret) in [
            ("SmsgToken", "", "t0ken-s3cret"),
            ("SmsgTokenNamed", "key", "t0ken-s3cret"),
            ("SmsgTokenEmpty", "", ""),
        ] {
            seed(
                facade,
                "moqui.service.message.SystemMessageRemote",
                &[
                    ("systemMessageRemoteId", remote),
                    ("messageAuthEnumId", "SmatUrlToken"),
                    ("authHeaderName", name),
                    ("sharedSecret", secret),
                ],
            );
        }
    }
    let body = r#"{"event":"track_updated","test":true,"data":{}}"#;
    let receive = |remote: &str, query: &[(&str, &str)]| {
        let mut request = crate::web::WebRequest::new("POST", format!("/rest/sm/SmsgTestInToken/{remote}"));
        request.query = query
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        request.body = body.as_bytes().to_vec();
        let _web = crate::web::install(BTreeMap::new());
        super::message::receive_rest_message(&request, &["SmsgTestInToken".to_string(), remote.to_string()]).unwrap();
        crate::web::output_status()
    };
    // `token` by default, or the parameter authHeaderName names
    assert_eq!(
        receive("SmsgToken", &[("token", "t0ken-s3cret")]),
        200,
        "{}",
        errors(&setup.context)
    );
    assert_eq!(
        receive("SmsgTokenNamed", &[("key", "t0ken-s3cret")]),
        200,
        "{}",
        errors(&setup.context)
    );
    // missing, wrong, empty, another parameter's and a repeated token are refused before anything is kept
    assert_eq!(receive("SmsgToken", &[]), 403);
    assert_eq!(receive("SmsgToken", &[("token", "t0ken-s3creT")]), 403);
    assert_eq!(receive("SmsgToken", &[("token", "")]), 403);
    assert_eq!(receive("SmsgTokenNamed", &[("token", "t0ken-s3cret")]), 403);
    assert_eq!(
        receive("SmsgToken", &[("token", "t0ken-s3cret"), ("token", "other")]),
        403
    );
    // a remote with no secret is not usable: an empty token would match it
    assert_eq!(receive("SmsgTokenEmpty", &[("token", "")]), 403);
}

#[test]
fn a_timestamp_hmac_message_not_kept_gives_its_replay_key_back_so_the_retry_is_taken() {
    let setup = setup("hmac-ts-retry");
    let _entered = setup.context.enter();
    install(&setup);
    // the replay cache MoquiDefaultConf.xml declares, which a test context has not
    setup
        .context
        .factory()
        .cache()
        .get_or_create("moqui.security.hmac.replay")
        .unwrap();
    let facade = &setup.facade;
    {
        let _authz = setup.context.disable_authz();
        seed(
            facade,
            "moqui.service.message.SystemMessageType",
            &[
                ("systemMessageTypeId", "SmsgTestInTs"),
                ("receiveServiceName", "receive#SmsgTestFail"),
            ],
        );
        seed(
            facade,
            "moqui.service.message.SystemMessageRemote",
            &[
                ("systemMessageRemoteId", "SmsgShippo"),
                ("messageAuthEnumId", "SmatHmacSha256Timestamp"),
                ("authHeaderName", "Shippo-Auth-Signature"),
                ("sharedSecret", "s3cret"),
            ],
        );
    }
    let body = r#"{"event":"transaction_updated","test":true,"data":{"object_id":"tx_1"}}"#;
    let header = super::message::timestamp_header("s3cret", body, crate::service::util::now_millis() / 1000);
    let receive = || {
        let mut request = crate::web::WebRequest::new("POST", "/rest/sm/SmsgTestInTs/SmsgShippo".to_string());
        request.set_header("Shippo-Auth-Signature", header.clone());
        request.body = body.as_bytes().to_vec();
        let _web = crate::web::install(BTreeMap::new());
        super::message::receive_rest_message(&request, &["SmsgTestInTs".to_string(), "SmsgShippo".to_string()])
            .unwrap();
        setup.context.message().clear_errors();
        crate::web::output_status()
    };
    // the receive service fails: not kept, so the same signed delivery is taken when it is sent again
    assert_eq!(receive(), 500);
    {
        let _authz = setup.context.disable_authz();
        crate::service::call::sync_with(
            "update#moqui.service.message.SystemMessageType",
            &params(&[("systemMessageTypeId", "SmsgTestInTs"), ("receiveServiceName", "")]),
            &crate::service::call::CallOptions {
                disable_authz: true,
                ..crate::service::call::CallOptions::default()
            },
        )
        .unwrap();
    }
    assert_eq!(receive(), 200, "{}", errors(&setup.context));
    // kept once: the same signature again is a replay
    assert_eq!(receive(), 403);
}

#[test]
fn hmac_values_match_the_java_web_utilities() {
    // HMAC-SHA512 of the usual test vector, hex (SmatHmacSha512).
    assert_eq!(
        super::message::hmac_sha512_hex("key", "The quick brown fox jumps over the lazy dog"),
        "b42af09057bac1e2d41708e48a902e09b5ff7f12ab428a4fe86653c73dd248fb\
         82f948a549f7b791a5b41915ee4d1ec3935357e4e2317250d0372afa2ebeeb3a"
    );
    // HMAC-SHA256 of the usual test vector, base64 as hmacSha256Base64 gives it.
    assert_eq!(
        super::message::hmac_base64_for_test("key", "The quick brown fox jumps over the lazy dog"),
        "97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg="
    );
    let header = super::message::timestamp_header("key", "body", 1_492_774_577);
    assert!(header.starts_with("t=1492774577,v1="), "{header}");
    assert_eq!(header.len(), "t=1492774577,v1=".len() + 64, "{header}");
    // v1 is the hex HMAC-SHA256 of `1492774577.body`, not only a 64-hex suffix.
    assert_eq!(
        header,
        "t=1492774577,v1=8873a6cf255dadf109c64c83dc66b532eec7bd05304ed07365b8be223a257e3b"
    );
}

/// Accept one request, read it whole (headers and the Content-Length body), and answer `reply` as JSON.
fn serve_one(listener: &TcpListener, reply: &str) -> String {
    listener.set_nonblocking(true).unwrap();
    let started = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() > Duration::from_secs(10) {
                    return String::from("accept timed out");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return format!("accept failed: {err}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 4096];
    loop {
        if let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= split + 4 + length {
                break;
            }
        }
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(size) => buf.extend_from_slice(&tmp[..size]),
        }
    }
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
    String::from_utf8_lossy(&buf).into_owned()
}

fn install_sftp(setup: &Setup) {
    let dir = crate::context::framework_dir().join("service/org/moqui");
    let files = [
        dir.join("impl/SystemMessageServices.xml"),
        dir.join("sftp/SftpMessageServices.xml"),
        setup.fixture.clone(),
    ];
    let failures = crate::service::lint(&files).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

/// Wait until every received message of the type has been consumed. The consume runs on a service worker.
fn consumed(facade: &EntityFacade, type_id: &str) -> bool {
    wait_until(|| {
        facade
            .find(MESSAGE)
            .unwrap()
            .eq("systemMessageTypeId", Value::string(type_id))
            .list()
            .unwrap()
            .iter()
            .all(|row| row.get("statusId").map(to_plain_string).as_deref() == Some("SmsgConsumed"))
    })
}

fn received(facade: &EntityFacade, type_id: &str) -> Vec<(String, String)> {
    facade
        .find(MESSAGE)
        .unwrap()
        .eq("systemMessageTypeId", Value::string(type_id))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get("remoteMessageId").map(to_plain_string).unwrap_or_default(),
                row.get("messageText").map(to_plain_string).unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn a_message_goes_out_by_sftp_and_a_poll_receives_moves_and_skips() {
    let setup = setup("sftp");
    let connector = crate::sftp::tests::memory(setup.context.factory());
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install_sftp(&setup);
    let facade = &setup.facade;
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SftpOut"),
            (
                "sendServiceName",
                "org.moqui.sftp.SftpMessageServices.send#SystemMessageSftp",
            ),
            (
                "sendPath",
                "/in/${systemMessageTypeId}-${remoteMessageId ?? systemMessageId}-${date}.txt",
            ),
        ],
    );
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SftpIn"),
            ("consumeServiceName", "consume#SmsgTestNote"),
            ("receivePath", "/in"),
            ("receiveFilePattern", "SftpOut-.*\\.txt"),
            ("receiveResponseEnumId", "MsgRrMove"),
            ("receiveMovePath", "/in/received/${systemMessageTypeId}"),
        ],
    );
    seed(
        facade,
        "moqui.service.message.SystemMessageType",
        &[
            ("systemMessageTypeId", "SftpGroovy"),
            ("sendPath", "/in/${remoteMessageId?:systemMessageId}"),
        ],
    );
    seed(
        facade,
        "moqui.service.message.SystemMessageRemote",
        &[
            ("systemMessageRemoteId", "SftpServer"),
            ("sendUrl", "files.example.com:2222"),
            ("username", "joe"),
            ("password", "pw"),
            ("remoteCharset", "ISO-8859-1"),
            ("sshHostKeyFingerprint", "SHA256:pinned"),
        ],
    );

    let out = crate::service::sync(
        "org.moqui.impl.SystemMessageServices.queue#SystemMessage",
        &params(&[
            ("systemMessageTypeId", "SftpOut"),
            ("systemMessageRemoteId", "SftpServer"),
            ("messageText", "caf\u{e9} order"),
        ]),
    )
    .unwrap();
    assert_eq!(errors(&setup.context), "");
    let sent_id = out.get("systemMessageId").map(to_plain_string).expect("queued id");
    assert!(
        wait_until(|| message_field(facade, &sent_id, "statusId") == "SmsgSent"),
        "status {} errors {}",
        message_field(facade, &sent_id, "statusId"),
        message_field(facade, &sent_id, "lastAttemptMessage")
    );
    assert_eq!(message_field(facade, &sent_id, "remoteMessageId"), sent_id);
    let config = connector.seen.lock().unwrap()[0].clone();
    assert_eq!(
        (config.host.as_str(), config.port, config.username.as_str()),
        ("files.example.com", 2222, "joe")
    );
    assert_eq!(config.host_key_fingerprint.as_deref(), Some("SHA256:pinned"));
    let file = {
        let disk = connector.disk.lock().unwrap();
        let (name, bytes) = disk.files.iter().next().expect("the sent file");
        assert_eq!(bytes, b"caf\xe9 order", "written in the remote charset");
        name.clone()
    };
    assert!(file.starts_with(&format!("/in/SftpOut-{sent_id}-")), "{file}");

    // The poll receives the file in a new transaction, the consume runs, and the file moves.
    let poll = params(&[
        ("systemMessageTypeId", "SftpIn"),
        ("systemMessageRemoteId", "SftpServer"),
    ]);
    let polled = crate::service::sync("org.moqui.sftp.SftpMessageServices.poll#SystemMessageSftp", &poll).unwrap();
    assert!(consumed(facade, "SftpIn"), "{:?}", received(facade, "SftpIn"));
    assert_eq!(errors(&setup.context), "");
    assert_eq!(polled.get("fileCount").map(to_plain_string).as_deref(), Some("1"));
    let name = file.trim_start_matches("/in/").to_string();
    assert_eq!(
        received(facade, "SftpIn"),
        [(name.clone(), "caf\u{e9} order".to_string())]
    );
    {
        let disk = connector.disk.lock().unwrap();
        assert!(
            disk.files.contains_key(&format!("/in/received/SftpIn/{name}")),
            "{:?}",
            disk.files.keys()
        );
        assert!(!disk.files.contains_key(&file));
    }

    // The same file arriving again is skipped: a message already holds it.
    connector
        .disk
        .lock()
        .unwrap()
        .files
        .insert(file.clone(), b"again".to_vec());
    connector
        .disk
        .lock()
        .unwrap()
        .files
        .insert("/in/other.csv".to_string(), b"no".to_vec());
    crate::service::sync("org.moqui.sftp.SftpMessageServices.poll#SystemMessageSftp", &poll).unwrap();
    assert_eq!(received(facade, "SftpIn").len(), 1);
    assert!(
        connector.disk.lock().unwrap().files.contains_key(&file),
        "a skipped file stays"
    );

    // fileCountLimit takes the first names in order.
    for n in ["SftpOut-b.txt", "SftpOut-a.txt", "SftpOut-c.txt"] {
        connector
            .disk
            .lock()
            .unwrap()
            .files
            .insert(format!("/in/{n}"), n.as_bytes().to_vec());
    }
    let mut limited = poll.clone();
    limited.insert("fileCountLimit".to_string(), Value::I32(2));
    let polled = crate::service::sync("org.moqui.sftp.SftpMessageServices.poll#SystemMessageSftp", &limited).unwrap();
    assert!(consumed(facade, "SftpIn"), "{:?}", received(facade, "SftpIn"));
    assert_eq!(polled.get("fileCount").map(to_plain_string).as_deref(), Some("2"));
    let names: Vec<String> = received(facade, "SftpIn").into_iter().map(|(name, _)| name).collect();
    assert!(
        names.contains(&"SftpOut-a.txt".to_string()) && names.contains(&name),
        "{names:?} {}",
        errors(&setup.context)
    );
    assert!(!names.contains(&"SftpOut-c.txt".to_string()), "{names:?}");

    // A Groovy ?: in a path says to use ??.
    let mut groovy = BTreeMap::new();
    groovy.insert("systemMessageId".to_string(), Value::string(&sent_id));
    let mut row = facade.make_value(MESSAGE).unwrap();
    row.set("systemMessageId", Value::string(&sent_id));
    row.set("systemMessageTypeId", Value::string("SftpGroovy"));
    row.update(facade).unwrap();
    setup.context.message().clear();
    let _ = crate::service::sync("org.moqui.sftp.SftpMessageServices.send#SystemMessageSftp", &groovy);
    assert!(
        errors(&setup.context).contains("use ?? instead"),
        "{}",
        errors(&setup.context)
    );
}
