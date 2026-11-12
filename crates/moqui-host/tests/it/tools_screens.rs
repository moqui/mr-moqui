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
//! whose `render-modes` leave out `html` answers that html is not supported, as Java does. The removed Groovy shell
//! route is absent. The screens and REST calls
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
    <moqui.security.sso.AuthFlow authFlowId="GATE_FLOW" authFlowTypeEnumId="AftOauth" description="Gate sign-in" disabled="Y"/>
    <moqui.basic.email.EmailServer emailServerId="GATE_RELAY" smtpHost="smtp.gate.example.test" smtpPort="465" smtpSsl="Y"/>
    <moqui.entity.sync.EntitySync entitySyncId="GATE_SYNC" description="Gate sync" statusId="EsNotStarted" forPull="N"/>
    <moqui.server.TransferBatch transferId="GATE_IMPORT" targetId="messageTranslations" userId="EX_JOHN_DOE" state="partial"
        filename="gate-translations.csv" fileHash="0000" rowCount="1" stagedDate="2026-01-02 03:04:05"
        requestJson="{&quot;headers&quot;:[&quot;original&quot;,&quot;locale&quot;,&quot;localized&quot;]}"/>
    <moqui.server.TransferItem transferId="GATE_IMPORT" itemSeqId="1" rowNumber="2" result="failed" errorMessage="Gate row reason"
        sourceJson="{&quot;sourceValues&quot;:{&quot;original&quot;:&quot;Gate original&quot;}}"/>
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
    <!-- a business dataset over the document, in a registered feed and waiting for review, for the dataset screens of Dashboards -->
    <moqui.entity.feed.DataFeed dataFeedId="GateFeed" dataFeedTypeEnumId="DTFDTP_MAN_PULL" feedName="Gate feed"/>
    <moqui.entity.feed.DataFeedDocument dataFeedId="GateFeed" dataDocumentId="GateDoc"/>
    <moqui.report.Definition definitionId="gate-business" definitionType="dataset" title="Gate business dataset" ownerId="EX_JOHN_DOE"
        readOnly="N" versionNumber="1"
        bodyJson='{"documentType":"GateDoc","timeField":"_timestamp","signal":"business","rowFilterService":"org.moqui.report.ReportServices.get#UserGroupRowFilter","rowFilterParameters":{"groupField":"enumTypeId"}}'/>
    <!-- the delivery screens of Dashboards: a schedule that goes to its owner alone with one run and its delivery, a
        schedule made in owner mode, and a screen schedule its user set up for themselves -->
    <moqui.screen.ScreenScheduled screenScheduledId="GATE_SCHEDULE" screenPath="/apps/dashboards/Reports" reportDefinitionId="monitor-overview"
        reportOwnerId="EX_JOHN_DOE" userId="EX_JOHN_DOE" cronExpression="0 0 6 ? * MON" renderMode="csv" deliveryMode="personal"
        emailSubject="Gate weekly overview" paused="Y" noResultsAbort="Y" versionNumber="1"/>
    <moqui.screen.ScreenScheduledLock screenScheduledId="GATE_SCHEDULE"/>
    <moqui.screen.ScreenScheduled screenScheduledId="GATE_OWNER_MODE" screenPath="/apps/dashboards/Reports" reportDefinitionId="monitor-overview"
        reportOwnerId="EX_JOHN_DOE" cronExpression="0 0 7 ? * *" renderMode="pdf" saveToLocation="dbresource://gate/reports"
        emailSubject="Gate owner mode" paused="Y" noResultsAbort="Y"/>
    <moqui.screen.ScreenScheduledLock screenScheduledId="GATE_OWNER_MODE"/>
    <moqui.screen.ScreenScheduled screenScheduledId="GATE_SCREEN_SCHEDULE" screenPath="/apps/system/Operations/Jobs" userId="EX_JOHN_DOE"
        cronExpression="0 0 8 ? * *" renderMode="csv" emailSubject="Gate saved list" paused="Y"/>
    <moqui.report.ReportOccurrence occurrenceId="GATE_OCCURRENCE" screenScheduledId="GATE_SCHEDULE" occurrenceKey="GATE_OCCURRENCE"
        occurrenceKind="manual" occurrenceDate="2026-01-02 03:04:05" completedDate="2026-01-02 03:04:06" ownerUserId="EX_JOHN_DOE"
        scheduleRevision="1" definitionId="monitor-overview" definitionVersion="1" renderMode="csv" deliveryMode="personal"
        statusId="RptOccComplete" recipientCount="1" sentCount="1" skippedCount="0" failedCount="0" uncertainCount="0" renderCount="1"
        versionNumber="1"/>
    <moqui.report.ReportRecipientDelivery deliveryId="GATE_DELIVERY" occurrenceId="GATE_OCCURRENCE" userId="EX_JOHN_DOE" channel="email"
        statusId="RptDlvSent" attemptCount="1" attemptDate="2026-01-02 03:04:06"/>
    <!-- the alert screens of Dashboards: an operational rule of the viewer's with a contact point, a shared one and the
        state of one series; a rule over the business dataset, which tells an audience; and the shared contact point -->
    <moqui.report.Definition definitionId="gate-alert" definitionType="alert" title="Gate memory alert" ownerId="EX_JOHN_DOE"
        readOnly="N" versionNumber="1"
        bodyJson='{"query":{"datasetId":"otel_metrics","from":"now-5m","thru":"now","aggregations":[]},"field":"share","reduce":"last","condition":{"type":"above","threshold":90},"intervalSeconds":60,"paused":true,"contactPoints":[{"id":"gate-ops","type":"notification","topic":"ReportAlert","userId":"EX_JOHN_DOE"}],"contactPointRefs":["gate-contact"]}'/>
    <moqui.report.AlertInstance definitionId="gate-alert" seriesId="gate-series" state="Firing" stateSince="2026-01-02 03:04:05"
        lastEvaluated="2026-01-02 03:05:05" labelsJson='{"instance":"gate-node"}' lastValue="97.5"/>
    <moqui.report.Definition definitionId="gate-business-alert" definitionType="alert" title="Gate business alert" ownerId="EX_JOHN_DOE"
        readOnly="N" versionNumber="1"
        bodyJson='{"query":{"datasetId":"gate-business","from":"now-1d","thru":"now","aggregations":[{"id":"rows","type":"count","role":"metric"}]},"field":"rows","reduce":"last","condition":{"type":"above","threshold":0},"intervalSeconds":3600,"paused":true,"audience":{"recipientUserIds":[],"recipientGroupIds":[],"channels":["notification"],"topic":"ReportAlert","deliveryMode":"personal"}}'/>
    <moqui.report.Definition definitionId="gate-contact" definitionType="contactPoint" title="Gate contact" ownerId="EX_JOHN_DOE"
        readOnly="N" versionNumber="1" bodyJson='{"type":"notification","topic":"ReportAlert","userId":"EX_JOHN_DOE"}'/>
</entity-facade-xml>"#;

/// Detail screens with the parameters of a record in [`DATA`] or the seed data.
const DETAILS: &[&str] = &[
    "/System/Cache/CacheElements?cacheName=l10n.message",
    "/System/Print/PrintJob/PrintJobDetail?printJobId=GATE_PJ",
    "/System/Operations/JobRunDetail?jobRunId=GATE_RUN",
    "/System/Operations/JobDetail?jobName=clean_TelemetryData_daily",
    "/System/Security/UserAccount/UserAccountDetail?userId=EX_JOHN_DOE",
    // Its render sets up a TOTP factor for the user, so it is not the user the test signs in as.
    "/System/Security/UserAccount/UserAccountDetail/VerifyTotp?userId=GATE_TOTP_USER",
    "/System/Access/GroupDetail?userGroupId=ADMIN",
    "/System/Access/AccessReview?groups=ADMIN",
    "/System/Access/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP",
    "/System/Security/IdentityProviderDetail?authFlowId=GATE_FLOW",
    // the instance administrator's view of Sessions: the visits by user
    "/System/Security/Sessions?visits=Y",
    // and of the changes: the whole audit log, here an account's rows
    "/System/Audit/AuditChanges?log=Y&changedEntityName=moqui.security.UserAccount",
    // the Data area: an import kept by the framework, and the export wizard at its review
    "/System/Data/ImportDetail?targetId=messageTranslations&operationKey=GATE_IMPORT",
    "/System/Data/DataExport?targetId=messageTranslationsExport&option1=es&review=true",
    "/System/Integrations/MessageTypeDetail?systemMessageTypeId=GateSmsg",
    "/System/Integrations/ConnectionDetail?adapterKey=remote&connectionKey=GateRemote",
    "/System/Integrations/SystemMessageDetail?systemMessageId=GATE_MSG",
    "/System/Integrations/SystemMessageDetail/EditMessageText?systemMessageId=GATE_MSG",
    "/System/Protocols/MailRelayDetail?connectionKey=GATE_RELAY",
    "/System/EntitySync/EntitySyncDetail?entitySyncId=GATE_SYNC",
    "/System/EntitySync/EntitySyncHistory?entitySyncId=GATE_SYNC",
    "/System/Resource/GitRepositoryDetail?repositoryId=GATE_GIT",
    "/System/Visit/VisitDetail?visitId=GATE_VISIT",
    "/System/Instance/InstanceDetail?appInstanceId=GATE_AI",
    "/Tools/Data/Documents/Edit/EditDataDocument?dataDocumentId=GateDoc",
    "/Tools/Data/Documents/Edit/ViewDataDocument?dataDocumentId=GateDoc",
    "/Tools/Data/Documents/Edit/DataDocFields?dataDocumentId=GateDoc",
    "/Tools/Data/Documents/Edit/DataDocFields/FieldList?dataDocumentId=GateDoc",
    "/System/Cache/CacheList?filterRegexp=l10n",
    "/System/Llm/Models/ModelDetail?modelId=default",
    "/System/Llm/Models/EndpointDetail?endpointId=default",
    "/System/Llm/Calls/CallDetail?callId=GATE_CALL",
    "/System/Llm/Triggers/TriggerDetail?triggerId=GATE_TRIGGER",
    "/System/Llm/Gateway/ClientDetail?clientId=GATE_CLIENT",
    "/Tools/Schema/Entity?selectedEntity=moqui.basic.Enumeration",
    "/Tools/Data/Record?selectedEntity=moqui.basic.Enumeration&enumId=PtjNotSent",
    "/Tools/Schema/Entities?viewOption=master",
    "/Tools/Database/SpeedTest?baseCalls=10",
    "/Tools/Database/Tables?viewOption=non-empty&filterRegexp=moqui.basic",
    "/Tools/Data/Browse/AutoFind?aen=moqui.basic.Enumeration",
    "/Tools/Data/Browse/AutoEdit/AutoEditMaster?aen=moqui.basic.StatusType&statusTypeId=PrintJob",
    "/Tools/Data/Browse/AutoEdit/AutoEditDetail?aen=moqui.basic.StatusType&den=moqui.basic.StatusItem&statusTypeId=PrintJob",
    "/Tools/Data/Views/ViewDbView?dbViewEntityName=GateView",
    "/Tools/Data/Views/ViewDbView?dbViewEntityName=GateView&enumId=GATE_ENUM",
    "/Tools/Data/Views/EditDbView?dbViewEntityName=GateView",
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
    if !crate::common::own_process() {
        return;
    }
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

    // The obsolete Groovy shell has no screen or placeholder route.
    let (status, shell) = get(address, &john, "/Tools/GroovyShell");
    assert_eq!(status, 404, "{shell}");
    let (status, logs) = get(address, &john, "/apps/system/Monitor/Logs");
    assert_eq!(status, 200, "{logs}");
    assert!(!logs.contains("Error rendering"), "{logs}");

    // The detail screens show their records.
    let (_, entity) = get(
        address,
        &john,
        "/Tools/Schema/Entity?selectedEntity=moqui.basic.Enumeration",
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
        "/Tools/Data/Views/ViewDbView?dbViewEntityName=GateView&enumId=GATE_ENUM",
    );
    assert!(
        view.contains("Gate enum type") && !view.contains("is not available"),
        "{view}"
    );
    // A data document's view is an entity to the data grid: its rows (with a condition) show its row.
    let source = encode(r#"{"type":"entity","entityName":"DataDocument.GateDoc","conditions":{"enumId":"GATE_ENUM"}}"#);
    let (_, doc) = get(
        address,
        &john,
        &format!("/Tools/Data/Grid/gridRows?sourceJson={source}"),
    );
    assert!(
        doc.contains("Gate enum") && doc.contains("GATE_ENUM / Gate enum") && !doc.contains("\"errors\" : \""),
        "{doc}"
    );
    let (_, columns) = get(
        address,
        &john,
        &format!("/Tools/Data/Grid/gridColumns?sourceJson={source}"),
    );
    // the document's fields are its columns, its expression field among them
    assert!(
        columns.contains("\"description\"") && columns.contains("\"enumLabel\""),
        "{columns}"
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
    // The dataset screens of Dashboards, with no mount: the business datasets first, with what a new one may be made
    // over and Tools' words for the scope field, then Tools' other datasets with the fields of the one chosen.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Datasets?otherDatasetId=otel_logs",
        &[
            "Gate business dataset",
            "Needs review",
            "data-dialog=\"CreateDataset\"",
            "Gate Document (gate)",
            "Scope field",
            "Other datasets",
            "New dataset definition",
            "Fields of otel_logs",
            "severity_text",
        ],
    );
    // A dataset for its steward: the checks, the scope by the policy's registered parameter, and its definition in
    // Tools' editor.
    assert_page(
        address,
        &john,
        "/apps/dashboards/DatasetDetail?datasetId=gate-business",
        &[
            "Admission checks",
            "indexed by the GateFeed feed",
            "user groups on enumTypeId",
            "/apps/dashboards/DatasetDetail/admitDataset",
            "/apps/dashboards/Definition?definitionId=gate-business",
        ],
    );
    // A dataset made here has no prefix before its short name, and its scope field goes under the parameter its policy
    // registers.
    let create = "/apps/dashboards/Datasets/createDataset";
    let (status, page) = post(
        address,
        &john,
        create,
        &[
            ("title", "Gate made dataset"),
            ("name", "Gate made"),
            ("dataDocumentId", "GateDoc"),
            ("scopePolicyId", "ReportUserGroups"),
            ("scopeField", "enumId"),
        ],
    );
    ran(create, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/DatasetDetail?datasetId=gate-made",
        &["Gate made dataset", "user groups on enumId"],
    );
    // Its document's fields, the expression one among them, with the engine's functions and operators.
    assert_page(
        address,
        &john,
        "/apps/dashboards/DatasetFields?datasetId=gate-business",
        &[
            "Fields of Gate business dataset",
            "enumLabel",
            "count distinct",
            "not between",
        ],
    );
    // Its rows: the steward reads it before admission; a dataset that is not a business one is not previewed here.
    assert_page(
        address,
        &john,
        "/apps/dashboards/DatasetView?datasetId=gate-business",
        &["Dataset", "Through"],
    );
    assert_page(
        address,
        &john,
        "/apps/dashboards/DatasetView?datasetId=otel_logs",
        &["Dataset otel_logs is not available"],
    );
    // The delivery screens of Dashboards, with no mount: the viewer's schedules (the one checked for each recipient, the
    // owner-mode one and the screen schedule of their own), New schedule with people and groups for one who may deliver
    // to others, and Tools' link to Reports.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Schedules",
        &[
            "Gate weekly overview",
            "Gate owner mode",
            "Gate saved list",
            "Jobs (saved list)",
            "data-dialog=\"NewSchedule\"",
            "People and groups",
            "/apps/dashboards/Reports",
            "/apps/dashboards/ScheduledReportDetail?screenScheduledId=GATE_SCHEDULE",
            "/apps/dashboards/ReportDelivery?occurrenceId=GATE_OCCURRENCE",
        ],
    );
    assert_page(
        address,
        &john,
        "/apps/dashboards/Schedules?view=received",
        &["Monitor overview", "Open my report"],
    );
    // One schedule: its timing, its recipients, the viewer's own preview and its runs; an owner-mode one has its timing
    // and output changed here (its location kept), and a screen schedule has its timing alone.
    assert_page(
        address,
        &john,
        "/apps/dashboards/ScheduledReportDetail?screenScheduledId=GATE_SCHEDULE",
        &[
            "Timing and output",
            "Sent to you alone.",
            "Change recipients",
            "Your preview",
            "1 sent",
            "/apps/dashboards/ScheduledReportDetail/storeSchedule",
        ],
    );
    let owner_mode = "/apps/dashboards/ScheduledReportDetail?screenScheduledId=GATE_OWNER_MODE";
    assert_page(
        address,
        &john,
        owner_mode,
        &[
            "· PDF · Owner mode",
            "As now:",
            "This schedule delivers in owner mode",
            "Its timing and output change here",
            "/apps/dashboards/ScheduledReportDetail/storeSchedule",
        ],
    );
    let store = "/apps/dashboards/ScheduledReportDetail/storeSchedule";
    let (status, page) = post(
        address,
        &john,
        store,
        &[
            ("screenScheduledId", "GATE_OWNER_MODE"),
            ("expectedRevision", "0"),
            ("cronSelected", "0 0 6 ? * MON-FRI"),
            ("renderMode", "csv"),
            ("noResultsAbort", "Y"),
            ("emailSubject", "Gate owner mode"),
        ],
    );
    ran(store, status, &page);
    // the new format in the heading, and the new time one of the presets (no "As now" choice); still in owner mode
    let (status, page) = get(address, &john, owner_mode);
    assert!(
        status == 200
            && page.contains("· CSV · Owner mode")
            && !page.contains("As now:")
            && page.contains("This schedule delivers in owner mode"),
        "{}",
        failure(owner_mode, status, &page)
    );
    assert_page(
        address,
        &john,
        "/apps/dashboards/ScheduledReportDetail?screenScheduledId=GATE_SCREEN_SCHEDULE",
        &["Jobs (saved list)", "Timing and output"],
    );
    // A schedule made here: of one of Tools' own dashboards, which an app that hides Tools' definitions does not offer,
    // for the viewer alone, with the engine's own template and recipient cap.
    let create = "/apps/dashboards/Schedules/createSchedule";
    let (status, page) = post(
        address,
        &john,
        create,
        &[
            ("newDefinitionId", "monitor-services"),
            ("cronSelected", "0 0 6 ? * MON-FRI"),
            ("renderMode", "csv"),
            ("emailSubject", "Gate made schedule"),
            ("audienceKind", "personal"),
        ],
    );
    ran(create, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/Schedules?state=active",
        &["Gate made schedule", "Weekdays"],
    );
    // A run: what ran, each recipient's outcome and the viewer's own delivery, with its status export.
    assert_page(
        address,
        &john,
        "/apps/dashboards/ReportDelivery?occurrenceId=GATE_OCCURRENCE",
        &[
            "Monitor overview",
            "Schedule revision 1, report version 1",
            "Your delivery",
            "Open my report",
            "/apps/dashboards/ReportDelivery/exportAudit",
        ],
    );
    let (status, audit) = get(
        address,
        &john,
        "/apps/dashboards/ReportDelivery/exportAudit?occurrenceId=GATE_OCCURRENCE",
    );
    assert!(
        status == 200 && audit.starts_with("Report,Run,Kind,Delivery") && audit.contains(",Sent,"),
        "{status}: {audit}"
    );
    // Reports keeps the previews, the composer and the owner-mode form, and links the schedules and the playlists.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Reports",
        &[
            "Preview and download",
            "Compose a multi-dashboard report",
            "Schedule a report in owner mode",
            "/apps/dashboards/Schedules",
            "/apps/dashboards/Playlists",
        ],
    );
    // The alert screens of Dashboards, with no mount: every rule the viewer reads with its kind, its owner, whom it tells
    // and its state (the engine's get#AlertState), the state of each series, the shipped rules among them, the shared
    // items and New alert rule.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Alerts",
        &[
            "Gate memory alert",
            "Operational · You",
            "2 contact points",
            "Gate business alert",
            "Business · You",
            "Its owner",
            "High memory use",
            "Operational · Shipped",
            "Series states",
            "instance gate-node",
            "Firing",
            "97.5",
            "New alert rule",
            "Shared contact points",
            "Gate contact",
            "/apps/dashboards/AlertRule?definitionId=gate-alert",
            "/apps/dashboards/AlertShared?definitionId=gate-contact",
            "/apps/dashboards/Alerts/pauseAlerts",
        ],
    );
    assert_page(
        address,
        &john,
        "/apps/dashboards/Alerts?view=templates",
        &[
            "High memory use",
            "it runs only once copied",
            "/apps/dashboards/Alerts/copyAlert",
        ],
    );
    // An operational rule for its owner: the basis with its query typed, its contact points, the shared item it names,
    // its silences and mute timings, and the state of its series.
    assert_page(
        address,
        &john,
        "/apps/dashboards/AlertRule?definitionId=gate-alert",
        &[
            "Reading and condition",
            "fires when share above 90",
            "Evaluated every (seconds)",
            "Routing labels (JSON)",
            "Contact points",
            "gate-ops",
            "data-dialog=\"AddContactPoint\"",
            "Gate contact",
            "One-off silences",
            "Recurring mute timings",
            "instance gate-node",
            "Firing",
            "/apps/dashboards/AlertRule/storeAlert",
            "/apps/dashboards/AlertRule/addContact",
            "/apps/dashboards/Alerts",
        ],
    );
    // A rule over business data tells an audience in place of contact points; its silences and mute timings stay.
    let (status, business) = get(
        address,
        &john,
        "/apps/dashboards/AlertRule?definitionId=gate-business-alert",
    );
    assert!(
        status == 200
            && !business.contains("Error rendering")
            && business.contains("Tells its owner alone, in the app.")
            && business.contains("data-dialog=\"EditAudience\"")
            && business.contains("Nobody has been told yet.")
            && business.contains("One-off silences")
            && !business.contains("AddContactPoint"),
        "{status}: {business}"
    );
    // A new rule starts from nothing, with its ID and its query to type; one made here keeps the query and takes a
    // contact point.
    assert_page(
        address,
        &john,
        "/apps/dashboards/AlertRule",
        &["New alert rule", "Rule ID", "Create alert"],
    );
    let store = "/apps/dashboards/AlertRule/storeAlert";
    let (status, page) = post(
        address,
        &john,
        store,
        &[
            ("ruleId", "gate-made-alert"),
            ("title", "Gate made alert"),
            (
                "queryJson",
                r#"{"datasetId":"otel_metrics","from":"now-5m","thru":"now","aggregations":[]}"#,
            ),
            ("conditionType", "above"),
            ("threshold", "5"),
            ("intervalSeconds", "120"),
            ("paused", "Y"),
            ("labelsJson", "{}"),
        ],
    );
    ran(store, status, &page);
    let add = "/apps/dashboards/AlertRule/addContact";
    let (status, page) = post(
        address,
        &john,
        add,
        &[
            ("definitionId", "gate-made-alert"),
            ("contactId", "gate-made-contact"),
            ("contactType", "notification"),
            ("topic", "ReportAlert"),
            ("contactUserId", "EX_JOHN_DOE"),
            ("matchersJson", "{}"),
        ],
    );
    ran(add, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/AlertRule?definitionId=gate-made-alert",
        &["Gate made alert", "fires when above 5", "gate-made-contact"],
    );
    // Playlists, the wall display, sharing, Analysis and Drill, with no mount. A playlist made on Playlists is listed
    // with its player and its full screen page, and its owner shares it with a group of their own.
    let create = "/apps/dashboards/Playlists/createPlaylist";
    let (status, page) = post(
        address,
        &john,
        create,
        &[
            ("playlistId", "gate-playlist"),
            ("playlistTitle", "Gate playlist"),
            ("playlistDashboardIds", "monitor-overview"),
            ("playlistSeconds", "15"),
        ],
    );
    ran(create, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/Playlists",
        &[
            "Gate playlist",
            "New playlist",
            "Share a playlist",
            "Play full screen",
            "/apps/dashboards/Playlist?definitionId=gate-playlist",
            "/apps/dashboards/WallDisplay?definitionId=gate-playlist",
        ],
    );
    let share = "/apps/dashboards/Playlists/sharePlaylist";
    let (status, page) = post(
        address,
        &john,
        share,
        &[("shareDefinitionId", "gate-playlist"), ("userGroupId", "ADMIN")],
    );
    ran(share, status, &page);
    assert_page(address, &john, "/apps/dashboards/Playlists", &["Shared with "]);
    assert_page(
        address,
        &john,
        "/apps/dashboards/Playlist?definitionId=gate-playlist",
        &[
            "Gate playlist",
            "All playlists",
            "/apps/dashboards/Dashboard?definitionId=monitor-overview",
        ],
    );
    // The wall display is a bare page of its own, and its data is a dashboard of the stored playlist over Tools' range,
    // whatever range the request names; an index outside the playlist answers the one fixed sentence.
    assert_page(
        address,
        &john,
        "/apps/dashboards/WallDisplay?definitionId=gate-playlist",
        &[
            "data-moqui-island=\"wall-display\"",
            "/js/WebrootPublic.qvt.js",
            "<title>Gate playlist</title>",
        ],
    );
    let (status, data) = get(
        address,
        &john,
        "/apps/dashboards/WallDisplay/data?definitionId=gate-playlist&index=0&from=now-30y",
    );
    assert!(status == 200 && data.contains("monitor-overview"), "{status}: {data}");
    let from = data
        .split("\"from\" : \"")
        .nth(1)
        .and_then(|rest| rest.get(..4))
        .and_then(|year| year.parse::<i32>().ok());
    assert!(
        from.is_some_and(|year| year >= 2026),
        "the wall display reads its own range: {data}"
    );
    let (status, data) = get(
        address,
        &john,
        "/apps/dashboards/WallDisplay/data?definitionId=gate-playlist&index=7",
    );
    assert!(status != 200 && !data.contains("monitor-overview"), "{status}: {data}");
    // A dashboard of the viewer's own has Share, with their own groups; shared, it says with whom.
    let save = "/apps/dashboards/Definition/save";
    let (status, page) = post(
        address,
        &john,
        save,
        &[
            ("definitionId", "gate-board"),
            ("definitionType", "dashboard"),
            ("definitionTitle", "Gate board"),
            ("definitionVersion", "0"),
            (
                "definitionJson",
                r#"{"panels":{"counts":{"title":"Gate counts","visualization":"table","queries":[{"datasetId":"otel_metrics","aggregations":[]}]}}}"#,
            ),
        ],
    );
    ran(save, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/Dashboard?definitionId=gate-board",
        &["Gate board", "Share", "Save sharing", "the one you choose"],
    );
    let share = "/apps/dashboards/Dashboard/share";
    let (status, page) = post(
        address,
        &john,
        share,
        &[("definitionId", "gate-board"), ("userGroupId", "ADMIN")],
    );
    ran(share, status, &page);
    assert_page(
        address,
        &john,
        "/apps/dashboards/Dashboard?definitionId=gate-board",
        &["It is shared with "],
    );
    // Analysis: the choices run as the viewer, and saved as a library panel on that dashboard.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Analysis?datasetId=otel_metrics&run=Run",
        &["Source", "Result", "Computed from ", "Save analysis"],
    );
    let save = "/apps/dashboards/Analysis/saveAnalysis";
    let (status, page) = post(
        address,
        &john,
        save,
        &[
            ("datasetId", "otel_metrics"),
            ("title", "Gate analysis"),
            ("dashboardId", "gate-board"),
        ],
    );
    ran(save, status, &page);
    assert_page(address, &john, "/apps/dashboards/Panels", &["Gate analysis"]);
    // Drill: a panel of that dashboard, its breakdown and its records, back to the dashboard and on to Analysis.
    assert_page(
        address,
        &john,
        "/apps/dashboards/Drill?definitionId=gate-board&panelId=counts",
        &[
            "Gate counts",
            "The whole panel",
            "Breakdown",
            "Source records",
            "/apps/dashboards/Dashboard?definitionId=gate-board",
            "/apps/dashboards/Analysis?datasetId=otel_metrics",
        ],
    );
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
        &["::en_US", "Cache entries", "l10n.message"],
    );
    // The instance administrator's stored rows of Settings › Localization (`rows=Y`), messages and labels.
    assert_page(
        address,
        &john,
        "/System/Settings/Localization?rows=Y&original=Add&original_op=equals",
        &["Add", "Añadir", "Instance administrator", "New message row"],
    );
    assert_page(
        address,
        &john,
        "/System/Settings/Localization?rows=Y&kind=labels&entityName=moqui.basic.Enumeration&pkValue=GEOT_STATE",
        &["moqui.basic.Enumeration", "GEOT_STATE", "New label row"],
    );
    // The conf is System's own: its link is shown to a user who opens System by their own grant.
    assert_page(
        address,
        &john,
        "/System/Settings/InstanceSettings",
        &["Instance preferences", "Open the conf"],
    );
    // The Data area: Tools' own data screens are linked for a user who opens Tools by their own grant; an import
    // target started in a wizard is only named where no mount gives the wizard; a seeded import shows its rows with
    // the retry and no wizard link; the export wizard reviews the framework's own target.
    assert_page(
        address,
        &john,
        "/System/Data/Imports?fromDate=2026-01-01&thruDate=2026-02-01",
        &[
            "Entity data of the instance itself, in Tools",
            "/apps/tools/Data/Load",
            "/apps/tools/Data/Snapshots",
            "Message translations",
            "The import wizard, in an app that has it",
            "gate-translations.csv",
        ],
    );
    // and not for one who opens System but not Tools (the import above is not theirs: they see the targets)
    let system_only = basic("gate.system", "moqui");
    let imports = "/System/Data/Imports?fromDate=2026-01-01&thruDate=2026-02-01";
    let (status, page) = get(address, &system_only, imports);
    assert_eq!(status, 200, "{}", failure(imports, status, &page));
    assert!(
        page.contains("Import targets")
            && page.contains("Message translations")
            && !page.contains("Error rendering")
            && !page.contains("Entity data of the instance itself, in Tools")
            && !page.contains("/apps/tools/Data/Load"),
        "{}",
        failure(imports, status, &page)
    );
    assert_page(
        address,
        &john,
        "/System/Data/ImportDetail?targetId=messageTranslations&operationKey=GATE_IMPORT",
        &[
            "gate-translations.csv",
            "Partly applied",
            "original: Gate original",
            "Gate row reason",
            "Retry failed rows",
        ],
    );
    assert_page(
        address,
        &john,
        "/System/Data/DataExport?targetId=messageTranslationsExport&option1=es&review=true",
        &[
            "3. Review and export",
            "Locale (empty for every locale): es",
            "in one export)",
        ],
    );
    assert_page(
        address,
        &john,
        "/System/Data/Exports",
        &["Export targets", "Message translations", "/apps/tools/Data/Export"],
    );
    assert_page(
        address,
        &john,
        "/System/Data/Backups",
        &["Not included in an archive", "Archives"],
    );
    // The registries (plans/archive/Admin-Screens-Down.md A6): Health lists the framework's own checks, each fix place a
    // System screen; the support summary and its redacted JSON; the settings index links the System screens of the
    // framework's entries; and the home's attention list is the checks that are not ready.
    // A row is "Provider: Check" where the titles differ; the header has the sign-in failures of the last day, linked
    // to the visits; report mail with no mail server is fixed at the mail relays, not at the email templates.
    let health_path = "/System/Operations/Health";
    let (status, health) = get(address, &john, health_path);
    assert_eq!(status, 200, "{}", failure(health_path, status, &health));
    for text in [
        "Support summary",
        "Administrator factors",
        "Database backup: Off-site mirror",
        "Backups are not mirrored off the server",
        "/Integrations/ConnectionDetail?adapterKey=remote",
        "Check again",
        "Sign-in failures, last 24 hours",
        "href=\"/System/Visit\"",
        "Report emails have no mail server to send through",
        "/Protocols/MailRelays",
    ] {
        assert!(
            health.contains(text),
            "{health_path} missing [{text}]: {}",
            failure(health_path, status, &health)
        );
    }
    assert!(
        !health.contains("Error rendering") && !health.contains("moqui.basic.email.EmailTemplate"),
        "{}",
        failure(health_path, status, &health)
    );
    assert_page(
        address,
        &john,
        "/System/Operations/Health?providerKey=admin.jobRuns",
        &["Job runs", "Check again"],
    );
    assert_page(
        address,
        &john,
        "/System/Operations/SupportSummary",
        &["Effective settings", "Download JSON", "Off-site mirror", "Prepared "],
    );
    assert_page(
        address,
        &john,
        "/System/Operations/SupportSummary/summaryJson",
        &[
            "moqui.admin.supportSummary",
            "\"redacted\": true",
            "\"preparedBy\": null",
        ],
    );
    assert_page(
        address,
        &john,
        "/System/Settings/SettingsHome",
        &[
            "Instance settings",
            "/Settings/InstanceSettings",
            "Mail relays",
            "/Protocols/MailRelays",
            "Notification topics",
        ],
    );
    assert_page(
        address,
        &john,
        "/System/Home",
        &["Needs attention", "Backups are not mirrored off the server"],
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
        "/System/Access/Groups",
        &["Administrators (full access)"],
    );
    // the group's page with the instance administrator's part: its authorizations and its members by account
    assert_page(
        address,
        &john,
        "/System/Access/GroupDetail?userGroupId=ADMIN",
        &["System App (via root screen)", "john.doe - John Doe"],
    );
    assert_page(address, &john, "/System/Access/ArtifactGroups", &["All Screens"]);
    assert_page(
        address,
        &john,
        "/System/Access/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP",
        &["component://tools/screen/System.xml", "Administrators (full access)"],
    );

    let create = "/Tools/Data/Browse/AutoFind/create";
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
        "/Tools/Data/Browse/AutoEdit/AutoEditMaster?testId=TEST_SCR&aen=moqui.test.TestEntity",
        &["Screen Test Example"],
    );
    // The Groovy rows look for the seed phrase "Test Name A" in the browser download. This runtime has no such
    // row; the record just created is the one the export has to carry. The transition runs actions, so the
    // parameters go in the body. Browser output is the form's default: the writer's text sent as a download, and
    // more than one entity as CSV a zip. The form's empty filter, and Java's `[:]`, are no filter.
    let path = "/Tools/Data/Export/EntityExport";
    for (file_type, filter) in [("XML", ""), ("JSON", "[:]"), ("CSV", "{testId: 'TEST_SCR'}")] {
        let (status, page) = post(
            address,
            &john,
            path,
            &[
                ("entityNames", "moqui.test.TestEntity"),
                ("dependentLevels", "0"),
                ("filterMap", filter),
                ("fileType", file_type),
                ("output", "browser"),
            ],
        );
        ran(path, status, &page);
        assert!(
            page.contains("Screen Test Example"),
            "{path} {file_type} to the browser has no record: {page}"
        );
    }
    let (status, page) = post(
        address,
        &john,
        path,
        &[
            ("entityNames", "moqui.test.TestEntity,moqui.basic.Enumeration"),
            ("fileType", "CSV"),
            ("output", "browser"),
        ],
    );
    ran(path, status, &page);
    assert!(
        page.starts_with("PK"),
        "{path} of two entities as CSV is not a zip: {}",
        page.chars().take(200).collect::<String>()
    );
    for file_type in ["JSON", "XML"] {
        let export_path = std::env::temp_dir().join(format!(
            "moqui-export-{}-{}.{}",
            std::process::id(),
            file_type,
            file_type.to_ascii_lowercase()
        ));
        let _ = std::fs::remove_file(&export_path);
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

    let create_view = "/Tools/Data/Views/FindDbView/create";
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
    assert_page(address, &john, "/Tools/Data/Views/FindDbView", &["UomDbView"]);
    let set_master = "/Tools/Data/Views/EditDbView/setMasterEntity";
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
    let set_fields = "/Tools/Data/Views/EditDbView/setMasterFields";
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
        "/Tools/Data/Views/ViewDbView?dbViewEntityName=UomDbView&orderByField=description",
        &["UomDbView", "Afghani", "Area"],
    );

    // The seed load runs in another process, whose metrics this server does not have. This create is the
    // `moqui.basic` entity the Monitor Artifacts data has to list, once the next one-second collection is indexed.
    let create_status = "/Tools/Data/Browse/AutoFind/create";
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
    assert_page(address, &john, "/apps/system/Monitor/Artifacts", &["Artifact statistics"]);
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
/// group; and one who opens System but not Tools. Their passwords are moqui.
const SQL_DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="GATE_TOOLS_ONLY" username="gate.tools" userFullName="Gate Tools"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroupMember userGroupId="ADMIN" userId="GATE_TOOLS_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserAccount userId="GATE_PLAIN" username="gate.plain" userFullName="Gate Plain"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.basic.Enumeration enumId="GATE_SQL" description="untouched"/>
    <!-- one who opens System by their own grant and reads the imports, but not Tools: the Data area's Tools links -->
    <moqui.security.UserAccount userId="GATE_SYSTEM_ONLY" username="gate.system" userFullName="Gate System"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="GATE_SYSTEM_ONLY" description="Gate System only"/>
    <moqui.security.UserGroupMember userGroupId="GATE_SYSTEM_ONLY" userId="GATE_SYSTEM_ONLY" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="GATE_SYSTEM_ONLY" userPermissionId="ADMIN_TRANSFER_VIEW" fromDate="0"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="GATE_SYSTEM_ONLY" userGroupId="GATE_SYSTEM_ONLY" artifactGroupId="SYSTEM_APP"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
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

/// Submit the SQL screen's terminal and plain-page form (`Tools/SqlTui`) as the browser does, the statement in the body:
/// read only, or with `changes` as Allow changes does it: a first submit lists the statements that may change data with
/// the confirmation token (`classify#Sql`), and a second with that token runs them.
fn run_sql(address: SocketAddr, auth: &str, changes: bool, sql: &str, limit: &str) -> (u16, String) {
    // the run is a POST to the screen's run transition, which keeps what it gave in the session and redirects to the
    // page: the page is read with the session the POST was given
    let post = |extra: &str| {
        let body = format!("db=group:transactional&sql={}&limit={limit}{extra}", encode(sql));
        let request = http_request(
            "POST",
            "/Tools/SqlTui/run.html",
            &[
                ("Authorization", auth),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &body,
        );
        let (status, headers, page) = exchange(address, &request);
        let header = |name: &str| {
            headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        let Some(location) = header("location").into_iter().next() else {
            return (status, page);
        };
        let cookie = header("set-cookie")
            .iter()
            .filter_map(|value| value.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let path = location
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|at| rest[at..].to_string()))
            .unwrap_or(location);
        let (base, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let target = if base.ends_with(".html") {
            path.clone()
        } else if query.is_empty() {
            format!("{base}.html")
        } else {
            format!("{base}.html?{query}")
        };
        let get = http_request(
            "GET",
            &target,
            &[("Authorization", auth), ("Cookie", cookie.as_str())],
            "",
        );
        let (status, _, page) = exchange(address, &get);
        (status, page)
    };
    if !changes {
        return post("");
    }
    let (status, page) = post("&allowChanges=true");
    let token = page.split_once(r#"name="token""#).and_then(|(before, after)| {
        // the value is an attribute of the same input, before or after its name
        let tag_start = before.rfind('<').map_or("", |at| &before[at..]);
        let tag_end = after.split('>').next().unwrap_or("");
        [tag_start, tag_end]
            .iter()
            .find_map(|part| {
                part.split_once(r#"value=""#)
                    .and_then(|(_, rest)| rest.split('"').next())
            })
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    // on a production instance the confirmation also asks for the database name, as "Type the database name (moqui) ..."
    let name = page
        .split_once("Type the database name (")
        .and_then(|(_, rest)| rest.split(')').next())
        .map(|name| format!("&confirmDatabaseName={}", encode(name)))
        .unwrap_or_default();
    match token {
        Some(token) => post(&format!("&allowChanges=true&token={}{name}", encode(&token))),
        None => (status, page),
    }
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
        false,
        r#"select enum_id from enumeration order by enum_id"#,
        "3",
    );
    assert_eq!(status, 200, "{page}");
    assert!(!page.contains("Error rendering"), "{page}");
    assert_eq!(page.matches("<tr><td").count(), 3, "{page}");
    assert!(page.contains("Only showing first 3 rows."), "{page}");
    let (_, page) = run_sql(address, &john, false, SELECT, "500");
    assert!(
        page.contains("untouched") && page.contains("Showing all 1 results."),
        "{page}"
    );
    // A failed statement shows its error.
    let (_, page) = run_sql(address, &john, false, "select * from no_such_table", "500");
    assert!(
        page.contains("no_such_table") && page.contains("does not exist"),
        "{page}"
    );

    // Without SQL_RUNNER_WEB, with or without the Tools app, neither runner runs a statement.
    for user in ["gate.tools", "gate.plain"] {
        let auth = basic(user, "moqui");
        for changes in [false, true] {
            let (status, page) = run_sql(address, &auth, changes, UPDATE, "500");
            assert!(
                !page.contains("Query altered"),
                "{user} changes {changes}: {status}: {page}"
            );
            if user == "gate.tools" {
                assert!(
                    page.contains("User does not have permission to use the SQL Runner"),
                    "{user} changes {changes}: {status}: {page}"
                );
            } else {
                assert_eq!(status, 403, "{user} changes {changes}: {page}");
            }
        }
    }
    let (_, page) = run_sql(address, &john, false, SELECT, "500");
    assert!(page.contains("untouched") && !page.contains("changed"), "{page}");
    // Read only, the admin's update is refused by the database.
    let (_, page) = run_sql(address, &john, false, UPDATE, "500");
    assert!(!page.contains("Query altered"), "{page}");
    let (_, page) = run_sql(address, &john, false, SELECT, "500");
    assert!(page.contains("untouched"), "{page}");

    // With Allow changes and its confirmation the admin's update runs.
    let (_, page) = run_sql(address, &john, true, UPDATE, "500");
    assert!(!page.contains("Error rendering"), "{page}");
    assert!(page.contains("Query altered 1 rows."), "{page}");
    let (_, page) = run_sql(address, &john, false, SELECT, "500");
    assert!(page.contains("changed"), "{page}");
}
