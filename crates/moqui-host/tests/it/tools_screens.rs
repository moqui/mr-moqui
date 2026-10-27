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

//! Every tools screen, System and Tools, renders in `html` for an admin user on the seeded embedded Postgres: each
//! screen file under the two apps with no parameters, then the detail screens with the records they show. A screen
//! whose `render-modes` leave out `html` answers that html is not supported, as Java does. The screens that wait for
//! a later phase or are not ported show their message: the Groovy shell. The screens and REST calls
//! `SystemScreenRenderTests`, `ToolsScreenRenderTests`, and `ToolsRestApiTests`
//! check for text are checked here too, on the same server, after that walk.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "git"))]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::common::{basic, exchange, http_request, start_with_conf_properties};

/// The records the detail screens show.
const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="GATE_TOTP_USER" username="gate.totp" userFullName="Gate Totp"/>
    <moqui.basic.print.NetworkPrinter networkPrinterId="GATE_PRINTER" serverHost="localhost" printerName="gate" description="Gate printer"/>
    <!-- Dates ahead, so the seed jobs clean_PrintJobData_daily and clean_ServiceJobRun_daily, which run on the first
         scheduler pass, keep these rows. -->
    <moqui.basic.print.PrintJob printJobId="GATE_PJ" networkPrinterId="GATE_PRINTER" statusId="PtjNotSent" jobName="Gate job"
        contentType="text/plain" createdDate="2999-01-02 03:04:05"/>
    <moqui.service.job.ServiceJobRun jobRunId="GATE_RUN" jobName="clean_TelemetryData_daily" hasError="N"
        startTime="2999-01-02 03:04:05" endTime="2999-01-02 03:04:06"/>
    <moqui.service.message.SystemMessageType systemMessageTypeId="GateSmsg" description="Gate message type"/>
    <moqui.service.message.SystemMessageRemote systemMessageRemoteId="GateRemote" description="Gate remote"/>
    <moqui.service.message.SystemMessage systemMessageId="GATE_MSG" systemMessageTypeId="GateSmsg" statusId="SmsgProduced"
        isOutgoing="Y" messageText="{}"/>
    <moqui.entity.sync.EntitySync entitySyncId="GATE_SYNC" description="Gate sync" statusId="EsNotStarted" forPull="N"/>
    <moqui.resource.GitRepository repositoryId="GATE_GIT" description="Gate git" repositoryKind="db"/>
    <moqui.server.Visit visitId="GATE_VISIT" userId="EX_JOHN_DOE" clientIpAddress="127.0.0.1" fromDate="2026-01-02 03:04:05"/>
    <moqui.server.instance.AppInstance appInstanceId="GATE_AI" instanceName="gate" hostName="gate.example"/>
    <moqui.llm.LlmCall callId="GATE_CALL" modelId="default" endpointId="default" source="test" startDate="2999-01-02 03:04:05"
        inputTokens="10" outputTokens="2"/>
    <moqui.agent.AgentTrigger triggerId="GATE_TRIGGER" agentId="assist" triggerTypeEnumId="AgttCron" cronExpression="0 0 * * * ?"
        description="Gate trigger" paused="Y" breakerState="closed" failureCount="0"/>
    <moqui.llm.LlmApiClient clientId="GATE_CLIENT" userId="EX_JOHN_DOE" description="Gate client" scopes="gateway"
        loginKey="0000000000000000000000000000000000000000000000000000000000000000" allowedAliases="fast"/>
    <moqui.llm.LlmModelAlias alias="gate-fast" modelId="default" description="Gate alias"/>
    <moqui.basic.EnumerationType enumTypeId="GateEnumType" description="Gate enum type"/>
    <moqui.basic.Enumeration enumId="GATE_ENUM" enumTypeId="GateEnumType" description="Gate enum"/>
    <moqui.entity.view.DbViewEntity dbViewEntityName="GateView" packageName="gate.view" isDataView="Y"/>
    <moqui.entity.view.DbViewEntityMember dbViewEntityName="GateView" entityAlias="MASTER" entityName="moqui.basic.Enumeration"/>
    <moqui.entity.view.DbViewEntityMember dbViewEntityName="GateView" entityAlias="MEMBER1" entityName="moqui.basic.EnumerationType"
        joinFromAlias="MASTER" joinOptional="Y"/>
    <moqui.entity.view.DbViewEntityKeyMap dbViewEntityName="GateView" joinFromAlias="MASTER" entityAlias="MEMBER1" fieldName="enumTypeId"/>
    <moqui.entity.view.DbViewEntityAlias dbViewEntityName="GateView" fieldAlias="enumId" entityAlias="MASTER" fieldName="enumId"/>
    <moqui.entity.view.DbViewEntityAlias dbViewEntityName="GateView" fieldAlias="description" entityAlias="MASTER" fieldName="description"/>
    <moqui.entity.view.DbViewEntityAlias dbViewEntityName="GateView" fieldAlias="description2" entityAlias="MEMBER1" fieldName="description"/>
    <moqui.entity.document.DataDocument dataDocumentId="GateDoc" documentName="Gate Document" primaryEntityName="moqui.basic.Enumeration"
        indexName="gate"/>
    <moqui.entity.document.DataDocumentField dataDocumentId="GateDoc" fieldSeqId="01" fieldPath="enumId"/>
    <moqui.entity.document.DataDocumentField dataDocumentId="GateDoc" fieldSeqId="02" fieldPath="description"/>
    <moqui.entity.document.DataDocumentField dataDocumentId="GateDoc" fieldSeqId="03" fieldPath="(enumId + ' / ' + description)"
        fieldNameAlias="enumLabel" defaultDisplay="Y"/>
</entity-facade-xml>"#;

/// Detail screens with the parameters of a record in [`DATA`] or the seed data.
const DETAILS: &[&str] = &[
    "/System/Cache/CacheElements?cacheName=l10n.message",
    "/System/Print/PrintJob/PrintJobDetail?printJobId=GATE_PJ",
    "/System/ServiceJob/JobRuns/JobRunDetail?jobRunId=GATE_RUN",
    "/System/ServiceJob/Jobs/ServiceJobDetail?jobName=clean_TelemetryData_daily",
    "/System/Security/UserAccount/UserAccountDetail?userId=EX_JOHN_DOE",
    // Its render sets up a TOTP factor for the user, so it is not the user the test signs in as.
    "/System/Security/UserAccount/UserAccountDetail/VerifyTotp?userId=GATE_TOTP_USER",
    "/System/Security/UserGroup/UserGroupDetail?userGroupId=ADMIN",
    "/System/Security/UserGroup/GroupUsers?userGroupId=ADMIN",
    "/System/Security/ArtifactGroup/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP",
    "/System/SystemMessage/Type/MessageTypeDetail?systemMessageTypeId=GateSmsg",
    "/System/SystemMessage/Remote/MessageRemoteDetail?systemMessageRemoteId=GateRemote",
    "/System/SystemMessage/Message/SystemMessageDetail?systemMessageId=GATE_MSG",
    "/System/SystemMessage/Message/SystemMessageDetail/EditMessageText?systemMessageId=GATE_MSG",
    "/System/EntitySync/EntitySyncDetail?entitySyncId=GATE_SYNC",
    "/System/EntitySync/EntitySyncHistory?entitySyncId=GATE_SYNC",
    "/System/Resource/GitRepositoryDetail?repositoryId=GATE_GIT",
    "/System/Visit/VisitDetail?visitId=GATE_VISIT",
    "/System/Instance/InstanceDetail?appInstanceId=GATE_AI",
    "/System/DataDocument/Edit/EditDataDocument?dataDocumentId=GateDoc",
    "/System/DataDocument/Edit/ViewDataDocument?dataDocumentId=GateDoc",
    "/System/DataDocument/Edit/DataDocFields?dataDocumentId=GateDoc",
    "/System/DataDocument/Edit/DataDocFields/FieldList?dataDocumentId=GateDoc",
    "/System/Cache/CacheList?filterRegexp=l10n",
    "/System/Llm/Models/ModelDetail?modelId=default",
    "/System/Llm/Models/EndpointDetail?endpointId=default",
    "/System/Llm/Calls/CallDetail?callId=GATE_CALL",
    "/System/Llm/Triggers/TriggerDetail?triggerId=GATE_TRIGGER",
    "/System/Llm/Gateway/ClientDetail?clientId=GATE_CLIENT",
    "/Tools/Entity/DataEdit/EntityDetail?selectedEntity=moqui.basic.Enumeration",
    "/Tools/Entity/DataEdit/EntityDataFind?selectedEntity=moqui.basic.Enumeration",
    "/Tools/Entity/DataEdit/EntityDataEdit?selectedEntity=moqui.basic.Enumeration&enumId=PtjNotSent",
    "/Tools/Entity/DataEdit/EntityList?viewOption=master",
    "/Tools/Entity/SpeedTest?baseCalls=10",
    "/Tools/Entity/TableStats?viewOption=non-empty&filterRegexp=moqui.basic",
    "/Tools/AutoScreen/AutoFind?aen=moqui.basic.Enumeration",
    "/Tools/AutoScreen/AutoEdit/AutoEditMaster?aen=moqui.basic.StatusType&statusTypeId=PrintJob",
    "/Tools/AutoScreen/AutoEdit/AutoEditDetail?aen=moqui.basic.StatusType&den=moqui.basic.StatusItem&statusTypeId=PrintJob",
    "/Tools/DataView/ViewDbView?dbViewEntityName=GateView",
    "/Tools/DataView/ViewDbView?dbViewEntityName=GateView&enumId=GATE_ENUM",
    "/Tools/Entity/DataEdit/EntityDataFind?selectedEntity=DataDocument.GateDoc&enumId=GATE_ENUM",
    "/Tools/DataView/EditDbView?dbViewEntityName=GateView",
    "/Tools/Service/ServiceDetail?serviceName=org.moqui.impl.BasicServices.convert%23Uom",
    "/Tools/Service/ServiceRun?serviceName=org.moqui.impl.BasicServices.echo%23Data",
    "/Tools/Service/ServiceReference?serviceName=PrintServices",
    "/Tools/ArtifactStats?levels=1",
];

/// The screen paths under `dir`, the subscreen directories walked as the screen facade walks them, with whether the
/// screen renders in html (`render-modes` leaves it out).
fn screen_paths(dir: &Path, prefix: &str, out: &mut Vec<(String, bool)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if path.extension().is_some_and(|ext| ext == "xml") {
            let text = std::fs::read_to_string(&path).unwrap();
            let html = text.split_once("render-modes=\"").is_none_or(|(_, rest)| {
                rest.split('"')
                    .next()
                    .unwrap_or("")
                    .split(',')
                    .any(|mode| mode == "html")
            });
            out.push((format!("{prefix}/{name}"), html));
            let sub = path.with_extension("");
            if sub.is_dir() {
                screen_paths(&sub, &format!("{prefix}/{name}"), out);
            }
        }
    }
}

fn get(address: SocketAddr, auth: &str, path: &str) -> (u16, String) {
    let (base, query) = path.split_once('?').map_or((path, ""), |(base, query)| (base, query));
    let target = if query.is_empty() {
        format!("{base}.html")
    } else {
        format!("{base}.html?{query}")
    };
    let (status, _, page) = exchange(address, &http_request("GET", &target, &[("Authorization", auth)], ""));
    (status, page)
}

/// A transition that runs actions refuses parameters on the URL, so they go in the body, as its form posts them.
fn post(address: SocketAddr, auth: &str, path: &str, fields: &[(&str, &str)]) -> (u16, String) {
    let body = fields
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let request = http_request(
        "POST",
        &format!("{path}.html"),
        &[
            ("Authorization", auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &body,
    );
    let (status, _, page) = exchange(address, &request);
    (status, page)
}

/// REST has no render-mode extension. A `.html` suffix would not be the service API.
fn rest_get(address: SocketAddr, auth: &str, path: &str) -> (u16, String) {
    let (status, _, page) = exchange(address, &http_request("GET", path, &[("Authorization", auth)], ""));
    (status, page)
}

fn rest_post_json(address: SocketAddr, auth: &str, path: &str, body: &str) -> (u16, String) {
    let request = http_request(
        "POST",
        path,
        &[("Authorization", auth), ("Content-Type", "application/json")],
        body,
    );
    let (status, _, page) = exchange(address, &request);
    (status, page)
}

fn assert_page(address: SocketAddr, auth: &str, path: &str, texts: &[&str]) {
    let (status, page) = get(address, auth, path);
    assert_eq!(status, 200, "{}", failure(path, status, &page));
    assert!(!page.contains("Error rendering"), "{}", failure(path, status, &page));
    for text in texts {
        assert!(
            page.contains(text),
            "{path} missing [{text}]: {}",
            failure(path, status, &page)
        );
    }
}

/// A transition answers 200, or 302 when it redirects back to its screen. Either way it did not fail to run.
fn ran(path: &str, status: u16, page: &str) {
    assert!(
        matches!(status, 200 | 302)
            && !page.contains("Error rendering")
            && !page.contains("Cannot run screen transition")
            && !page.contains("Missing required parameter"),
        "{path}: {status}: {page}"
    );
}

fn failure(path: &str, status: u16, page: &str) -> String {
    let snippet: String = page.find("Error rendering").map_or_else(
        || page.chars().take(400).collect(),
        |at| page[at..].chars().take(400).collect(),
    );
    format!("{path}: {status}: {snippet}")
}

/// Groovy's suite reads `moqui.basic.StatusType` creates from the artifact hit summary, which the Monitor Artifacts
/// dashboard replaced. The default conf records no entity frames, and metric documents are written each minute. This
/// server records the entity duration metric and collects it every second, so a create this test makes is in the
/// dashboard's dataset before the query.
const TELEMETRY_CONF: &str = r#"
    <telemetry interval-seconds="1"><artifact type="AT_ENTITY" spans="false" metrics="true"/></telemetry>
"#;

/// The checks share one server, and so one seed of [`DATA`] and [`SQL_DATA`]: a server start is most of what the SQL runner check costs.
#[test]
#[ignore = "slow: cargo test-slow"]
fn tools_screens_render_and_the_sql_runners_run() {
    let seed_end = "</entity-facade-xml>";
    let sql_rows = SQL_DATA.split_once('>').map_or("", |(_, rows)| rows);
    let data = format!("{}{sql_rows}", DATA.trim_end_matches(seed_end));
    let gate = start_with_conf_properties("tools-screens", Some(data.as_str()), TELEMETRY_CONF, &[]);
    every_tools_screen_renders_in_html_for_an_admin(&gate);
    the_groovy_specs_text_is_on_the_screens_and_the_rest_api(&gate);
    the_sql_runners_run_for_an_admin_and_refuse_other_users(&gate);
}

fn every_tools_screen_renders_in_html_for_an_admin(gate: &crate::common::Gate) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/tools/screen");
    let mut paths = Vec::new();
    for app in ["System", "Tools"] {
        paths.push((format!("/{app}"), true));
        screen_paths(&root.join(app), &format!("/{app}"), &mut paths);
    }

    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let mut failures = Vec::new();
    let mut checked = 0;
    for (path, html) in &paths {
        // The speed test runs its whole set at render; the parameter case below runs a small one.
        if path.ends_with("/SpeedTest") {
            continue;
        }
        let (status, page) = get(address, &john, path);
        checked += 1;
        if !html {
            if status != 404 || !page.contains("render mode html is not supported") {
                failures.push(failure(path, status, &page));
            }
            continue;
        }
        if status != 200 || page.contains("Error rendering") {
            failures.push(failure(path, status, &page));
        }
    }
    for path in DETAILS {
        let (status, page) = get(address, &john, path);
        checked += 1;
        if status != 200 || page.contains("Error rendering") {
            failures.push(failure(path, status, &page));
        }
    }
    assert!(checked > 140, "{checked} screens");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    // Screens of a later phase or not ported show why.
    let (_, shell) = get(address, &john, "/Tools/GroovyShell");
    assert!(shell.contains("The Groovy Shell is not available"), "{shell}");
    let (status, logs) = get(address, &john, "/apps/monitor/Logs");
    assert_eq!(status, 200, "{logs}");
    assert!(!logs.contains("Error rendering"), "{logs}");

    // The detail screens show their records.
    let (_, entity) = get(
        address,
        &john,
        "/Tools/Entity/DataEdit/EntityDetail?selectedEntity=moqui.basic.Enumeration",
    );
    assert!(
        entity.contains("Entity Detail for Enumeration (moqui.basic.Enumeration)"),
        "{entity}"
    );
    assert!(
        entity.contains("enumTypeId") && entity.contains("moqui.basic.EnumerationType"),
        "{entity}"
    );
    // A DB view entity is built from its rows, so the data view shows them, as Java's.
    let (_, view) = get(
        address,
        &john,
        "/Tools/DataView/ViewDbView?dbViewEntityName=GateView&enumId=GATE_ENUM",
    );
    assert!(
        view.contains("Gate enum type") && !view.contains("is not available"),
        "{view}"
    );
    // A data document's view is an entity to the entity data screens: the find shows its row, and its expression field
    // with `defaultDisplay` `Y` is a display field of the find form.
    let (_, doc) = get(
        address,
        &john,
        "/Tools/Entity/DataEdit/EntityDataFind?selectedEntity=DataDocument.GateDoc&enumId=GATE_ENUM",
    );
    assert!(
        doc.contains(r#"<span class="display String">Gate enum</span>"#) && doc.contains(r#"name="enumLabel""#),
        "{doc}"
    );
    // The conf caches, the screen definitions, and the entity list caches are listed together.
    let (_, cache) = get(address, &john, "/System/Cache/CacheList?filterRegexp=screen.def");
    assert!(cache.contains("cacheName=screen.definition"), "{cache}");
    let (_, cache) = get(
        address,
        &john,
        "/System/Cache/CacheList?filterRegexp=list.moqui.basic.Enum",
    );
    assert!(
        cache.contains("cacheName=entity.record.list.moqui.basic.Enumeration"),
        "{cache}"
    );
    let (_, job) = get(
        address,
        &john,
        "/System/Print/PrintJob/PrintJobDetail?printJobId=GATE_PJ",
    );
    assert!(job.contains("Gate job") && job.contains("Not Sent"), "{job}");
    // ServiceRun's parameter form is auto-fields-service: a field for each in-parameter, the date one a date-time.
    let (_, run) = get(
        address,
        &john,
        "/Tools/Service/ServiceRun?serviceName=org.moqui.impl.BasicServices.convert%23Uom",
    );
    for name in ["amount", "uomId", "toUomId", "effectiveDate"] {
        assert!(run.contains(&format!("name=\"{name}\"")), "{name}: {run}");
    }
    assert!(run.contains("type=\"datetime-local\""), "{run}");
}

/// The text `SystemScreenRenderTests`, `ToolsScreenRenderTests`, and `ToolsRestApiTests` require.
fn the_groovy_specs_text_is_on_the_screens_and_the_rest_api(gate: &crate::common::Gate) {
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    assert_page(
        address,
        &john,
        "/System/Cache/CacheList",
        &["entity.definition", "artifact.tarpit.hits"],
    );
    // Groovy looks for the literal `${artifactName}::en_US` and for `evictionStrategy`. Keys here are
    // `original::locale`, and `evictionStrategy` is on Cache List, not this elements screen. The screen
    // walk above has already localized messages, so an `en_US` key is on the page.
    assert_page(
        address,
        &john,
        "/System/Cache/CacheElements?orderByField=key&cacheName=l10n.message&pageSize=1000",
        &["::en_US", "Elements for Cache [l10n.message]"],
    );
    assert_page(address, &john, "/System/Localization/Messages", &["Add", "Añadir"]);
    assert_page(
        address,
        &john,
        "/System/Localization/EntityFields?entityName=moqui.basic.Enumeration&pkValue=GEOT_STATE",
        &["moqui.basic.Enumeration", "GEOT_STATE"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/UserAccount/UserAccountList?username=john.doe",
        &["john.doe", "John Doe"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/UserAccount/UserAccountDetail?userId=EX_JOHN_DOE",
        &["john.doe@moqui.org", "Administrators (full access)"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/UserGroup/UserGroupList",
        &["Administrators (full access)"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/UserGroup/UserGroupDetail?userGroupId=ADMIN",
        &["System App (via root screen)"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/UserGroup/GroupUsers?userGroupId=ADMIN",
        &["john.doe - John Doe"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/ArtifactGroup/ArtifactGroupList",
        &["All Screens"],
    );
    assert_page(
        address,
        &john,
        "/System/Security/ArtifactGroup/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP",
        &["component://tools/screen/System.xml", "Administrators (full access)"],
    );

    let create = "/Tools/AutoScreen/AutoFind/create";
    let (status, page) = post(
        address,
        &john,
        create,
        &[
            ("aen", "moqui.test.TestEntity"),
            ("testId", "TEST_SCR"),
            ("testMedium", "Screen Test Example"),
        ],
    );
    ran(create, status, &page);
    assert_page(
        address,
        &john,
        "/Tools/AutoScreen/AutoEdit/AutoEditMaster?testId=TEST_SCR&aen=moqui.test.TestEntity",
        &["Screen Test Example"],
    );
    // The Groovy rows look for the seed phrase "Test Name A" in the browser download. This runtime has no such
    // row; the record just created is the one the export has to carry. Browser output calls `edw.writer`, and
    // `makeDataWriter` has `text()` instead, so that call fails. Single-file output uses `edw.file`, which exists.
    // The transition runs actions, so the parameters go in the body.
    for file_type in ["JSON", "XML"] {
        let export_path = std::env::temp_dir().join(format!(
            "moqui-export-{}-{}.{}",
            std::process::id(),
            file_type,
            file_type.to_ascii_lowercase()
        ));
        let _ = std::fs::remove_file(&export_path);
        let path = "/Tools/Entity/DataExport/EntityExport";
        let (status, page) = post(
            address,
            &john,
            path,
            &[
                ("entityNames", "moqui.test.TestEntity"),
                ("dependentLevels", "1"),
                ("fileType", file_type),
                ("output", "file"),
                ("path", export_path.to_str().unwrap()),
            ],
        );
        ran(path, status, &page);
        let written = std::fs::read_to_string(&export_path)
            .unwrap_or_else(|err| panic!("{}: {err}\n{page}", export_path.display()));
        for text in ["Screen Test Example", "testMedium"] {
            assert!(written.contains(text), "{path} {file_type} missing [{text}]: {written}");
        }
        let _ = std::fs::remove_file(&export_path);
    }

    assert_page(
        address,
        &john,
        "/Tools/Service/ServiceReference?serviceName=UserServices",
        &["org.moqui.impl.UserServices.create#UserAccount", "Service Detail"],
    );
    // `#` is `%23` on the wire. ScreenTest keeps it in the parameter map, where a URL would drop it as a fragment.
    // Groovy also looks for `moqui.security.UserAccount.username` and the generated Groovy `ec.service.sync()` call.
    // The parameter element is named `username`, and the actions are not rendered as that Groovy call.
    assert_page(
        address,
        &john,
        "/Tools/Service/ServiceDetail?serviceName=org.moqui.impl.UserServices.create%23UserAccount",
        &["username", "org.moqui.impl.UserServices.create#UserAccount"],
    );

    let create_view = "/Tools/DataView/FindDbView/create";
    let (status, page) = post(
        address,
        &john,
        create_view,
        &[
            ("dbViewEntityName", "UomDbView"),
            ("packageName", "test.basic"),
            ("isDataView", "Y"),
        ],
    );
    ran(create_view, status, &page);
    assert_page(address, &john, "/Tools/DataView/FindDbView", &["UomDbView"]);
    let set_master = "/Tools/DataView/EditDbView/setMasterEntity";
    let (status, page) = post(
        address,
        &john,
        set_master,
        &[
            ("dbViewEntityName", "UomDbView"),
            ("entityAlias", "MASTER"),
            ("entityName", "moqui.basic.Uom"),
        ],
    );
    ran(set_master, status, &page);
    let set_fields = "/Tools/DataView/EditDbView/setMasterFields";
    let (status, page) = post(
        address,
        &john,
        set_fields,
        &[
            ("dbViewEntityName_0", "UomDbView"),
            ("field_0", "moqui.basic.Uom.description"),
            ("dbViewEntityName_1", "UomDbView"),
            ("field_1", "UomType#moqui.basic.Enumeration.description"),
            ("dbViewEntityName_2", "UomDbView"),
            ("field_2", "UomType#moqui.basic.Enumeration.enumTypeId"),
        ],
    );
    ran(set_fields, status, &page);
    // The walk's GateView assertion is unchanged. This is the view the Groovy flow creates.
    assert_page(
        address,
        &john,
        "/Tools/DataView/ViewDbView?dbViewEntityName=UomDbView&orderByField=description",
        &["UomDbView", "Afghani", "Area"],
    );

    // The seed load runs in another process, whose metrics this server does not have. This create is the
    // `moqui.basic` entity the Monitor Artifacts data has to list, once the next one-second collection is indexed.
    let create_status = "/Tools/AutoScreen/AutoFind/create";
    let (status, page) = post(
        address,
        &john,
        create_status,
        &[
            ("aen", "moqui.basic.StatusType"),
            ("statusTypeId", "GATE_ST"),
            ("description", "Gate status type"),
        ],
    );
    ran(create_status, status, &page);
    // The query the Monitor Artifacts dashboard's statistics run: entity creates by name, from the duration metric.
    let artifacts_query = r#"{"query":{"datasetId":"otel_metrics","from":"now-1h","thru":"now+1h","queryString":"name:\"moqui.artifact.duration\" AND moqui.artifact.type:AT_ENTITY AND moqui.artifact.sub_type:create","aggs":{"names":{"terms":{"field":"moqui.artifact.name","size":50},"aggs":{"hits":{"sum":{"field":"count"}}}}}}}"#;
    let started = std::time::Instant::now();
    let (status, page) = loop {
        let got = rest_post_json(address, &john, "/rest/s1/moqui/reports/query", artifacts_query);
        if got.0 == 200 && got.1.contains("moqui.basic.StatusType") {
            break got;
        }
        if started.elapsed() >= std::time::Duration::from_secs(20) {
            break got;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(status, 200, "{}", failure("Monitor Artifacts query", status, &page));
    assert!(
        page.contains("moqui.basic.StatusType"),
        "Monitor Artifacts query missing the entity create: {page}"
    );
    assert_page(address, &john, "/apps/monitor/Artifacts", &["Artifact statistics"]);
    let (status, page) = rest_get(address, &john, "/rest/s1/moqui/basic/geos/USA");
    assert_eq!(status, 200, "{}", failure("geos/USA", status, &page));
    for text in ["United States", "Country"] {
        assert!(page.contains(text), "geos/USA missing [{text}]: {page}");
    }
    let (status, page) = rest_get(address, &john, "/rest/s1/moqui/basic/geos/USA/regions");
    assert_eq!(status, 200, "{}", failure("geos/USA/regions", status, &page));
    assert!(
        !page.contains("Error rendering"),
        "{}",
        failure("geos/USA/regions", status, &page)
    );
    let (status, page) = rest_get(address, &john, "/rest/s1/moqui/email/templates");
    assert_eq!(status, 200, "{}", failure("email/templates", status, &page));
    for text in ["PASSWORD_RESET", "Default Password Reset"] {
        assert!(page.contains(text), "email/templates missing [{text}]: {page}");
    }
}

/// Users for the SQL runners: one in ADMIN without ADMIN_ADV (the Tools app but not `SQL_RUNNER_WEB`), one in no
/// group. Both passwords are moqui.
const SQL_DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="GATE_TOOLS_ONLY" username="gate.tools" userFullName="Gate Tools"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroupMember userGroupId="ADMIN" userId="GATE_TOOLS_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserAccount userId="GATE_PLAIN" username="gate.plain" userFullName="Gate Plain"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.basic.Enumeration enumId="GATE_SQL" description="untouched"/>
</entity-facade-xml>"#;

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Submit the SQL runner form of `screen` as the browser does, the statement in the body.
fn run_sql(address: SocketAddr, auth: &str, screen: &str, sql: &str, limit: &str) -> (u16, String) {
    let body = format!("groupName=transactional&sql={}&limit={limit}", encode(sql));
    let request = http_request(
        "POST",
        &format!("/Tools/Entity/{screen}.html"),
        &[
            ("Authorization", auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &body,
    );
    let (status, _, page) = exchange(address, &request);
    (status, page)
}

fn the_sql_runners_run_for_an_admin_and_refuse_other_users(gate: &crate::common::Gate) {
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    const UPDATE: &str = r#"update enumeration set description = 'changed' where enum_id = 'GATE_SQL'"#;
    const SELECT: &str = r#"select description from enumeration where enum_id = 'GATE_SQL'"#;

    // An admin reads a query to its limit, and no further.
    let (status, page) = run_sql(
        address,
        &john,
        "SqlRunner",
        r#"select enum_id from enumeration order by enum_id"#,
        "3",
    );
    assert_eq!(status, 200, "{page}");
    assert!(!page.contains("Error rendering"), "{page}");
    assert_eq!(page.matches("<tr><td").count(), 3, "{page}");
    assert!(page.contains("Only showing first 3 rows."), "{page}");
    let (_, page) = run_sql(address, &john, "SqlRunner", SELECT, "500");
    assert!(
        page.contains("untouched") && page.contains("Showing all 1 results."),
        "{page}"
    );
    // A failed statement shows its error.
    let (_, page) = run_sql(address, &john, "SqlRunner", "select * from no_such_table", "500");
    assert!(
        page.contains("no_such_table") && page.contains("does not exist"),
        "{page}"
    );

    // Without SQL_RUNNER_WEB, with or without the Tools app, neither runner runs a statement.
    for user in ["gate.tools", "gate.plain"] {
        let auth = basic(user, "moqui");
        for screen in ["SqlRunner", "SqlScriptRunner"] {
            let (status, page) = run_sql(address, &auth, screen, UPDATE, "500");
            assert!(!page.contains("Query altered"), "{user} {screen}: {status}: {page}");
            if user == "gate.tools" {
                assert!(
                    page.contains("User does not have permission to use the SQL Runner"),
                    "{user} {screen}: {status}: {page}"
                );
            } else {
                assert_eq!(status, 403, "{user} {screen}: {page}");
            }
        }
    }
    let (_, page) = run_sql(address, &john, "SqlRunner", SELECT, "500");
    assert!(page.contains("untouched") && !page.contains("changed"), "{page}");

    // The script runner runs the admin's update.
    let (_, page) = run_sql(address, &john, "SqlScriptRunner", UPDATE, "500");
    assert!(!page.contains("Error rendering"), "{page}");
    assert!(page.contains("Query altered 1 rows."), "{page}");
    let (_, page) = run_sql(address, &john, "SqlRunner", SELECT, "500");
    assert!(page.contains("changed"), "{page}");
}
