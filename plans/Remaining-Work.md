# Remaining work

Open items only, grouped by domain. Each item says what is missing and links to where the detail is; what was built, and when, is recorded in the linked plans and guides, not here.

What is not ported on purpose (JSON-RPC, the Groovy and Java runners, the database drivers not in this tree, `service-group`, Swagger 2.0 and RAML, Hazelcast, the embedded SMTP server, vuet and `/vapps`) is in [Parity-Mapping, Excluded on purpose](Parity-Mapping.md#excluded-on-purpose); the non-SQL entity datasources (`ElasticDatasourceFactory`, OrientDB) are a proposal in [Future-Plans](Future-Plans.md). Differences from Java that are kept on purpose are in [Parity-Diffs](Parity-Diffs.md).

## Agents, Assist, A2A, MCP and the gateway

Plan [LLM-A2A-Assist](LLM-A2A-Assist.md); guides [Assist](../docs/ai/Assist.md), [A2A](../docs/ai/A2A.md), [MCP](../docs/ai/MCP.md), [Gateway](../docs/ai/Gateway.md). Every check so far ran on the scripted fake provider, fixture or loopback servers only.

| Area | Item | Source |
|---|---|---|
| Live models | No check has run against a live model: the evaluation suites on Qwen 3.8 27B (at least 80%) and on an 8B model in JSON mode (60%, at most 10% wrong calls), with their `baseline-local-qwen.json` and 8B baselines; live embeddings, transcription and text actions. The provider stream fixtures (`llm/provider/testdata/`) are hand-written from the documented formats; replace them with recorded streams when an endpoint is at hand | 13.1, 13.3, 13.7, 13.13, 13.14 |
| Drawer | A record chip map from the server; "Let Assist save it" on a hand-off; Summarize earlier messages; Chromium smokes for the 13.14 drawer extras (they have node tests only); TUI decisions over `--server` (a key login cannot decide) | docs/ai/Assist.md, docs/ui/TUI.md "Assist" |
| Canvas | The Wide splitter, Page mode and phone tabs; a canvas `Lookup` against real option transitions in the Chromium smoke; an admin field for `AgentDefinition.canvasDialects`; the worker renderer for `vue-sfc`, required before enabling it for agents that read untrusted content (self-navigation and WebRTC put back by the component's own code are only mitigated) | 13.6, 13.8 |
| Window | Reading the context length the server reports; did-you-mean for screen paths (the `screen` tool refuses a bad path with no suggestion) | 13.7 |
| Learning | Lessons from fail-then-succeed and from repeated Adjust | 13.14 |
| Chat agents | Feedback from reactions; manager/worker agents (`parentRunId` is unused); form questions in rooms | 13.9 |
| A2A server | OIDC JWT auth; an `LlmApiClient` `a2a` scope; an A2A tasks admin screen; A2A in `clean#AgentData`; CORS for `/a2a` | docs/ai/A2A.md "Not yet" |
| MCP server | OAuth 2.1 with Moqui as the authorization server and the `LlmApiClient` `mcp` scope; `subscriptions/listen` and `listChanged`; resources and prompts; SSE answers; legacy `initialize` clients; an approval path for clients without URL elicitation | docs/ai/MCP.md "Not yet" |
| Remote clients | Long remote tasks as a RemoteTask pause resumed by push; per-user OAuth sign-in for MCP servers; form elicitations and legacy in-stream requests; remote resources and prompts | 13.12 |
| Gateway | Protocol translation (Chat Completions to Responses or Anthropic models); interrupting a call's blocking upstream request when its caller leaves; making a login key for a client in the admin screen (shown once); cluster-wide requests per minute (counted per node) | docs/ai/Gateway.md "Not built" |
| Apps | The active organization and organizations as a Mantle user-facts provider, and Mantle skills in mr-mantle and mr-marble (no component has a `skill/` directory yet) | [LLM-A2A-Assist](LLM-A2A-Assist.md) §12 |

`allow-remote` on services is not parsed; no remote path calls services by name, so it is documentation only. Provider calls keep the sim-allow exception on the outbound client.

## Mail

Guides [Mail-Inbound](../docs/interfaces/Mail-Inbound.md), [Mail-OAuth](../docs/interfaces/Mail-OAuth.md); mr-boss `plan/impl-drawers-commons.md` §5 M5.

| Area | Item | Source |
|---|---|---|
| Replies | The automatic reply counts as internal a user's address, the domain of a hosted or shared account, or one of its `internalDomains`, and answers only mail with the account's or an identity's address in To or Cc (not mail an alias forwards) | M5c, `send#OutOfOfficeReplies` |
| Sync | Not yet run against a real account (a Gmail app password). A folder gone from the server keeps its local messages (`remoteRemovedDate`); POP3 reads no sync window, newest first. BOSS's mail settings have no control for `EmailServer.storeAllowPlain` (the opt-out that lets a plain `imap` or `pop3` store sign in without STARTTLS or STLS) | M5d, `service/mail/sync.rs`, `inbound.rs` |
| OAuth | Not yet run against real Google and Microsoft clients (the Testing 7-day limit and Google verification) | M5e, docs/interfaces/Mail-OAuth.md |
| Inbound SMTP | No TLS client certificates (the allow-list is the gateway's identity); no BINARYMIME; no failure or delay DSNs of its own; the delivered report waits on the outgoing server for up to 30 seconds inside the store, before the listener answers | M5f, `moqui-host/src/smtp.rs`, `service/mail/delivery.rs` |
| Sending | A held send goes at the job's next minute unless the client calls `send#DueDrafts` when its hold ends | `EmailClientServices` |
| Accounts | `delete#EmailAccount` with deleteLocalCopies runs in the request (a job for large accounts) | `EmailClientServices` |
| Capture | The mail capture records at the SMTP step of `send#EmailMessage`, so a send with no `EmailServer` or an empty SMTP host is not captured; IMAP and POP have no capture. mr-mantle `InboxThreads.spec.js` still makes a send fail and calls `record#SendResult`, which `ec.factory.getTool("MailCapture").enable()` would replace | `service/mail_capture.rs` |

## Monitoring

Plan [Monitoring](Monitoring.md); guide [Monitoring](../docs/tools/Monitoring.md). Not started: the plan's Later stages (server-JS profiling, browser RUM and replay, business operations, regressions, incidents and recovery, day partitions and gRPC).

| Area | Item | Source |
|---|---|---|
| Overhead | No per-milestone overhead baselines were captured; production-scale concurrency, cardinality, long retention and rollup load, storage growth and a multi-host soak are not measured. The per-batch export timing (`monitor_performance.rs`, now protobuf with gzip) has not been re-run against a live collector | Monitoring.md "Overhead" |
| Export | No real collector has received a gzipped request yet (the Docker test `collector_forwards_json_and_raw_protobuf_for_every_signal` would show it); received exponential histograms are not on `/metrics` | `telemetry/otlp.rs`, `telemetry/received.rs` |
| Dashboards | The flame graph is span self time (no stored profiles); the service map and span flame read the newest 10000 spans of a range, so with head sampling they show mostly the always-kept roots, errors and slow spans; the geomap has no zoom or pan, and the TUI geomap shows a country frame as a list, not a map; TUI and print text panels in a repeat show the dashboard's variable values, not the copy's | `report/graph.rs`, `WebrootReport.js`, `moqui-tui` `components/report/graph.rs` |
| Git sync | A pull cannot save a definition another user owns; only the repository's default branch syncs; no test runs the gitECA path end to end (a push over HTTP or SSH, including a pull that deletes) | `report/git_sync.rs` |
| Alerts | No test drives a state write that fails after a real delivery (the retry check stands in for it); a process that dies between marking a notification sent and sending it loses that one notification | `report/alert.rs` |
| Pull | Scrape targets have no relabeling, extra labels, basic auth or service discovery, and OpenMetrics exemplars are ignored; checks are single HTTP requests (no TCP, DNS, certificate expiry or multi-step flows) from this server only; a check that calls an unsampled page of this same server loses its CLIENT child (the page's SERVER span decides the trace first) | `telemetry/scrape.rs`, `telemetry/mod.rs` |
| Patterns, AI | Log patterns group the newest 1,000 matching lines (the documents page cap), and a pattern narrows the search by its constant words as phrases, which can match a little more than the pattern; Monitor › AI groups the newest 1,000 LLM calls, and spans carry no cost (System › LLM › Usage has it) | `report/patterns.rs`, `Monitor/AgentTraces.xml` |

## Chat and presence

| Item | Source |
|---|---|
| Calls (`ChatRoomMember.inCall` is not set); a link preview's picture, through an image proxy so no reader's address leaks (previews are text only, `service/chat_link.rs`); BOSS's chat screen rendering `m-assist-card`, with a Chromium smoke of the card | mr-boss `plan/impl-drawers-commons.md` §5 M10; 13.9 |

## Files and drives

| Item | Source |
|---|---|
| Previews and extracted text of docx, pptx and odt (deferred); PDF fonts, cmaps and JPEG 2000 or JBIG2 images (pdf.js `standard_fonts`, `cmaps` and `wasm` are not vendored: License-Review) | mr-boss `plan/impl-drawers-commons.md` §5 M8 |
| WebDAV: locks are per server (a client stays on one, `resource/locks.rs`), dead properties are not kept, no quota properties | [Sync](../docs/interfaces/Sync.md) |

## Calendar and contacts

| Item | Source |
|---|---|
| Not built in the `VCard` and `ICalendar` tools and the CalDAV and CardDAV servers (`crates/moqui-core/src/pim`, `dav/pim.rs`): sync-collection (clients keep in step by ctag and entity tags), scheduling (CalDAV `schedule-*`), free-busy-query, attendees written by a client, iTIP processing of a received REPLY or COUNTER, VJOURNAL and VFREEBUSY, RSCALE and SKIP (RFC 7529), vCard PHOTO and other binary values, IMPP, ANNIVERSARY, GENDER and related cards. A VTIMEZONE that no IANA zone matches keeps its rule as a POSIX TZ text in the event's `timeZone`, which `expand` reads back. A `RANGE=THISANDFUTURE` override splits its series on import (mr-mantle `store#CalendarFeedEvents`), but a day shift is not applied to the new series' BYDAY or BYMONTHDAY, and RDATEs stay on the old series | [Sync](../docs/interfaces/Sync.md); M4, M7 framework |

## Git and git hosting

Guides [Git-Hosting](../docs/interfaces/Git-Hosting.md), [Revision-Management](../docs/data/Revision-Management.md).

| Area | Item | Source |
|---|---|---|
| Push | No push to another server: gitoxide has none yet. A `db` repository is published by `export#GitRepository` and the git command line; add `push#GitRepository` when gix supports it | `resource/git/fs.rs` |
| Transport | SSH and `file://` fetches run the system `ssh` and `git-upload-pack`; an in-process SSH transport over `russh` (already linked for the SSH server) would drop that | same |
| Formats | SHA-1 repositories only; no submodules, tags as versions, or signed commits | same |
| Hosting | Forks; merges reading `.gitattributes` (merge drivers); revision expressions (`main~1`); merge request notifications are the app's (Mantle and BOSS topics); no `GitUpdate` is spared for a `git://` write now that the framework's own rules listen to writes | `resource/git/hosting.rs`, `service/git_hosting.rs` |
| Hosting | Not served: partial clone (`filter blob:none`), `report-status-v2`, `push-cert`, the dumb HTTP protocol, anonymous read of a public repository | `resource/git/server/` |
| Hosting | `SSH_GIT` is granted to ADMIN only. A permission granted to ALL_USERS counts only with a membership row (as in Java, pinned by `service/eca.rs`), so a grant for every user needs a group of its own | `framework/data/SecurityTypeData.xml` |
| Storage | A `db` delta chain is read a level of bases per query; on Postgres a recursive CTE could read a cold chain in one | `resource/git/db.rs` |
| Storage | Content blobs are in the database only (`ContentBlob.storageKind` `db`); a file system or S3-like backend behind the same store for the chunks | `resource/blob.rs` |
| Storage | A `local` repository's object written through `git://` over 8 MiB is a loose object of its size (only a `db` repository keeps large objects as content blobs); keep large files in LFS | `resource/git/fs.rs` |
| LFS | gc of a `local` repository does not let go of LFS objects no kept file points to (only `db` gc sweeps LFS); a `remote` mirror does not fetch LFS objects; no transfer adapters other than basic (`git-lfs-transfer` over SSH, multipart) | `resource/git/server/lfs.rs` |

## Search

| Item | Source |
|---|---|
| Universal search result links go to the generic entity edit screens until the apps have record screens; the per-document hook predicate is not built | [SearchFacade, REST service and UI](archive/SearchFacade.md) |
| Postgres broad queries (common words: rank, facets, proximity) take tens to hundreds of ms at 50k documents; candidates are a stored per-field `tsvector`, a ranked-row cap, and measuring cover density on a real corpus. Hybrid `k` and weights wait on a relevance set | [Performance, Search](Performance.md) |
| Semantic retrieval has the development hash embedder and `LlmEmbedder` (`embedder="llm"`, a model server's `/v1/embeddings`); there is no built-in local model, and no live embedding model has been run | [Implementation-Phases, Phase 9](archive/Implementation-Phases.md); docs/ai/Gateway.md |
| mr-mantle `search#Universal` can now limit to current party rows (its TODO on `parties.partyThruDate`) | `UniversalSearchServices.xml` |

## Screens, client and shell

| Item | Source |
|---|---|
| Full-page Chromium screenshots can crop the right edge by the scrollbar width: the clip uses content width before capture hides the scrollbar and reflows. Actual DOM/frame bounds and viewport companion images distinguish this artifact from product overflow; the generic full-page capture width still needs correction | `crates/moqui-host/src/test_client/browser.rs`; Accounting P5b1 F99 |
| Client locator clicks use the union bounding rectangle; a wrapped inline link's center can fall between text fragments and hit its surrounding element. Select a visible client-rectangle hit point generically. Accounting attempt links use a rectangular block target and verify the actual hit without forced clicks | `crates/moqui-host/src/test_client/locate.js`; Accounting P5b2a F107 |
| `columns type="mobile"` is chosen when the page loads (width at most 600 px, or the Quasar platform), not on resize; the style guide defines phone by width | gallery finding F22; `WebrootVue.qvt.js` |
| A server-static `qjs`/`qvue` component is cached and not remounted on a query-only navigation (`m-subscreens-active`); a component whose state is in the URL must watch `$root.currentParameters`. Decide: remount on a search change, or keep the documented watcher | gallery finding F30 |
| A `type="none"` transition response to a JSON request carries no messages (as Java), so a background form relies on `background-message`; answering the messages JSON on a `none` response would let the transition's own `<message>` show | gallery finding F3; `screen/render.rs` |
| The gallery's Editor archetype uses `text-area editor-type="md"`, and its label says the co-editing component is not built; `m-coedit` exists (IronStone Notes and Knowledge use it), so the Editor should use it | `example/screen/Gallery/Editor.xml` |
| `m-html-view` treats a `//` image as https and opens every link (a `mailto:` too) in a new window rather than handing it to the composer; the html render mode shows the HTML cleaned to the safe set (no styles, no images but `data:`) at a fixed height | M3 and M5 framework |
| `container-split` keeps its pane widths per id; it has no collapse of the side, no keyboard resize (Quasar's separator takes none) and no list-detail focus handling (Escape back to the list). The wizard's step form is found in the DOM (the first `m-form` of the step outside a list or a dialog), so a step with two forms that should both post has to use one; the wizard does not save a draft itself (each step's transition does) | M4 framework |
| `sri.getShellProviders` is kept per session for up to ten minutes: a grant changed by an admin, or a quick-create or key target screen that changes, shows in another session at once and in the same one after that time (the app root screens are checked every time) | `screen/cache.rs` |
| The 390 px header check (`gate3_browser` tray test) runs without theme plugins: mr-marble's `ActiveOrgNav.qvue` adds a button and its organization's pseudoId as a label, which a long id can still push past a phone's width | framework gaps 2026-10-19 |

## Terminal client

| Item | Source |
|---|---|
| Live checks: the browser terminal's keys and paste in Chrome, Firefox and Safari; the mouse (clicks, the wheel and drag to copy in kitty and GNOME Terminal over SSH, in local mode, and OSC 52 in the browser terminal in Chrome and Firefox); F-keys, Alt and paste in xterm, kitty, GNOME Terminal, tmux and the Linux console | [Screen-TUI-Impl](archive/Screen-TUI-Impl.md) §5 Phase 4 findings, §7 |
| Per-user key bindings and a screen for them (D11); a two-way rich-text editor (3.8; Markdown is styled source); a `diagram` component (Revue's Mermaid widget does not draw a status flow); board sums and the calendar's create and resize in list views; the upstream Revue asks of §7 | same |

## Tooling: lint and XSD

| Item | Source |
|---|---|
| `moqui lint --screens` does not check attributes against the XSD, so an unknown attribute (`display default-text`) passes silently (unknown elements are flagged) | gallery finding F24; `script/screen_lint.rs` |
| Validating the runtime's files against the XSDs shows long-standing violations: 100 of 936 screens (`box-body` without `box-header`, `transition` out of order, `${}` in enumerated attributes, attributes not in the schema; mr-boss `Comms/Settings/MailAccounts/List.xml` puts `row-col` in `section-iterate`), 3 service files, 2 conf files and 2 entity files. Fix the files or loosen the schemas | `runtime/base-component/framework/xsd/` |

## Sessions, notifications and security

| Item | Source |
|---|---|
| A stored NotificationMessage keeps the message's own `showAlert` (`notify/message.rs`), but nothing reads the stored value: the live sockets get the in-memory message. A client or screen that lists stored notifications (unread alerts after a login, a notification history) should read it | PR #134 |
| Login key, app password and external (SSO) logins leave `UserLoginHistory` rows, at most one a minute per user on a node. If automation logging in with a key on every request makes high volumes, record automated logins differently (sampled, counted, or in their own entity) | PR #63 follow-up; `user/login.rs` |

## Apps: IronStone BOSS, MarbleERP and Mantle

| Area | Item | Source |
|---|---|---|
| Accounting | P0–P5 code complete and independently reviewed; paused at the requested checkpoint before P6. Final transfers: Mantle `9a13f26c`, BOSS `49f3001`; final focused gates native 45/45 and BOSS 40/40, affected queue correction 2/2, XML/lint pass with unchanged 121 baseline sites. Further screenshot/browser/full-suite sweeps were omitted under the user's code-completion priority. P6 payable intake/holds/notification filtering and P7–P21 remain unstarted | mr-boss `plan/impl-accounting.md`; `docs/AccountingImplementation.md` |
| IronStone | Products P1–P14 are built and validated: guarded catalog, taxonomy, variants, dated prices and supplier offers, stock/method contracts, standard estimates and forward-cost revaluation, documents, units/packs/labels, durable import, catalog checks and outputs. Remaining Products scope is `impl-products.md` §7: other apps' shared contexts and callers, configurable products/change notices, and the deferred owner-app workflows. Shared contexts extend the server policy in `docs/ProductsSharedScreens.md`. Other business apps contain placeholder screens (they render in qvt, pass `moqui lint --screens`, and `test/AppStructure.spec.js` renders each with a demo record). Projects: `impl-projects.md` §7 Later and the open findings of its §6 | mr-boss `plan/impl-drawers-commons.md`, `plan/impl-projects.md` |
| IronStone | Sales/Reports (`SalesSummary`, `SalesAnalysis`) has no spec that renders it. Marking the BOSS data screens outside Projects and Commons `server-static="qvt"` waits on their specs, which check data in the merged qvt output that a server-static screen leaves to `/actions` | same |
| Tools | The tools screens still call the `StringUtilities`/`WebUtilities` globals (`runtime/base-component/tools/screen`, e.g. `EntityDataFind.xml`) | — |
| Marble ERP (mr-marble) | Acceptance work after the migration: wiki fixtures, exhaustive role and organization isolation, CSRF, every mounted QVT interaction, physical checks and scanners | [MarbleERPMigration, Limits](archive/MarbleERPMigration.md#limits) |
| Mantle | `moqui test mr-mantle` calls 751 of 762 services. Not called: three Google ones (`GooglePartyServices`) and `create#ContactRequestReCaptcha` (need a network); four `TenantSubscriptionServices` (deprecated multi-tenant mode); `TaxServices.calculate#InvoiceSalesTax` (empty upstream); `ReturnServices.returnAndResell#Shipment` (never places its order upstream); `PartyServices.send#DataDocumentNotifications` (needs a search backend's data feed). Unfinished upstream and kept as they are: `generate#Efw2File` (no RT, RU, RV or RF records, no `fileText`), `calculate#SalesTaxLocal`, `post#WorkEffortCost`. Migrating other components' scripts is outside this repository's gates | [ConvertProcess](archive/ConvertProcess.md), [MantleFlowTests](archive/MantleFlowTests.md) |

## Wasm browser host

| Item | Source |
|---|---|
| No login flow: calls run with authorization disabled, except renders (`crates/moqui-browser/src/host.rs`) | [Wasm, Status](archive/Wasm.md) |
| No lock between tabs: without pthreads the WasmFS OPFS backend opens a file through its async access handle, so a second tab opens and writes the database the first has open (`cargo test -p moqui-host --test wasm_browser` warns), and two writers can corrupt the SQLite file. A Web Locks (`navigator.locks`) lock per database name in the worker would make the second tab wait or refuse | same |
| The WebSocket check is `cargo test -p moqui-host --test wasm_browser`, against that test's `/notws` mock, not moqui-host. A live-host mode needs the package served from moqui-host's origin, a signed-in session, and checks of what the real `/notws` does (the mock's `echo:`, `drop` and `Ack` replies are its own) | same |

## Tests

| Item | Source |
|---|---|
| The Java vs MR-Moqui comparison harness and its diff report (a gate artifact) has no code; only search-backend comparison tests exist. The Java runtime must stay runnable | [Implementation-Phases, Tests](archive/Implementation-Phases.md) |
| The Java Spock security suite (`Security*.groovy`) has not been ported | [Proof-Tests](../docs/security/Proof-Tests.md) |
| Optional client-test extensions: move suitable `gate3_browser` flows into client specs; browser users in load tests, an arrival-rate model, server metrics around load runs and merged client reports; screenshot comparison; CDP over a pipe; TypeScript declarations shared with server specs | [ClientTestHarness, Later](archive/ClientTestHarness.md#15-later) |
| Anonymous `GET /menuData/apps/tools` is HTTP 500 (`sri.getScreenUrlInfo()` is undefined in that script) before `getMenuData` would answer 401. The client proof only requires that the body is not the Tools menu | [Proof-Tests](../docs/security/Proof-Tests.md) |
| A full `moqui test` run (all components, 1540 features) runs out of memory in QuickJS about 930 features in: mr-boss screen renders then fail with `out of memory` (about 63 features), though the same specs pass on their own. Something held per run grows: find it (contexts, compiled screens, cached finds) or give each spec file its own runtime | `./moq test-all`, 2026-10-25 |
| Co-editing across servers has no test: `cluster::publish` giving a message its `message_id` (without one a peer refuses `collab-update` posts) is checked only by a unit test of the post. A two-server test of a co-edited document should edit on one and see it on the other | `notws/collab.rs`, `cluster/mod.rs` |
| `example/test/GitHosting.spec.js` "a merge request waits for its review and check, then merges" fails about 1 run in 10: `merge#MergeRequest` right after `create#GitCommitStatus` for the head commit still says "checks not passed: ci/build". Look at how `latest_statuses` (`service/git_hosting.rs`) sees the new row, and whether the merge request's head and the branch head can differ there | `./moq test-all`, 2026-10-25 |

## Framework internals

- **Process-wide global state:** the notify bus and its origin id, the transaction deadline slots and lock tracking, the QZ signer, and `LAST_BOOTED`. One factory runs per process outside tests; `LAST_BOOTED` and the single deadline timer are process-level by design, and the notify bus and lock tracking have no factory handle at their call sites. From the [Code-Quality-Review](archive/Code-Quality-Review.md#not-done-or-left-as-noted).

## Java functionality review

The entity and service modules ([EntityService-Parity-Review](archive/EntityService-Parity-Review.md)) and the screen and web parts ([ScreenWeb-Parity-Review](archive/ScreenWeb-Parity-Review.md)) were reviewed phase by phase against their Java sources, with the accepted differences in [Parity-Diffs](Parity-Diffs.md). The rest of [Parity-Mapping](Parity-Mapping.md) (moqui-lib, moqui-db, context and resources, user and session, transactions and drivers, script and templates, search, notifications and cluster, mail and the tool components, tooling, browser host) has had no phase review yet; the threads to carry through one are [Parity-Mapping, Cross-cutting threads](Parity-Mapping.md#cross-cutting-threads).

## Related documents
[Future-Plans](Future-Plans.md) (digest of the unbuilt [proposals](proposals/)), [Performance-Summary](Performance-Summary.md) (the next phase), [Parity-Mapping](Parity-Mapping.md) and [Parity-Diffs](Parity-Diffs.md) (reference for the review). The index is [README](README.md).
