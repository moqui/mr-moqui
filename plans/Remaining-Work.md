# Remaining work

Summary derived from the archived plans on 2026-10-14, extended 2026-10-16 with the parity and code quality review leftovers. Phases 0–12 and gates 1–4 of [Implementation-Phases](archive/Implementation-Phases.md) are built. This file lists what is not. Details stay in the linked archived sections; this file does not replace them.

## Deferred on purpose

- **Phase 13: LLM and A2A.** Ported as the Java framework stands when the phase begins, because the Java side is not yet stable and community feedback iterations are still running. Scope: LLM client, conversation, protocols, tool loop, host routes for LLM endpoints, A2A routes (design in `../moqui/framework/plans/A2A-Server.md`). `LlmServices.xml`, `A2AServices.xml` and the tools component's `llm` REST resource were removed on 2026-10-14; take fresh copies from moqui-framework when the phase begins and wire them then. The LLM and A2A entities and `LlmTypeData.xml` stay. The LLM client sets the sim-allow exception on `RestClient`. Check: same cases as the Java tests at the time of the port. See [Implementation-Phases, Phase 13](archive/Implementation-Phases.md).
- **Further database drivers** (SQL Server, Oracle, DB2, DB2 for i, Derby, HSQLDB). Build stays on Postgres, MySQL, and SQLite (browser). A missing driver is a configuration error naming the database. Phase 12 in the same file.
- **Assist panel** and the other agent/assist parts: part of this phase.
- **Deliberately not ported:** JSON-RPC, Groovy shell and web eval, Java/Groovy service runners, `vuet`/`/vapps`, `service-group`, Swagger 2.0/RAML output, Hazelcast/session-byte replication. Non-SQL entity datasources (`ElasticDatasourceFactory`, OrientDB) are a proposal in [Future-Plans](Future-Plans.md).

## Known open items

| Area | Item | Source |
|---|---|---|
| Wasm browser host | No login flow (calls run with authz disabled except renders); Groovy services skipped (`crates/moqui-browser/src/host.rs`) | [Wasm, Status](archive/Wasm.md) |
| Wasm | Second-tab OPFS failure untested; `/opfs` must not be touched via the Emscripten FS JS API; WebSocket check uses a Node mock of `/notws`, not moqui-host | same |
| Screens | Assist panel (`js/assist`) not ported; it belongs with the Phase 13 assist parts | [Implementation-Phases, Phase 8](archive/Implementation-Phases.md) |
| Mail | Embedded SMTP server: no code; stays disabled behind a feature if ever built | [Crate-Layout](archive/Crate-Layout.md) |
| Parity | Java vs MR-Moqui parity harness and its diff report (a gate artifact) has no code; only search-backend parity tests exist. Java runtime must stay runnable | [Implementation-Phases, Tests](archive/Implementation-Phases.md) |
| Search | Universal search result links go to the generic entity edit screens until IronStone has record screens; the per-document hook predicate is not built | [SearchFacade, REST service and UI](archive/SearchFacade.md) |
| Search | Semantic retrieval has the `Embedder` trait and a development hash embedder only; no local model (`semantic` cargo feature) | [Implementation-Phases, Phase 9](archive/Implementation-Phases.md) |
| Search | Postgres broad queries (common words: rank, facets, proximity) take tens to hundreds of ms at 50k documents; candidates are a stored per-field `tsvector`, a ranked-row cap, and measuring cover density on a real corpus. Hybrid `k` and weights wait on a relevance set | [Performance, Search](Performance.md) |
| IronStone | The `boss` menu has Search, QuickSearch and SalesSummary only; SalesAnalysis, SimpleReport and the User screens are not converted | ironstone-boss `mr-moqui` branch |
| MarbleERP | Acceptance work after the migration: wiki fixtures, exhaustive role and organization isolation, CSRF, every mounted QVT interaction, physical checks and scanners, FOP pagination parity | [MarbleERPMigration, Limits](archive/MarbleERPMigration.md#limits) |
| Docs | A `docs/Testing.md` for `moqui test` (spec format, script API, power asserts, runner), extracted from the archived design; today the design and `mr-mantle-usl/test/README.md` are the only description | [TestHarness](archive/TestHarness.md) |

## Parity gap review

The entity and service modules (Parity-Mapping's Entity and Service sections) were reviewed phase by phase against their Java sources on 2026-10-16: [EntityService-Parity-Review](archive/EntityService-Parity-Review.md), 8 phases, with the accepted differences in [Parity-Diffs](Parity-Diffs.md). The rest of [Parity-Mapping](Parity-Mapping.md) (moqui-lib, moqui-db, context and resources, user and session, transactions and drivers, script and templates, screens and macros, web and the HTTP host, search, notifications and cluster, mail and the tool components, tooling, browser host) has had no phase review yet; the threads to carry through one are [Parity-Mapping, Cross-cutting threads](Parity-Mapping.md#cross-cutting-threads).

Open findings from the entity and service review (none High; IDs link to the finding's section):

| ID | Item | Severity | Note |
|---|---|---|---|
| [P1-14](archive/EntityService-Parity-Review.md#p1-14--update-of-a-missing-row-is-silent) | An update that matches no row returns `Ok`; Java throws. Making the zero count an error needs a pass over the Rust callers that update rows that may be gone | Med | |
| [P1-15](archive/EntityService-Parity-Review.md#p1-15--caller-set-stamp-without-a-read) | A `lastUpdatedStamp` the caller sets on a found value is replaced unless the update reads the row; Java keeps it | Low | `EntityValue` does not track what it read |
| [P3-20](archive/EntityService-Parity-Review.md#p3-20--like-on-a-text-fts-field-is-not-a-full-text-search) | A `LIKE` on a `text-fts` field stays SQL `LIKE`; Java rewrites it to the full-text predicate. Reachable only through `EntityFind::matches`, which no component calls | Med | |
| [P3-21](archive/EntityService-Parity-Review.md#p3-21--foreign-key-existence-by-name) | A foreign key counts as present only by its computed name; Java matches the key columns, so a database whose keys Java named differently (MySQL `name_fk`) gets a second constraint | Med | Needs a catalog read of key columns per group |
| [P4a-18](archive/EntityService-Parity-Review.md#p4a-18--entitydataloaderlist-not-ported) | `EntityDataLoader.list()` is not ported | Low | Open for want of a caller: nothing in `runtime/` uses it |
| [P5-19](archive/EntityService-Parity-Review.md#p5-19--entity-schema-and-data-writer-csv-of-a-view-have-no-fields) | The entity JSON schema and the data writer's CSV of a view have no fields (the model keeps a view's fields as aliases); fix is to read `field_names` and the alias types as `EntitySnap` does | Med | Includes views the runtime overlay builds |
| [P8a-41](archive/EntityService-Parity-Review.md#p8a-41--portrait-and-monochrome-on-every-job) | Every Java print job asks for portrait and monochrome (cups4j builder defaults); Rust sends neither, so the queue's defaults apply | Med | Decision open: match Java, or accept the queue defaults as a difference, or add a per-job setting |

Noted by the review but outside it:

- `cargo test -p moqui-host --test gate3` has 3 failures not from that work: `artifact_group_detail_renders_as_text_csv_xml_and_html_print`, `artifact_group_detail_static_qvt_is_a_data_free_template_with_its_data_on_actions` and `the_login_page_and_the_root_render_as_html_and_a_failed_login_comes_back_with_its_message` (screen-render assertions: an ALL_USERS link, a `$mq.url` template, a login alert div).
- `user/preference.rs` `date_open` treats `thruDate` as exclusive; Java's `filterByDate` is inclusive (as fixed for jobs and security checks in that review).
- The Java-vs-Rust parity harness with a diff report (a gate artifact) still has no code; see the Parity row above.

## Code quality review leftovers

The [Code-Quality-Review](archive/Code-Quality-Review.md) of 2026-10-14 was resolved on 2026-10-15 (six fix phases, about 170 new tests). Left as noted in its [Resolution](archive/Code-Quality-Review.md#not-done-or-left-as-noted):

- **Lows:** `service/mail.rs` still uses `Result<_, String>` (49 sites); `moqui-lib` `regex_match` is polynomial on nested quantifiers; static files are read whole, with no Range support; search mapping caches can go stale across cluster nodes; the login lockout gate is checked before the failure increment; `remember_client_ip` updates the Visit on every request; HttpSession rows are never purged when `webapp_session_timeout` is 0 or less.
- **Process-wide global state:** the notify bus and its origin id, the transaction deadline slots and lock tracking, the QZ signer, and `LAST_BOOTED`.
- **Not verified live:** `catch_unwind` in the wasm32-emscripten build; the pgvector embedding SQL (its test needs `PGVECTOR_URL`); MySQL and MariaDB integration (needs a server).
- **Known limits:** on MySQL (not MariaDB) the statement timeout bounds only SELECT; the entity JSON loader relies on serde_json's `arbitrary_precision` number key (a test covers it); `cargo clippy -p moqui-host --no-default-features` failed at baseline and is not linted.
- **Test isolation:** `screen/cache.rs` and `script/runtime/facades.rs` still read `instance_purpose` directly.

## Checked and done (removed from the list)

- Postgres `NOTIFY` cluster channel: `cluster_transport=notify`, `moqui-host/src/cluster_notify.rs`, with an integration test.
- The russh SSH cluster sender: `moqui-host/src/ssh/cluster.rs`.
- `ScreenServices.xml` bodies: JavaScript or Rust under `ServiceKind::Tools` (Phase 11).
- The last framework Groovy (2026-10-14): `WikiServices`, `InstanceServices` (Docker provisioning and the database create and check services) and `verify#ReCaptcha` are JavaScript, on `ec.service.rest()`, the wiki's ResourceReference methods, dbresource version history, `ec.entity` SQL on a conf connection and `ec.resource.runTokenCommand`. The email services run in Rust (`service/mail.rs`) selected by name, and the `Security.eecas.xml` checks call the `UserSecurityChecks` script helper. `moqui lint --groovy runtime/base-component` reports no parse failures. Checked by `crates/moqui-core/src/service/script_ports_tests.rs`.
- Static `qvt` template double download: the handler loads the script with the response's ETag/Cache-Control serving the second request, so it is a cache hit, not a second download. Not verified in a browser.
- `lookup` widget as a text line: matches Java, where the macro was commented out. Not a gap.
- `WASM_BIGINT` link flag: no longer present in the repo.
- Search callers on Mantle: `SalesReportServices`, `get#InventoryProjectedInfo`, party and general search on `ec.search`, with the org limit as a trusted filter; universal search (`search#Universal`, `/rest/s1/mantle/search`, ironstone-boss `Base/Search`); the wiki manual data feed. Checked by `crates/moqui-host/tests/mantle_search.rs` with the Mantle components present.

## Mantle conversion follow-up

mantle-usl services are hand ported to JavaScript and `moqui lint --groovy` is at 0 flagged sites (the lint separates Groovy and run-time hazards from valid JavaScript idioms; see the sixth and seventh iterations in [ConvertProcess](archive/ConvertProcess.md)). The eight Spock specs and thirty-five specs without a Groovy original run under `moqui test mantle-usl` ([MantleFlowTests](archive/MantleFlowTests.md)): 344 features in 43 specs, 751 of 762 services called (`moqui test mantle-usl --coverage`). Open: the 11 services never called. Three call Google (`GooglePartyServices`) and one the reCAPTCHA service (`create#ContactRequestReCaptcha`), which need a network; four are `TenantSubscriptionServices`, left for the deprecated multi-tenant mode (the `fulfill#ProductSubscriptionResource` it builds on is covered); `TaxServices.calculate#InvoiceSalesTax` is an empty TODO upstream; `ReturnServices.returnAndResell#Shipment` never places its new order upstream (a spec documents where it stops); `PartyServices.send#DataDocumentNotifications` needs the data feed of a search backend. `generate#Efw2File` is unfinished upstream (no RT, RU, RV or RF records, and it never sets `fileText`); `autoApply#Payment`, `get#VariantProductsInfo`, `calculate#SalesTaxLocal` and `post#WorkEffortCost` are stubs. Open elsewhere: the 18 `moqui lint --screens` failures in ironstone-boss; Drools is removed rather than ported. Application migration of other components' scripts is outside this repository's gates.

## Related documents
[Future-Plans](Future-Plans.md) (digest of the unbuilt [proposals](proposals/)), [Performance-Summary](Performance-Summary.md) (the next phase), [Parity-Mapping](Parity-Mapping.md) and [Parity-Diffs](Parity-Diffs.md) (reference for the gap review). The index is [README](README.md).
