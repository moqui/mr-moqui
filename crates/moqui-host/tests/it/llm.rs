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

//! On a seeded server on embedded Postgres: every `moqui.llm` and `moqui.agent` table is there and takes a row, the
//! run status flow in the database is the table of `llm/run/state.rs`, a model row merges with the conf, a call is
//! logged and rolled up into the daily usage, the probe and the endpoint test run against the fake provider and their
//! calls roll up with their cost, and the System > LLM screens render for an admin.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "llm"))]

use std::collections::BTreeSet;

use moqui_core::llm::run::state::{RunState, TRANSITIONS};
use moqui_core::llm::usage::{CallRecord, CallSource, Usage};
use moqui_lib::object::{to_plain_string, Value};

use crate::common::{basic, exchange, http_request, start_with_data};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="gate-fake" protocol="fake" description="Gate fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="gate-model" endpointId="gate-fake" providerModel="fake-1" tier="fast" priceInput="2"
        priceOutput="8" currencyUomId="USD"/>
    <moqui.llm.LlmFeatureBinding featureId="classify" modelId="gate-model" reasoningLevel="none"/>
    <moqui.agent.AgentToolExposure toolName="gate_find_enums" serviceName="org.moqui.impl.BasicServices.find#Enumeration"
        title="Find enumerations" lockVersion="1"/>
    <moqui.agent.AgentToolPolicy agentId="assist" sequenceNum="90" toolPattern="gate_*" classificationEnumId="AgclRead"/>
</entity-facade-xml>"#;

fn get(address: std::net::SocketAddr, auth: &str, path: &str) -> (u16, String) {
    let (base, query) = path.split_once('?').unwrap_or((path, ""));
    let target = if query.is_empty() {
        format!("{base}.html")
    } else {
        format!("{base}.html?{query}")
    };
    let (status, _, page) = exchange(address, &http_request("GET", &target, &[("Authorization", auth)], ""));
    (status, page)
}

#[test]
fn the_llm_tables_the_registry_the_usage_and_the_screens_on_postgres() {
    let gate = start_with_data("llm", DATA);
    let factory = std::sync::Arc::clone(gate.server.factory());
    let call_id = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let facade = factory.entity().unwrap();

        // every table is there
        let names: Vec<String> = moqui_core::entity::info::all_entity_names(&facade, "")
            .into_iter()
            .filter(|name| name.starts_with("moqui.llm.") || name.starts_with("moqui.agent."))
            .collect();
        assert!(names.len() > 40, "{names:?}");
        for name in &names {
            facade
                .find(name)
                .unwrap()
                .disable_authz()
                .count()
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }

        // a conversation, a run, a message, an event and a tool call take rows
        let _authz = context.disable_authz();
        let create = |entity: &str, fields: &[(&str, Value)]| {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(*name, value.clone());
            }
            row.create(&facade).unwrap_or_else(|error| panic!("{entity}: {error}"));
        };
        let now = moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond()), "java.sql.Timestamp").value;
        create(
            "moqui.agent.AgentConversation",
            &[
                ("conversationId", Value::string("GATE_CONV")),
                ("userId", Value::string("EX_JOHN_DOE")),
                ("agentId", Value::string("assist")),
                ("surfaceEnumId", Value::string("AgsfDrawer")),
                ("statusId", Value::string("AgcActive")),
                ("searchText", Value::string("find the open orders")),
            ],
        );
        create(
            "moqui.agent.AgentRun",
            &[
                ("runId", Value::string("GATE_RUN")),
                ("conversationId", Value::string("GATE_CONV")),
                ("agentId", Value::string("assist")),
                ("userId", Value::string("EX_JOHN_DOE")),
                ("statusId", Value::string(RunState::Submitted.status_id())),
                ("submittedDate", now.clone()),
            ],
        );
        create(
            "moqui.agent.AgentMessage",
            &[
                ("messageId", Value::string("GATE_MSG")),
                ("conversationId", Value::string("GATE_CONV")),
                ("messageSeq", Value::I64(1)),
                ("role", Value::string("user")),
            ],
        );
        create(
            "moqui.agent.AgentMessagePart",
            &[
                ("messageId", Value::string("GATE_MSG")),
                ("partSeq", Value::I64(1)),
                ("partKindEnumId", Value::string("AgpkText")),
                ("textValue", Value::string("Find the open orders")),
            ],
        );
        create(
            "moqui.agent.AgentRunEvent",
            &[
                ("runId", Value::string("GATE_RUN")),
                ("eventSeq", Value::I64(1)),
                ("eventType", Value::string("run.state")),
                ("payloadJson", Value::string(r#"{"state":"TASK_STATE_SUBMITTED"}"#)),
            ],
        );
        create(
            "moqui.agent.AgentToolCall",
            &[
                ("toolCallId", Value::string("GATE_TC")),
                ("runId", Value::string("GATE_RUN")),
                ("toolName", Value::string("screen")),
                ("classificationEnumId", Value::string("AgclRead")),
                ("statusId", Value::string("AgtcRunning")),
            ],
        );

        // the run status flow in the database is the state table
        let run_ids: Vec<Value> = RunState::ALL.iter().map(|state| Value::string(state.status_id())).collect();
        let seeded: BTreeSet<(String, String)> = facade
            .find("moqui.basic.StatusFlowTransition")
            .unwrap()
            .condition(moqui_core::entity::Condition::is_in("statusId", run_ids))
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    to_plain_string(row.get("statusId").unwrap()),
                    to_plain_string(row.get("toStatusId").unwrap()),
                )
            })
            .collect();
        let table: BTreeSet<(String, String)> = TRANSITIONS
            .iter()
            .map(|(from, to)| (from.status_id().to_string(), to.status_id().to_string()))
            .collect();
        assert_eq!(seeded, table);
        // an entity-auto update follows the flow: Submitted to Working, not Working back to Submitted
        let call = |name: &str, parameters: &[(&str, Value)]| {
            let parameters = parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect();
            let out = moqui_core::service::sync(name, &parameters).unwrap();
            let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
                let mut messages = ec.message();
                let errors = messages.errors().to_vec();
                messages.clear_errors();
                errors
            })
            .unwrap_or_default();
            (out, errors)
        };
        let working = [("runId", Value::string("GATE_RUN")), ("statusId", Value::string("AgrWorking"))];
        let (_, errors) = call("update#moqui.agent.AgentRun", &working);
        assert!(errors.is_empty(), "{errors:?}");
        let (_, errors) = call(
            "update#moqui.agent.AgentRun",
            &[("runId", Value::string("GATE_RUN")), ("statusId", Value::string("AgrSubmitted"))],
        );
        assert!(!errors.is_empty());

        // the registry: the database model over the conf, and a feature bound to it
        let llm = moqui_core::llm::facade(&factory);
        let target = llm.resolve_feature(&facade, "classify").unwrap();
        assert_eq!((target.model.id.as_str(), target.model.name.as_str()), ("gate-model", "fake-1"));
        assert_eq!(target.endpoint.protocol.as_str(), "fake");
        let (config, errors) = call("org.moqui.agent.AgentAdminServices.get#EffectiveConfig", &[]);
        assert!(errors.is_empty(), "{errors:?}");
        let Some(Value::List(models)) = config.get("modelList") else {
            panic!("{config:?}")
        };
        let gate_model = models
            .iter()
            .find_map(|model| match model {
                Value::Map(map) if map.get("modelId").map(to_plain_string).as_deref() == Some("gate-model") => Some(map.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(gate_model.get("usable").map(to_plain_string).as_deref(), Some("yes"));
        assert_eq!(gate_model.get("source").map(to_plain_string).as_deref(), Some("database"));
        assert!(models.iter().any(|model| matches!(model, Value::Map(map) if map.get("modelId").map(to_plain_string).as_deref() == Some("default"))));

        // a call is logged with its cost and rolled up
        let mut record = CallRecord::new(CallSource::Api, &target);
        record.user_id = Some("EX_JOHN_DOE".to_string());
        record.run_id = Some("GATE_RUN".to_string());
        record.duration_ms = 42;
        record.http_status = Some(200);
        record.finish = Some("stop".to_string());
        record.usage = Usage {
            input: 500_000,
            output: 100_000,
            ..Usage::default()
        };
        let call_id = llm.record_call(&facade, &target, &record).unwrap();
        let stored = facade
            .find("moqui.llm.LlmCall")
            .unwrap()
            .eq("callId", Value::string(&call_id))
            .one()
            .unwrap()
            .unwrap();
        // 0.5M in at 2 and 0.1M out at 8
        assert_eq!(to_plain_string(stored.get("cost").unwrap()).trim_end_matches('0').trim_end_matches('.'), "1.8");
        let (rolled, errors) = call("org.moqui.agent.AgentAdminServices.rollup#LlmUsage", &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(rolled.get("rowCount").map(to_plain_string).is_some_and(|count| count != "0"), "{rolled:?}");
        let daily = facade
            .find("moqui.llm.LlmUsageDaily")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .eq("modelId", Value::string("gate-model"))
            .list()
            .unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(to_plain_string(daily.get(0).unwrap().get("totalTokens").unwrap()), "600000");
        let (quota, errors) = call("org.moqui.agent.AgentAdminServices.check#LlmQuota", &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(quota.get("withinQuota"), Some(&Value::Bool(true)), "{quota:?}");
        assert_eq!(quota.get("usedTokens").map(to_plain_string).as_deref(), Some("600000"));
        let (totals, errors) = call("org.moqui.agent.AgentAdminServices.get#DashboardTotals", &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(totals.get("runsToday").map(to_plain_string).as_deref(), Some("1"));

        // the probe against the fake: every capability is yes, an admin's verdict stays
        let (_, errors) = call(
            "org.moqui.agent.AgentAdminServices.set#ModelCapability",
            &[
                ("modelId", Value::string("gate-model")),
                ("capability", Value::string("prompt-caching")),
                ("verdict", Value::string("no")),
            ],
        );
        assert!(errors.is_empty(), "{errors:?}");
        let (probed, errors) = call(
            "org.moqui.agent.AgentAdminServices.probe#Model",
            &[("modelId", Value::string("gate-model"))],
        );
        assert!(errors.is_empty(), "{errors:?}");
        let Some(Value::List(capabilities)) = probed.get("capabilityList") else {
            panic!("{probed:?}")
        };
        assert_eq!(capabilities.len(), 6);
        for capability in capabilities {
            let Value::Map(capability) = capability else { panic!("{capability:?}") };
            let name = capability.get("capability").map(to_plain_string).unwrap_or_default();
            assert_eq!(capability.get("verdict").map(to_plain_string).as_deref(), Some("yes"), "{capability:?}");
            assert_eq!(capability.get("stored"), Some(&Value::Bool(name != "prompt-caching")), "{capability:?}");
        }
        let stored: BTreeSet<(String, String, String)> = facade
            .find("moqui.llm.LlmModelCapability")
            .unwrap()
            .eq("modelId", Value::string("gate-model"))
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
                (field("capability"), field("verdict"), field("source"))
            })
            .collect();
        let expected: BTreeSet<(String, String, String)> = [
            ("json-schema", "yes", "probe"),
            ("parallel-tools", "yes", "probe"),
            ("prompt-caching", "no", "admin"),
            ("reasoning-replay", "yes", "probe"),
            ("streaming", "yes", "probe"),
            ("tools", "yes", "probe"),
        ]
        .iter()
        .map(|(a, b, c)| ((*a).to_string(), (*b).to_string(), (*c).to_string()))
        .collect();
        assert_eq!(stored, expected);
        let (tested, errors) = call(
            "org.moqui.agent.AgentAdminServices.test#Endpoint",
            &[("endpointId", Value::string("gate-fake"))],
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(tested.get("ok"), Some(&Value::Bool(true)), "{tested:?}");
        assert!(tested.get("replyText").map(to_plain_string).is_some_and(|text| text.starts_with("Fake reply to:")));
        // the probe's calls and the test's roll up by source, with their cost
        let (_, errors) = call("org.moqui.agent.AgentAdminServices.rollup#LlmUsage", &[]);
        assert!(errors.is_empty(), "{errors:?}");
        let by_source = |source: &str| {
            facade
                .find("moqui.llm.LlmUsageDaily")
                .unwrap()
                .eq("userId", Value::string("EX_JOHN_DOE"))
                .eq("modelId", Value::string("gate-model"))
                .eq("source", Value::string(source))
                .list()
                .unwrap()
                .iter()
                .map(|row| {
                    let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
                    (field("callCount"), field("cost"))
                })
                .collect::<Vec<_>>()
        };
        let probe = by_source("probe");
        assert_eq!(probe.len(), 1, "{probe:?}");
        // tools, parallel tools, the schema and streaming one call each, the replay and the cache two
        assert_eq!(probe[0].0, "8");
        assert!(probe[0].1.parse::<f64>().is_ok_and(|cost| cost > 0.0), "{probe:?}");
        assert_eq!(by_source("test").first().map(|(calls, _)| calls.as_str()), Some("1"));

        // a client message id and a client request id are unique with the user
        let duplicate = |entity: &str, fields: &[(&str, Value)]| {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(*name, value.clone());
            }
            row.create(&facade)
        };
        let message = |id: &str, seq: i64| {
            duplicate(
                "moqui.agent.AgentMessage",
                &[
                    ("messageId", Value::string(id)),
                    ("conversationId", Value::string("GATE_CONV")),
                    ("messageSeq", Value::I64(seq)),
                    ("role", Value::string("user")),
                    ("userId", Value::string("EX_JOHN_DOE")),
                    ("clientMessageId", Value::string("client-1")),
                ],
            )
        };
        assert!(message("GATE_MSG_C1", 2).is_ok());
        assert!(message("GATE_MSG_C2", 3).is_err());
        let run = |id: &str| {
            duplicate(
                "moqui.agent.AgentRun",
                &[
                    ("runId", Value::string(id)),
                    ("conversationId", Value::string("GATE_CONV")),
                    ("userId", Value::string("EX_JOHN_DOE")),
                    ("clientRequestId", Value::string("request-1")),
                    ("statusId", Value::string(RunState::Submitted.status_id())),
                ],
            )
        };
        assert!(run("GATE_RUN_R1").is_ok());
        assert!(run("GATE_RUN_R2").is_err());

        // call content: the user's own, and another user's, which AgentAdmin alone does not show
        let content = |id: &str, user: &str, text: &str| {
            let mut row = facade.make_value("moqui.llm.LlmCall").unwrap();
            row.set("callId", Value::string(id));
            row.set("userId", Value::string(user));
            row.set("modelId", Value::string("gate-model"));
            row.set("requestJson", Value::string(text));
            row.set("startDate", now.clone());
            row.create(&facade).unwrap();
        };
        content("GATE_OWN_CALL", "EX_JOHN_DOE", "OWN_REQUEST_TEXT");
        content("GATE_OTHER_CALL", "_NA_", "OTHER_REQUEST_TEXT");
        call_id
    };

    // a user without AgentAdmin is refused by an admin service
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let facade = factory.entity().unwrap();
        {
            let _authz = context.disable_authz();
            let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
            user.set("userId", Value::string("GATE_LLM_PLAIN"));
            user.set("username", Value::string("llm.plain"));
            user.create(&facade).unwrap();
        }
        moqui_core::user::internal_login_user("llm.plain").unwrap();
        let parameters = std::collections::BTreeMap::new();
        let name = "org.moqui.agent.AgentAdminServices.get#EffectiveConfig";
        // the service's artifact authz refuses first
        let refused = moqui_core::service::sync(name, &parameters).unwrap_err().to_string();
        assert!(refused.contains("not authorized"), "{refused}");
        // and with that out of the way, the service's own AgentAdmin check
        let out = {
            let _authz = context.disable_authz();
            moqui_core::service::sync(name, &parameters)
        };
        let errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
        assert!(errors.iter().any(|error| error.contains("AgentAdmin")), "{out:?} {errors:?}");
        assert!(out.map(|out| !out.contains_key("modelList")).unwrap_or(true));
    }

    // the screens, for an admin
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let call_detail = format!("/System/Llm/Calls/CallDetail?callId={call_id}");
    let screens: [(&str, &str); 13] = [
        ("/System/Llm/Dashboard", "Recent Failed Calls"),
        ("/System/Llm/Models/ModelList", "gate-model"),
        ("/System/Llm/Models/ModelDetail?modelId=gate-model", "fake-1"),
        ("/System/Llm/Models/ModelDetail?modelId=default", "Capabilities"),
        ("/System/Llm/Models/EndpointDetail?endpointId=gate-fake", "Set Key"),
        ("/System/Llm/Usage", "gate-model"),
        ("/System/Llm/Calls/CallList", &call_id),
        (&call_detail, "fake-1"),
        ("/System/Llm/PromptPreview", "## In the drawer"),
        ("/System/Llm/PromptPreview?surface=a2a&variant=small&modelId=gate-model", "## Small steps"),
        ("/System/Llm/Tools/ExposureList", "gate_find_enums"),
        ("/System/Llm/Tools/ExposureDetail?toolName=gate_find_enums", "Schema Override"),
        ("/System/Llm/Tools/Policies", "gate_*"),
    ];
    for (path, text) in screens {
        let (status, page) = get(address, &john, path);
        assert!(
            status == 200 && !page.contains("Error rendering") && page.contains(text),
            "{path}: {status}: {}",
            page.chars().take(3000).collect::<String>()
        );
    }

    // content is shown for the viewer's own call; another user's needs AgentAudit, and its read is logged
    let detail = |id: &str| {
        let (status, page) = get(address, &john, &format!("/System/Llm/Calls/CallDetail?callId={id}"));
        assert!(status == 200 && !page.contains("Error rendering"), "{status}: {page}");
        page
    };
    assert!(detail("GATE_OWN_CALL").contains("OWN_REQUEST_TEXT"));
    let page = detail("GATE_OTHER_CALL");
    assert!(!page.contains("OTHER_REQUEST_TEXT") && page.contains("needs the AgentAudit permission"), "{page}");
    let audit_rows = || {
        let context = factory.new_context();
        let _entered = context.enter();
        let facade = factory.entity().unwrap();
        let _authz = context.disable_authz();
        facade
            .find("moqui.agent.AgentAuditLog")
            .unwrap()
            .eq("callId", Value::string("GATE_OTHER_CALL"))
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(audit_rows(), 0);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let facade = factory.entity().unwrap();
        let _authz = context.disable_authz();
        for (entity, fields) in [
            ("moqui.security.UserGroup", vec![("userGroupId", "GATE_AUDIT"), ("description", "Gate auditors")]),
            ("moqui.security.UserGroupMember", vec![("userGroupId", "GATE_AUDIT"), ("userId", "EX_JOHN_DOE"), ("fromDate", "2020-01-01 00:00:00")]),
            ("moqui.security.UserGroupPermission", vec![("userGroupId", "GATE_AUDIT"), ("userPermissionId", "AgentAudit"), ("fromDate", "2020-01-01 00:00:00")]),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(name, Value::string(value));
            }
            row.create(&facade).unwrap();
        }
    }
    assert!(detail("GATE_OTHER_CALL").contains("OTHER_REQUEST_TEXT"));
    assert_eq!(audit_rows(), 1);
}

const RUN_DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="runs-fake" protocol="fake" description="Runs fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="runs-model" endpointId="runs-fake" providerModel="fake-1" tier="fast"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="runs-model" reasoningLevel="none"/>
</entity-facade-xml>"#;

/// Read a response to its end (an event stream ends when its run does), or until `limit` passes.
fn read_stream(address: std::net::SocketAddr, request: &str, limit: std::time::Duration) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(limit)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => out.extend_from_slice(&buffer[..count]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `id:` of each stored event of an SSE body, in order.
fn stream_ids(body: &str) -> Vec<i64> {
    body.lines().filter_map(|line| line.strip_prefix("id: ")).filter_map(|id| id.trim().parse().ok()).collect()
}

#[test]
fn runs_stream_their_events_to_their_owner_and_one_conversation_has_one_active_run() {
    use moqui_core::llm::provider::fake::{self, Step};
    use moqui_core::llm::run::{self, StartRequest};
    use std::time::Duration;

    let gate = start_with_data("llm-runs", RUN_DATA);
    let factory = std::sync::Arc::clone(gate.server.factory());
    let address = gate.server.address();
    fake::clear("runs-fake");
    fake::script("runs-fake", vec![fake::Reply::text("Hello from the run engine.")]);
    let context = factory.new_context();
    let _entered = context.enter();
    let ask = |text: &str, conversation_id: Option<String>| StartRequest {
        user_id: "EX_JOHN_DOE".to_string(),
        text: text.to_string(),
        conversation_id,
        ..StartRequest::default()
    };
    let done = run::start_run(&factory, &ask("Say hello", None)).unwrap();
    assert_eq!(run::wait_for(&factory, &done.run_id, Duration::from_secs(20)).unwrap(), RunState::Completed);

    // the owner replays the stored events; the stream ends with the run
    let cookie = crate::common::sign_in(address, "john.doe", "moqui");
    let path = format!("/assist/runs/{}/events", done.run_id);
    let body = read_stream(address, &http_request("GET", &path, &[("Cookie", &cookie)], ""), Duration::from_secs(20));
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(body.to_ascii_lowercase().contains("content-type: text/event-stream"), "{body}");
    let ids = stream_ids(&body);
    assert_eq!(ids.first(), Some(&1));
    assert!(ids.windows(2).all(|pair| pair[1] == pair[0] + 1), "{ids:?}");
    assert!(body.contains("event: text.chunk") && body.contains("Hello from the run engine."), "{body}");
    assert!(body.contains("event: run.done"), "{body}");
    let last = *ids.last().unwrap();
    // Last-Event-ID resumes after it; ?after= does the same for a client without the header
    let again = read_stream(
        address,
        &http_request("GET", &path, &[("Cookie", &cookie), ("Last-Event-ID", &(last - 1).to_string())], ""),
        Duration::from_secs(20),
    );
    assert_eq!(stream_ids(&again), [last]);
    let (status, _, json_body) = exchange(
        address,
        &http_request("GET", &format!("{path}.json?after={}", last - 2), &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{json_body}");
    let catch_up: serde_json::Value = serde_json::from_str(&json_body).unwrap();
    assert_eq!(catch_up["lastEventSeq"], last);
    assert_eq!(catch_up["events"].as_array().unwrap().len(), 2);
    assert_eq!(catch_up["state"], "TASK_STATE_COMPLETED");
    // no one else sees it: not logged in is 401, another user's run is not found
    let (status, _, _) = exchange(address, &http_request("GET", &path, &[], ""));
    assert_eq!(status, 401);
    {
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.agent.AgentRun").unwrap();
        row.set("runId", Value::string("OTHERS_RUN"));
        row.set("userId", Value::string("_NA_"));
        row.set("statusId", Value::string("AgrCompleted"));
        row.create(&facade).unwrap();
    }
    let (status, _, _) = exchange(
        address,
        &http_request("GET", "/assist/runs/OTHERS_RUN/events", &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 404);

    // a run that streams while the client listens: the live deltas come with no id, then the stored events
    fake::script(
        "runs-fake",
        vec![fake::Reply::steps(vec![
            Step::Text("Slowly ".to_string()),
            Step::Delay(1500),
            Step::Text("but surely.".to_string()),
            Step::Finish("stop".to_string()),
        ])],
    );
    let live = run::start_run(&factory, &ask("Take a moment", Some(done.conversation_id.clone()))).unwrap();
    let path = format!("/assist/runs/{}/events", live.run_id);
    let body = read_stream(address, &http_request("GET", &path, &[("Cookie", &cookie)], ""), Duration::from_secs(20));
    assert!(body.contains("event: text.delta"), "{body}");
    assert!(body.contains("but surely.") && body.contains("event: run.done"), "{body}");
    assert_eq!(run::wait_for(&factory, &live.run_id, Duration::from_secs(20)).unwrap(), RunState::Completed);

    // two messages at once to one conversation: one starts a run, the other goes to it as input
    fake::script(
        "runs-fake",
        vec![fake::Reply::steps(vec![Step::Delay(800), Step::Text("One at a time.".to_string()), Step::Finish("stop".to_string())])],
    );
    let conversation_id = done.conversation_id.clone();
    let both: Vec<run::Started> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["First", "Second"]
            .into_iter()
            .map(|text| {
                let factory = std::sync::Arc::clone(&factory);
                let request = ask(text, Some(conversation_id.clone()));
                scope.spawn(move || {
                    let context = factory.new_context();
                    let _entered = context.enter();
                    run::start_run(&factory, &request).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    assert_eq!(both.iter().filter(|started| started.queued).count(), 1, "{both:?}");
    assert_eq!(both[0].run_id, both[1].run_id);
    run::wait_for(&factory, &both[0].run_id, Duration::from_secs(20)).unwrap();
}
