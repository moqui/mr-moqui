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

//! System messages and entity sync through the HTTP server on the seeded embedded Postgres, with the server as its own
//! remote: a message queued through the tools REST API goes out with `send#SystemMessageRest`, comes back in at
//! `/rest/sm`, and is consumed; an entity sync push and a pull each run against `/rest/s1/moqui/entity/syncs/data` and
//! leave a history row. The tools screens for both then render with that data.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::common::{basic, exchange, http_request, start_with_data};

/// The message type, the remote (this server, logged in as john.doe), the two syncs of the three EntitySyncArtifactAppl
/// enumerations, and ADMIN access to the entities the test reads and patches through `/rest/e1`. The services under
/// `/rest/s1/moqui` need no grant here: the seed MOQUI_API grant on the REST path is inherited.
const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.ArtifactGroup artifactGroupId="GATE_SYNC" description="Gate system sync entities"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="GATE_SYNC" artifactName="moqui\.service\.message\..*"
        artifactTypeEnumId="AT_ENTITY" nameIsPattern="Y" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="GATE_SYNC" artifactName="moqui\.entity\.sync\..*"
        artifactTypeEnumId="AT_ENTITY" nameIsPattern="Y" inheritAuthz="Y"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="GATE_SYNC_ADMIN" userGroupId="ADMIN" artifactGroupId="GATE_SYNC"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>

    <moqui.service.message.SystemMessageType systemMessageTypeId="GateSmsg" description="Gate message"
        sendServiceName="org.moqui.impl.SystemMessageServices.send#SystemMessageRest"
        consumeServiceName="org.moqui.impl.SystemMessageServices.reset#SystemMessageInError" contentType="application/json"/>
    <moqui.service.message.SystemMessageRemote systemMessageRemoteId="GateSelf" description="This server"
        messageAuthEnumId="SmatLogin" username="john.doe" password="moqui"/>

    <moqui.security.ArtifactGroup artifactGroupId="GATE_SYNC_DATA" description="Gate sync data"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="GATE_SYNC_DATA" artifactName="moqui.basic.Enumeration"
        artifactTypeEnumId="AT_ENTITY" nameIsPattern="N" filterMap="{enumTypeId: 'EntitySyncArtifactAppl'}"/>
    <moqui.entity.sync.EntitySync entitySyncId="GATE_PUSH" description="Gate push" statusId="EsNotStarted" forPull="N"
        delayBufferMillis="1" targetUsername="john.doe" targetPassword="moqui"/>
    <moqui.entity.sync.EntitySyncArtifact entitySyncId="GATE_PUSH" artifactGroupId="GATE_SYNC_DATA" applEnumId="EsaaInclude"/>
    <moqui.entity.sync.EntitySync entitySyncId="GATE_PULL" description="Gate pull" statusId="EsNotStarted" forPull="Y"
        delayBufferMillis="1" targetUsername="john.doe" targetPassword="moqui"/>
    <moqui.entity.sync.EntitySyncArtifact entitySyncId="GATE_PULL" artifactGroupId="GATE_SYNC_DATA" applEnumId="EsaaInclude"/>
</entity-facade-xml>"#;

fn call(address: SocketAddr, method: &str, path: &str, auth: Option<&str>, body: &str) -> (u16, serde_json::Value) {
    let mut headers = vec![("Content-Type", "application/json")];
    if let Some(auth) = auth {
        headers.push(("Authorization", auth));
    }
    let (status, _, text) = exchange(address, &http_request(method, path, &headers, body));
    let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
    (status, json)
}

fn text(json: &serde_json::Value, key: &str) -> String {
    match json.get(key) {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Poll a GET until `done` holds for its JSON, for the work the service workers do.
fn wait_for(
    address: SocketAddr,
    path: &str,
    auth: &str,
    done: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let (status, json) = call(address, "GET", path, Some(auth), "");
        if status == 200 && done(&json) {
            return json;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{path}: {status} {json}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A tools screen in the text mode answers 200 without a render error.
fn renders(address: SocketAddr, path: &str, auth: &str) -> String {
    let (status, _, body) = exchange(address, &http_request("GET", path, &[("Authorization", auth)], ""));
    assert_eq!(status, 200, "{path}: {body}");
    assert!(!body.contains("Error rendering"), "{path}: {body}");
    body
}

/// One server runs both: they use the same data, and a server start is most of what each costs.
#[test]
fn system_messages_and_entity_syncs_run_over_rest() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_data("system-sync", DATA);
    a_system_message_goes_out_over_rest_comes_back_at_rest_sm_and_is_consumed(&gate);
    an_entity_sync_push_and_a_pull_complete_with_a_history_row(&gate);
}

fn a_system_message_goes_out_over_rest_comes_back_at_rest_sm_and_is_consumed(gate: &crate::common::Gate) {
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let send_url =
        format!("http://{address}/rest/sm/${{systemMessageTypeId}}/${{systemMessageRemoteId}}/${{remoteMessageId}}");
    let (status, json) = call(
        address,
        "PATCH",
        "/rest/e1/moqui.service.message.SystemMessageRemote/GateSelf",
        Some(&john),
        &serde_json::json!({ "sendUrl": send_url }).to_string(),
    );
    assert_eq!(status, 200, "{json}");

    // The receive checks the path, the type, and the login before anything is stored.
    let (status, json) = call(address, "POST", "/rest/sm/GateSmsg", Some(&john), "{}");
    assert_eq!(status, 400, "{json}");
    let (status, json) = call(address, "POST", "/rest/sm/NoSuchType/GateSelf/1", Some(&john), "{}");
    assert_eq!(status, 400, "{json}");
    assert!(text(&json, "errors").contains("NoSuchType"), "{json}");
    let (status, json) = call(address, "POST", "/rest/sm/GateSmsg/GateSelf/1", None, "{}");
    assert_eq!(status, 401, "{json}");

    let message = r#"{"hello":"gate"}"#;
    let (status, json) = call(
        address,
        "POST",
        "/rest/s1/moqui/systemMessages/outgoing/queue",
        Some(&john),
        &serde_json::json!({
            "systemMessageTypeId": "GateSmsg",
            "systemMessageRemoteId": "GateSelf",
            "messageText": message,
        })
        .to_string(),
    );
    assert_eq!(status, 200, "{json}");
    let sent_id = text(&json, "systemMessageId");
    assert!(!sent_id.is_empty(), "{json}");

    // Sent on a worker: the remote's reply carried the ID of the message it received.
    let sent = wait_for(
        address,
        &format!("/rest/e1/moqui.service.message.SystemMessage/{sent_id}"),
        &john,
        |json| text(json, "statusId") == "SmsgSent",
    );
    let received_id = text(&sent, "remoteMessageId");
    assert!(!received_id.is_empty(), "{sent}");
    assert_ne!(received_id, sent_id);

    // Received at /rest/sm, then consumed on a worker by the type's consume service.
    let received = wait_for(
        address,
        &format!("/rest/e1/moqui.service.message.SystemMessage/{received_id}"),
        &john,
        |json| text(json, "statusId") == "SmsgConsumed",
    );
    assert_eq!(text(&received, "isOutgoing"), "N", "{received}");
    assert_eq!(text(&received, "remoteMessageId"), sent_id, "{received}");
    assert_eq!(text(&received, "systemMessageTypeId"), "GateSmsg", "{received}");
    assert_eq!(text(&received, "messageText"), message, "{received}");

    renders(address, "/System/SystemMessage/Message/SystemMessageList.text", &john);
    let detail = renders(
        address,
        &format!("/System/SystemMessage/Message/SystemMessageDetail.text?systemMessageId={received_id}"),
        &john,
    );
    assert!(detail.contains(&received_id), "{detail}");
}

fn an_entity_sync_push_and_a_pull_complete_with_a_history_row(gate: &crate::common::Gate) {
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let target = serde_json::json!({ "targetServerUrl": format!("http://{address}") }).to_string();

    for sync_id in ["GATE_PUSH", "GATE_PULL"] {
        let row_path = format!("/rest/e1/moqui.entity.sync.EntitySync/{sync_id}");
        let (status, json) = call(address, "PATCH", &row_path, Some(&john), &target);
        assert_eq!(status, 200, "{sync_id}: {json}");
        let run_path = format!("/rest/s1/moqui/entity/syncs/{sync_id}/run");
        let (status, json) = call(address, "POST", &run_path, Some(&john), "{}");
        assert_eq!(status, 200, "{sync_id}: {json}");

        let (status, sync) = call(address, "GET", &row_path, Some(&john), "");
        assert_eq!(status, 200, "{sync_id}: {sync}");
        assert_eq!(text(&sync, "statusId"), "EsComplete", "{sync_id}: {sync}");
        assert!(!text(&sync, "lastSuccessfulSyncTime").is_empty(), "{sync_id}: {sync}");

        let (status, history) = call(
            address,
            "GET",
            &format!("/rest/e1/moqui.entity.sync.EntitySyncHistory?entitySyncId={sync_id}"),
            Some(&john),
            "",
        );
        assert_eq!(status, 200, "{sync_id}: {history}");
        let rows = history.as_array().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 1, "{sync_id}: {history}");
        let row = &rows[0];
        assert_eq!(text(row, "statusId"), "EsComplete", "{sync_id}: {row}");
        assert_eq!(text(row, "errorMessage"), "", "{sync_id}: {row}");
        // The three EntitySyncArtifactAppl enumerations, by the member's filter.
        assert_eq!(text(row, "recordsStored"), "3", "{sync_id}: {row}");
        assert!(!text(row, "inclusiveThruTime").is_empty(), "{sync_id}: {row}");

        renders(
            address,
            &format!("/System/EntitySync/EntitySyncDetail.text?entitySyncId={sync_id}"),
            &john,
        );
        renders(
            address,
            &format!("/System/EntitySync/EntitySyncHistory.text?entitySyncId={sync_id}"),
            &john,
        );
    }
    renders(address, "/System/EntitySync/EntitySyncList.text", &john);
}
