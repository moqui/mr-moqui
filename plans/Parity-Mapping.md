# Framework Parity Mapping

Date: 2026-10-16. This file maps the MR-Moqui Rust tree onto the Moqui Java, Groovy, XML, and
FreeMarker it was ported from, and accounts for every framework source the other way.

The entity and service sections are still the basis for the phase reviews in
[EntityService-Parity-Review](archive/EntityService-Parity-Review.md). Those tables keep their phase
column. The rest of the file is the same kind of map for the whole framework. It names primary
sources. It is not a line-level review and it does not record divergences: the accepted ones are in
[Parity-Diffs](Parity-Diffs.md), the open ones are findings in [Remaining-Work](Remaining-Work.md).

## How to read a row

| Status | Meaning |
|---|---|
| Port | The Rust file is the intended behavioral match for the named source. |
| Partial | Part of that source. The rest is named in another row or under Excluded. |
| Redesign | The behavior changed on purpose. The row cites the README redesign or the archive plan. |
| Native | No Java counterpart. |
| Excluded | Named and not ported. |

Paths are relative to `crates/` unless they start with `../moqui/`. A Java interface sits with its
implementation; the reverse index names the interface anyway. Tests (`tests.rs`, `*_tests.rs`,
`src/test`) are evidence only and are not line-reviewed.

Shared XML (entity definitions, service definitions, screens, data files, ECA rules) is loaded by
both runtimes. Those files are mapped at the loader, not one row per file. Service XML whose
body moved into Rust is named in the service section.

## Reference locations

- **Framework classes:** `../moqui/framework/src/main/{java,groovy}/org/moqui/**` and
  `../moqui/framework/src/start/java/MoquiStart.java`.
- **Framework resources that are code:** `MoquiDefaultConf.xml`, `log4j2.xml`, `shiro.ini`,
  `web.xml`, and `src/main/resources/org/moqui/impl/{pollEmailServer,sendEmailMessage,sendEmailTemplate}.groovy`.
- **Screen macros:** `../moqui/runtime/template/screen-macro/*.ftl` and
  `../moqui/framework/template/XmlActions.groovy.ftl`. The Rust copies are
  `runtime/base-component/framework/template/`.
- **Framework service, entity, screen, and conf XML:** `../moqui/framework/{service,entity,screen,data}`
  and the checked-in `runtime/base-component/framework/` copy.
- **Absorbed components** (class and service XML, not their screens): `../moqui/runtime/component/moqui-sso`,
  `moqui-sftp`, `moqui-image`, `moqui-poi`, `moqui-cups`, `moqui-fop`. Image and CUPS ship service
  XML and a Java library (ImageIO, CUPS4J) rather than Groovy classes. SSO, SFTP, POI, and FOP have
  Groovy under `src/main/groovy`.
- **Not in this map:** Mantle and other application components, tools and webroot screen XML (both
  runtimes execute those files), seed data, and XSD files except where a Rust module is the consumer.
  The client libraries the port drops or replaces (jQuery, Moment, CKEditor, vuet, `/vapps`) are one
  exclusion, not a file list.

## Excluded on purpose

A reviewer classifies these as intentional. Each Java type is also in the reverse index.

- **JSON-RPC.** `ServiceJsonRpcDispatcher`, `RemoteJsonRpcServiceRunner`. `send#SystemMessageJsonRpc` is not ported.
- **Groovy and Java service runners.** `JavaServiceRunner`, `GroovyScriptRunner`, `JavaxScriptRunner`. A `type="script"` location that still names `.groovy` is unsupported. `GroovyShellEndpoint` is not a web endpoint; the SSH admin shell is the later replacement (`moqui-host/src/ssh/shell.rs`, [Ssh](archive/Ssh.md)).
- **`service-group`.** Not ported.
- **Swagger 2.0 and RAML.** `RestApi` output of those forms. `screen/schema.rs` writes JSON Schema only.
- **Elasticsearch as an entity datasource.** `impl/entity/elastic/**` (`ElasticDatasourceFactory`, `ElasticEntityFind`, `ElasticEntityListIterator`, `ElasticEntityValue`, `ElasticSynchronization`). Search is the SearchFacade redesign below. The `moqui-elasticsearch` add-on is not linked.
- **Hazelcast and session-byte replication.** `moqui-hazelcast` is not linked. `MoquiSessionListener` does not copy session bytes.
- **LLM and A2A.** `impl/llm/**`, `org.moqui.llm/**`, `LlmFacade`, `A2AFacade`, `LlmServlet`, `A2AServlet`, `A2ACardServlet`, and `LlmServices.xml`, `LlmAdminServices.xml`, `A2AServices.xml`. Deferred; see [Remaining-Work](Remaining-Work.md).
- **Databases not in this tree.** H2 (`H2ServerToolFactory`), SQL Server, Oracle, DB2, Derby, HSQLDB. The port's drivers are Postgres, MySQL, and the browser SQLite driver.
- **Shiro.** `shiro.ini`, `MoquiShiroRealm`, `MoquiWebSecurityManager`. Login still verifies a stored Shiro `SimpleHash` and rehashes to argon2 (`user/password.rs`).
- **Jackrabbit, JCS, SubEtha.** `JackrabbitRunToolFactory`, `JCSCacheToolFactory`, `cache.ccf`, `SubEthaSmtpToolFactory`. The embedded SMTP server stays disabled. Inbound mail is `poll#EmailServer`.
- **FreeMarker, GString, and FOP as engines.** `FtlTemplateRenderer`, `GStringTemplateRenderer`, `FtlMarkdownTemplateRenderer`, `MoquiFopServlet`, and `moqui-fop` (`FopToolFactory`, `HtmlRenderer`, `HtmlRenderServlet`). Markdown files render through `moqui-lib/src/markdown.rs`. Print is `html-print` (README redesign 5).
- **vuet, vue, js render modes and `/vapps`.** `DefaultScreenMacros.vuet.ftl` and the vuet/js screen companions.
- **Other dropped pieces.** `SimpleSgmlReader` (no SGML reader). `content://` (`ContentResourceReference`) is a named error. OAuth 1.0 and SAML (the SSO section). `MClassLoader` and `LiteStringMap` (JVM class loading and a Java map optimization). `bitronix-default-config.properties` (Bitronix is a behavior reference only). `MoquiLog4jAppender` and `log4j2.xml` as a runtime (logging is `tracing` in the host).

## Entity and service files with no parity basis

These Rust files have no single Java counterpart. The phase review skips them as parity targets.
Other Native modules are marked in their own sections.

| Rust file | Why no basis |
|---|---|
| `moqui-core/src/entity/share.rs` | `Send`/`Sync` wrapper around the model. |
| `moqui-core/src/entity/params.rs` | Driver value binding. `moqui-db` binds the SQL values. |
| `moqui-core/src/entity/exec.rs` | Statement execution, timing, and the span. |
| `moqui-core/src/entity/mod.rs`, `service/mod.rs` | Module spines and re-exports. |
| `moqui-core/src/service/util.rs` | Shared helpers. Java scatters the same bits. |
| `moqui-core/src/service/coverage.rs` | `moqui test --coverage`. No Java equivalent. |
| `moqui-core/src/service/rust_services.rs` | `type="rust"` function registry. New with the port. |
| `service/inline_tests.rs`, `service/script_ports_tests.rs`, every `tests.rs` | Tests. Evidence only. |

## moqui-lib

Standalone utilities. No `ExecutionContext`. Java's `org.moqui.util` and `org.moqui.jcache` land here, plus ETL and the markdown renderer.

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-lib/src/cache.rs` | `jcache/MCache.java`, `MCacheConfiguration`, `MEntry`, `MStats`. No JSR-107 manager and no JCS. | Partial |
| `moqui-lib/src/node/mod.rs`, `node/parse.rs` | `util/MNode.java`. Hand-written reader, no XML crate. | Port |
| `moqui-lib/src/object.rs` | `util/ObjectUtilities.java`. Decimals are `rust_decimal`, timestamps are `jiff`. | Port |
| `moqui-lib/src/collection.rs` | `util/CollectionUtilities.java`, the helpers the framework calls. | Partial |
| `moqui-lib/src/string.rs` | `util/StringUtilities.java`. Groovy class-name cleanup, DOM text, Elastic query helpers, and `numberToWords` are not ported. | Partial |
| `moqui-lib/src/context_stack.rs` | `util/ContextStack.java`. | Port |
| `moqui-lib/src/etl.rs` | `etl/SimpleEtl.java`, `etl/FlatXmlExtractor.java`. | Port |
| `moqui-lib/src/http.rs` | `util/RestClient.java`. TLS certificates are verified; Java trusts any certificate. | Partial |
| `moqui-lib/src/json.rs` | Groovy `JsonSlurper` number kinds (integers stay integers, other numbers stay decimals) and the Jackson map in `ContextJavaUtil`. | Partial |
| `moqui-lib/src/format.rs`, `datefmt.rs` | The `DecimalFormat` and date-pattern subset `L10nFacadeImpl` uses. `HALF_EVEN`. | Port |
| `moqui-lib/src/signer.rs` | `impl/util/SimpleSigner.java`. QZ Tray RSA SHA1withRSA. | Port |
| `moqui-lib/src/env.rs` | `util/SystemBinding.java` property and `${name}` expansion. `ResourceFacade.expand` stays in core. | Partial |
| `moqui-lib/src/resource.rs` | File and in-memory half of `resource/ResourceReference.java`. No scheme router. | Partial |
| `moqui-lib/src/regex_match.rs` | Java `String.matches` (whole input, `.` does not cross a line). | Port |
| `moqui-lib/src/markdown.rs` | `MarkdownTemplateRenderer` (flexmark). CommonMark plus GitHub tables. The flexmark `[TOC]` extension is not rendered. | Partial |
| `moqui-lib/src/codec.rs` | Hex, constant-time compare, FNV-1a. | Native |
| `moqui-lib/src/sync.rs` | Lock guards that ignore poisoning. | Native |
| `moqui-lib/src/error.rs` | Plain error for this crate. Artifact exceptions stay in core. | Native |
| `moqui-lib/src/lib.rs` | Crate root. | Native |

`MCacheToolFactory` is the tool entry. The cache it returned is this `cache.rs`, installed from `moqui-core/src/context/cache.rs`.

## moqui-db

Entity model and SQL text. No `ExecutionContext` and no driver. The entity section calls into these files; they are in scope here.

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-db/src/dialect.rs` | The `<database>` profile `EntityFacadeImpl` reads (type maps, quoting, identity style). | Port |
| `moqui-db/src/model/mod.rs` | `EntityDefinition.groovy`, `FieldInfo.java`. The model keeps no `MNode`. | Port |
| `moqui-db/src/model/view.rs` | View-entity member and `alias-all` expansion inside `EntityDefinition`. | Port |
| `moqui-db/src/model/condition.rs` | `entity-condition` on a view-entity or a member. | Port |
| `moqui-db/src/sql/condition.rs` | `EntityFindBuilder` condition SQL and `impl/entity/condition/` (`BasicJoinCondition.java`, `ConditionAlias.java`, `ConditionField.java`, `DateCondition.groovy`, `EntityConditionImplBase.java`, `FieldToFieldCondition.groovy`, `FieldValueCondition.java`, `ListCondition.java`, `TrueCondition.java`, `WhereCondition.groovy`). | Port |
| `moqui-db/src/sql/statement.rs` | `EntityQueryBuilder` insert, update, delete, select, count, and DDL text. | Port |
| `moqui-db/src/sql/view.rs` | View-entity select and count after `EntityFindBuilder`. | Port |
| `moqui-db/src/sql/ident.rs` | Identifier quoting and `name-replace` (`EntityJavaUtil` naming). | Port |
| `moqui-db/src/sql/mod.rs`, `model` re-exports, `lib.rs` | Module spines. | Native |

`EntityConditionFactory` and `EntityConditionFactoryImpl` are this condition builder plus `entity/find.rs`, which assembles the tree. There is no separate factory type.

## Context, factory, and resources

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-core/src/context/factory.rs` | `ExecutionContextFactoryImpl`, `ExecutionContextImpl`, `ExecutionContext`, `ExecutionContextFactory`, `Moqui.java` (the entered factory; no servlet static). | Port |
| `moqui-core/src/context/conf.rs` | `ExecutionContextFactoryImpl.mergeConfigNodes` over `MoquiDefaultConf.xml`. | Port |
| `moqui-core/src/context/component.rs` | Component directories and one-pass `depends-on`, from the factory's component load. | Port |
| `moqui-core/src/context/locate.rs` | `component://`, `file:`, and `classpath://` as an alias for framework `service/`, `entity/`, `data/`, `screen/`, `template/`. `ClasspathResourceReference`. | Partial |
| `moqui-core/src/context/resource.rs` | `ResourceFacadeImpl` and `reference/{Base,Component,Wrapper,Db}ResourceReference`. Schemes: `file`, `runtime`, `component`, `dbresource`, `http`. | Port |
| `moqui-core/src/entity/dbresource.rs` | `DbResourceReference` record operations (versions, the `DbResource` entities). The scheme router is `context/resource.rs`. | Port |
| `moqui-core/src/context/cache.rs` | `CacheFacadeImpl` on one `MCache` per `<cache name>`. | Port |
| `moqui-core/src/context/artifact.rs` | `ArtifactExecutionFacadeImpl` stack and `ArtifactExecutionInfoImpl`. Push and pop are tracing spans. | Partial |
| `moqui-core/src/context/hits.rs` | `countArtifactHit`. The scheduler tick writes `ArtifactHit` and `ArtifactHitBin`. | Port |
| `moqui-core/src/context/message.rs` | `MessageFacadeImpl`, `MessageFacade`, `MessageFacadeException`. | Port |
| `moqui-core/src/l10n.rs` | `L10nFacadeImpl`, `L10nFacade`. Formatters are `moqui-lib`. | Port |
| `moqui-core/src/context/status.rs` | `ExecutionContextFactoryImpl.getStatusMap` and `getComponentInfoList`. JVM management beans are process stats (`/proc/self/status`). Disk is not reported. | Partial |
| `moqui-core/src/context/error.rs` | `BaseException`, `BaseArtifactException`, and the artifact, auth, and web exceptions (`ArtifactAuthorizationException`, `ArtifactTarpitException`, `AuthenticationRequiredException`, `TransactionException`, `EntityException`, `ServiceException`, and the not-found types). | Port |
| `moqui-core/src/context/confcache.rs` | User and authz conf parsed once per factory. | Native |
| `moqui-core/src/context/mod.rs` | Module spine. | Native |

`user/authz.rs` is the other half of `ArtifactExecutionFacadeImpl`: `isPermitted`. `script/runtime.rs` replaces `ContextBinding` (bare names through a proxy on `globalThis`). `UrlResourceReference` is `context/resource.rs` for `http` plus `moqui-lib/src/http.rs`.

## User and session

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-core/src/user/login.rs` | `UserFacadeImpl` login against `UserAccount`. A bound web session rotates on password success. | Port |
| `moqui-core/src/user/password.rs` | `UserFacadeImpl` password check. New hashes are argon2id. A stored Shiro `SimpleHash` verifies, then rehashes. | Redesign |
| `moqui-core/src/user/policy.rs` | `user-facade/password` policy (length, digits, history, change interval, max failures, disable minutes). | Port |
| `moqui-core/src/user/factor.rs` | Second factor in `UserFacadeImpl`: TOTP (HMAC-SHA1, 6 digits, 30 seconds) and single-use codes. `SecondFactorRequiredException`. | Port |
| `moqui-core/src/user/preference.rs` | `UserFacadeImpl` preferences, group membership, and permissions. | Port |
| `moqui-core/src/user/period.rs` | `UserFacadeImpl.getPeriodRange`. | Port |
| `moqui-core/src/user/visit.rs` | `UserFacadeImpl` visit and visitor rows. | Port |
| `moqui-core/src/user/filter.rs` | `ArtifactExecutionFacadeImpl.filterFindForUser`, `ArtifactAuthzFilter`, `EntityFilter`. | Port |
| `moqui-core/src/user/authz.rs` | `ArtifactExecutionFacadeImpl.isPermitted`. | Port |
| `moqui-core/src/user/session.rs` | Session state. The durable row is `moqui.server.HttpSession`, not a Shiro session and not `MoquiSessionListener` bytes. README redesign 7 and [Cluster](archive/Cluster.md). | Redesign |
| `moqui-core/src/user/mod.rs` | Module spine. `UserFacade` is this module. | Native |

`PasswordChangeRequiredException` is raised from `user/policy.rs` through `context/error.rs`.

## Transactions and drivers

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-core/src/transaction/manager.rs` | `TransactionFacadeImpl`. `TransactionInternalBitronix` is the behavior reference (local one-phase commit, suspend, synchronizations). It is not linked. No JTA and no XA. | Port |
| `moqui-core/src/transaction/pool.rs` | The pool Bitronix provided: min, max, idle, acquire timeout, reserve tier. One implementation. | Port |
| `moqui-core/src/transaction/mod.rs` | Module spine. `TransactionFacade` and `TransactionInternal` are this module. | Native |
| `moqui-core/src/driver.rs` | `EntityDatasourceFactory` as a trait. No Postgres or MySQL types in core. | Port |
| `moqui-core/src/entity/datasource.rs` | `EntityDatasourceFactoryImpl`. Registers the host driver. | Port |
| `moqui-core/src/entity/cache.rs` | `EntityCache`, `EntityTxCache`, `TransactionCache`, and the in-memory half of `TransactionCacheDb` (per-frame cache, sim sequence banks, production-entity list). | Port |
| `moqui-host/src/db/postgres.rs` | The PostgreSQL JDBC driver, as the sync `postgres` client. | Port |
| `moqui-host/src/db/mysql.rs` | MySQL Connector/J, as the sync `mysql` crate. | Port |
| `moqui-host/src/db/embedded.rs` | H2 embedded lifecycle, replaced by `postgresql_embedded`. [EmbeddedPostgres](archive/EmbeddedPostgres.md). | Redesign |
| `moqui-host/src/db/sim.rs` | `TransactionCacheDb` overlay cluster: one schema per sim on a second postmaster. | Port |
| `moqui-host/src/db/jdbc_extract.rs` | `impl/util/JdbcExtractor.java`. | Port |
| `moqui-host/src/db/entity_db.rs` | Which conf datasource is embedded Postgres, external Postgres, or MySQL. | Native |
| `moqui-host/src/db/entity_facade.rs` | Installs the host driver on the core facade. | Native |
| `moqui-host/src/db/statement_lru.rs` | Prepared-statement cache, cap 100. Java cached these inside the JDBC driver. | Native |
| `moqui-host/src/db/mod.rs` | Module spine. | Native |
| `moqui-core/src/testing.rs` | In-memory SQL for unit tests. Not H2. | Native |

`impl/context/SimFence.java` and `SimTerminatedException.java` are the sim fence on `entity/cache.rs` and the outbound-HTTP denial in `moqui-lib/src/http.rs`.

## Entity

Phase numbers match [EntityService-Parity-Review](archive/EntityService-Parity-Review.md). SQL text and the definition model are in moqui-db above. `params.rs`, `exec.rs`, and `share.rs` are the native rows in the table before this section.

| Rust file | Primary basis | Phase |
|---|---|---|
| `entity/value.rs` | `EntityValueBase.java`, `EntityValueImpl.java`, `EntityValue.java`, `EntityListImpl.java`, `EntityList.java` | 1 |
| `entity/field_rw.rs` | `EntityValueBase.java`, `FieldInfo.java` (encrypt/decrypt, audit, l10n overlay) | 1 |
| `entity/crypto.rs` | `EntityJavaUtil.java` (`enDeCrypt`), `FieldInfo.java` | 1 |
| `entity/fk.rs` | `EntityValueBase.java` (FK check), `EntityDefinition` FK metadata | 1 |
| `entity/find.rs` | `EntityFindBase.groovy`, `EntityFindImpl.java`, `EntityFind.java`, `EntityQueryBuilder.java`, `EntityFindBuilder.java`, `EntityListIteratorImpl`, `EntityListIteratorWrapper` | 2 |
| `entity/search_form.rs` | `EntityFindBase.groovy` (`searchFormMap`) | 2 |
| `entity/schema.rs` | `RestSchemaUtil.groovy` per-entity JSON Schema. Screen-served `entity.json` and `master.json` are `screen/schema.rs`. | 2 |
| `entity/facade.rs` | `EntityFacadeImpl.groovy`, `EntityFacade.java` | 3 |
| `entity/cache.rs` | `EntityCache.groovy`, `EntityTxCache.groovy`, `TransactionCache.groovy` (facade hooks in `EntityFacadeImpl`) | 3 |
| `entity/sequence.rs` | `EntityFacadeImpl` sequenced-id / `SequenceValueItem`. A sim uses `TransactionCacheDb` banks. | 3 |
| `entity/meta.rs` | `EntityDbMeta.groovy`, `FieldInfo.java`, `FtsDdl.java`, `FtsSql.java` | 3 |
| `entity/info.rs` | `EntityFacadeImpl`, `EntityDefinition.groovy` (tools metadata getters) | 3 |
| `entity/datasource.rs` | `EntityDatasourceFactoryImpl.groovy`, `EntityDatasourceFactory.java` | 3 |
| `entity/dbresource.rs` | `context/reference/DbResourceReference.groovy` | 3 |
| `entity/load_feed/loader.rs`, `entity/load_feed/csv.rs`, `entity/load_feed/json.rs`, `entity/load_feed/xml.rs`, `entity/load_feed/zip.rs`, `entity/load_feed/pull.rs`, `entity/load_feed/check.rs` | `EntityDataLoaderImpl.groovy`, `EntityDataLoader.java`, `OverlayColumnNames.java` | 4 |
| `entity/load_feed/feed.rs` | `EntityDataFeed.groovy` registration. The after-commit index write is `search/feed.rs`. | 4 |
| `entity/load_feed/eca.rs`, `entity/load_feed/mod.rs` | `EntityEcaRule.groovy` | 4 |
| `entity/data_writer.rs` | `EntityDataWriterImpl.groovy`, `EntityDataWriter.java` | 4 |
| `entity/document.rs` | `EntityDataDocument.groovy` | 4 |
| `entity/etl.rs` | `EntityFacadeImpl.makeEtlLoader`, `EntityFind.extract`. `SimpleEtl` itself is `moqui-lib/src/etl.rs`. | 4 |
| `entity/query_rest.rs` | `EntityFacadeImpl.rest`, `EntityDynamicViewImpl`, `EntityDynamicView`, `impl/EntityServices.xml`, `RestSchemaUtil` | 5 |
| `entity/sensitive.rs` | `WebUtilities` identity-admin entities, secret-config entities, and `UserAccount` secret fields. Also used by the data loader. | 5 |

## Service

The same phase numbers. Component Groovy that this table used to call missing is under `../moqui/runtime/component/`.

| Rust file | Primary basis | Phase |
|---|---|---|
| `service/define.rs` | `ServiceDefinition.java`, `ServiceFacadeImpl.groovy` (definition load), `runner/ServiceRunner.groovy`. `type="script"` plus a `.groovy` location is unsupported. | 6 |
| `service/validate.rs` | `ParameterInfo.java`, `ServiceDefinition.java` validation. `ValidationError`. | 6 |
| `service/call.rs` | `ServiceCallImpl`, `ServiceCallSyncImpl`, `ServiceCall`, `ServiceCallSync`, `ServiceFacadeImpl`, `ServiceCallSpecialImpl` (`registerOnCommit` / `registerOnRollback`), `ServiceCallback` | 6 |
| `service/semaphore.rs` | `ServiceCallSyncImpl` / `ServiceDefinition` semaphore | 6 |
| `service/info.rs` | `ServiceFacadeImpl` (`getKnownServiceNames`, `getServiceDefinition`, SECA getters) | 6 |
| `service/eca.rs` | `EntityEcaRule.groovy`, `ServiceFacadeImpl` entity-ECA, `*.eecas.xml` | 7 |
| `service/eca_rules.rs` | `ServiceEcaRule.groovy`, `*.secas.xml`, `*.emecas.xml` load | 7 |
| `service/seca.rs` | `ServiceEcaRule.groovy` | 7 |
| `service/rule_actions.rs` | `ServiceEcaRule.groovy` action running | 7 |
| `service/email_eca.rs` | `EmailEcaRule.groovy`, `*.emecas.xml` | 7 |
| `service/security_checks.rs` | `Security.eecas.xml` and `UserSecurityChecks.groovy` (`assertPrivilegedGroupMembership`, `assertSealedPermissionGrant`). The script helper is `script/runtime/ec_extra.rs`. | 7 |
| `service/pool.rs` | `ServiceFacadeImpl` worker threads, `ServiceCallAsyncImpl` | 7 |
| `service/request_pool.rs` | `ExecutionContextFactoryImpl` request workers. The HTTP listener does not use the servlet thread. | 7 |
| `service/jobs.rs` | `ScheduledJobRunner.groovy`, `ServiceCallJobImpl`, `ServiceCallJob` | 7 |
| `service/schedule.rs` | `ScheduledJobRunner` cron parsing and the scheduler thread | 7 |
| `service/rest.rs` | `RestApi.groovy` request path, `impl/ServiceServices.xml`. JSON-RPC, Swagger, and RAML excluded. | 8a |
| `service/remote.rs` | `RemoteRestServiceRunner.groovy`. `RemoteJsonRpcServiceRunner` excluded. | 8a |
| `service/tools.rs` | `PrintServices.xml` (moqui-cups), `impl/EntityServices.xml` encrypted-field rewrite, data-document notification | 8a |
| `service/mail/implementation.rs`, `service/mail/mod.rs` | `EmailServices.xml` (both), `pollEmailServer.groovy`, `sendEmailMessage.groovy`, `sendEmailTemplate.groovy` | 8b |
| `service/sso.rs` | `moqui-sso` `AuthServices.xml`, `AuthenticationFlow.groovy`, `MoquiSecurityGrantedAccessAdapter.groovy` | 8c |
| `service/elfinder.rs` | `ElFinderServices.xml`, `ElFinderConnector.groovy` | 8c |
| `service/system_sync/mod.rs`, `service/system_sync/entity_sync.rs` | `EntitySyncServices.xml` | 8c |
| `service/system_sync/message.rs` | `SystemMessageServices.xml`, `WebFacadeImpl.handleSystemMessage` | 8c |
| `service/system_sync/sftp.rs` | `moqui-sftp` `SftpMessageServices.xml` | 8c |
| `service/images.rs` | `moqui-image` `ImageServices.xml` (both). No Groovy class; Java used ImageIO. | 8c |
| `service/spreadsheet.rs` | `moqui-poi` `SpreadsheetServices.xml` (`read#Spreadsheet`, `load#SpreadsheetEntity`). Screen xlsx output is `screen/xlsx.rs`. | 8c |

`runner/EntityAutoServiceRunner` is `service/call.rs` together with `entity/`. `runner/InlineServiceRunner` and `runner/ScriptServiceRunner` are `service/call.rs` running XML actions through `script/`. `JavaServiceRunner` is excluded. `service/rust_services.rs` is the registry those `type="rust"` locations name.

Framework service XML with no Rust body (`BasicServices`, `UserServices`, `ServerServices`, `ScreenServices`, `WikiServices`, `InstanceServices`, `GoogleServices`, `SmsServices`, `SearchServices`, `ElasticSearchServices`, and the entity and service XML that is still actions or `entity-auto`) is loaded by `service/define.rs` and run as XML actions or entity-auto. Groovy script locations are unsupported. `SmsServices` send stays a stub. LLM and A2A service XML is excluded.

## Script, XML actions, and templates

| Rust file | Primary basis | Status |
|---|---|---|
| `script/actions.rs`, `script/walker.rs`, `script/parse.rs` | `XmlActionsScriptRunner`, `actions/XmlAction.java`, `framework/template/XmlActions.groovy.ftl`. The embedded module is `runtime/base-component/framework/template/XmlActions.js`. | Port |
| `script/runtime.rs` | QuickJS evaluation. Replaces the Groovy binding (`ContextBinding`): bare names resolve through a proxy, `ec` is a host object. `plans/archive/ContextObjects.md`. | Redesign |
| `script/runtime/entities.rs` | Entity value, find, and list methods scripts call on `ec.entity`, including `EntityListIterator`. | Port |
| `script/runtime/services.rs` | `ec.service` call shape (`ServiceFacade` from script). | Port |
| `script/runtime/facades.rs` | Smaller `ec` members: cache, message, logger, resource, user, web. | Port |
| `script/runtime/ec_extra.rs` | `ec.resource.expand`, `ec.l10n`, `ec.context`, `ec.screen`, and the bare helpers `WebUtilities`, `StringUtilities`, `CollectionUtilities`, `UserSecurityChecks`. | Partial |
| `script/runtime/ec_screen.rs` | Screen-facing `ec` members used while a screen renders. | Partial |
| `script/runtime/decimal.rs` | `BigDecimal` call shape (`add`, `subtract`, `multiply`, `divide`, `eq`, `compare`, `isZero`). | Port |
| `script/runtime/convert.rs` | `ObjectUtilities` / `basicConvert` coercions at the script boundary. | Port |
| `script/runtime/values.rs` | Host `Value` to QuickJS and back. | Native |
| `script/runtime/helpers.rs` | Bare `compare`, `basicConvert`, list order and filter. | Partial |
| `script/runtime/conditions.rs` | XML action condition operators, including `matches`. | Port |
| `script/runtime/compiled.rs` | Cached compiled functions. | Native |
| `script/runtime/edi.rs` | Script entry to `EdiHandler` (`ec` EDI methods). The handler is `edi.rs`. | Port |
| `script/runtime/notification.rs` | `NotificationMessage` from script. | Port |
| `script/runtime/screen_host.rs` | `sri` methods the generated screen code calls. The Rust side is `screen/sri/`. | Port |
| `script/runtime/search.rs` | `ec.search` script surface. The facade is `search/`. | Redesign |
| `script/runtime/sftp.rs` | Script surface of `SftpClient`. | Port |
| `script/runtime/upload.rs` | Upload handle a script reads from a multipart field. | Port |
| `script/runtime/urls.rs` | `sri.buildUrl` / `makeUrlByType` from script. The resolver is `screen/urls.rs`. | Port |
| `script/runtime/user_host.rs` | `ec.user` script methods beyond the ones in `facades.rs`. | Partial |
| `script/runtime/tools_host.rs` | Tools-screen facade calls: entity and service model, screen stats, server status, SQL runner, data writer and loader builders, `ServiceCallSpecial`. | Partial |
| `script/runtime/native_tools.rs` | `ec.factory.getTool`. `ToolFactory` for a linked component. | Native |
| `script/runtime/error_info.rs` | Error facts a script reads. The web side is `web/error_info.rs`. | Partial |
| `script/template.rs` | Template renderers by extension. The fancy dialect (`if`, `list`, `assign`) replaces FreeMarker `?interpret`. `.md` goes to `moqui-lib`. `NoTemplateRenderer` is the pass-through class name a conf entry may still name. README redesign 4. | Redesign |
| `script/bounded.rs` | Cap on the compiled-script caches. | Native |
| `script/groovy_convert.rs` | `moqui convert --groovy`. Migration tool, not a runtime. [ConvertProcess](archive/ConvertProcess.md). | Native |
| `script/lint_hazards.rs`, `script/screen_lint.rs` | `moqui lint --groovy` and `moqui lint --screens`. | Native |
| `script/mod.rs` | Module spine. `ScriptRunner` for JavaScript and XML actions. | Native |

`TemplateRenderer` is this registry. `FtlTemplateRenderer`, `GStringTemplateRenderer`, and `FtlMarkdownTemplateRenderer` are excluded as engines.

## Screens and macros

| Rust file | Primary basis | Status |
|---|---|---|
| `screen/definition.rs` | `ScreenDefinition`, `ScreenSection`, `ScreenWidgets`, `xml-screen-3.xsd`. Widget subtrees stay XML text until the walker reads them. | Port |
| `screen/form.rs` | `ScreenForm.initForm` (`extends`, `auto-fields-entity`, `auto-fields-service`, `DbForm`). | Port |
| `screen/render.rs` | `ScreenRenderImpl.internalRender`, `ScreenRender`, `WebFacadeStub` via `make_render`. | Port |
| `screen/url.rs` | `ScreenUrlInfo.initUrl`. | Port |
| `screen/urls.rs` | `sri.buildUrl` and `sri.makeUrlByType`, including transition alias expansion. | Port |
| `screen/tree.rs` | `ScreenTree.sendSubNodeJson`. | Port |
| `screen/screen_test.rs` | `ScreenTestImpl`, `ScreenTest`. | Port |
| `screen/saved_finds.rs` | `ScreenForm` saved-find statics and the `ScreenRenderImpl` / `ScreenDefinition` uses (`formSaveFind`, `formSelectColumns`, `screenDoc`). | Port |
| `screen/menu.rs` | `sri.getMenuData` on the root `menuData` transition. | Port |
| `screen/sri/mod.rs` | `sri` object `ScreenRenderImpl` exposes to macros. `WebUtilities.encodeHtml`. | Port |
| `screen/sri/fields.rs` | Field values, `parameter` and `set`, validation hints. | Port |
| `screen/sri/forms.rs` | Dynamic forms, user columns, saved finds, form-list rows. | Port |
| `screen/sri/options.rs` | Drop-down, radio, and check options. | Port |
| `screen/sri/sections.rs` | Section, section-iterate, subscreens, include. | Port |
| `screen/sri/layout.rs` | Text-mode form-list column widths. | Port |
| `screen/sri/menu_items.rs` | Menu items and whether the user may view a screen. | Port |
| `screen/sri/theme.rs` | Theme id, resource values, icon classes. | Port |
| `screen/aggregate.rs` | `AggregationUtil.aggregateList`. | Port |
| `screen/cache.rs` | `ScreenFacadeImpl` conf, render modes, definition cache, URL cache. | Port |
| `screen/info.rs` | `ScreenFacadeImpl.getAllRootScreenLocations`, `getScreenInfoList`. | Port |
| `screen/static_files.rs` | The file branch of `ScreenUrlInfo.initUrl` (css, js, libs, images, favicon). | Port |
| `screen/generate.rs` | `ScreenWidgetRender` and `ScreenWidgetRenderFtl`. Generates one function per section from the JS macro module. | Port |
| `screen/statics.rs`, `screen/static_ast.rs`, `screen/statics/report.rs` | Server-static qvt. Java `server-static` was only a `Cache-Control` hint. [QvtServerStatic](archive/QvtServerStatic.md). | Redesign |
| `screen/vue_check.rs` | Vue 3 compiler check of a qvt template, in QuickJS. Gate 3. | Native |
| `screen/pdf.rs`, `moqui-host/src/pdf.rs`, `moqui-host/src/pdf/cdp.rs` | PDF bytes for `html-print`. `pdf.rs` in the host keeps one Chromium process. `pdf/cdp.rs` is the DevTools socket (`chromiumoxide` was not used). `xsl-fo` as a stored mode name selects this. README redesign 5. | Redesign |
| `screen/xlsx.rs` | moqui-poi `FormListExcelRender`, `ScreenWidgetRenderExcel`. `ScreenXlsx.js` fills the book through `sri.xlsx*`. | Port |
| `screen/dispatch.rs` | Rust bodies for the `rest.xml` transitions (`login`, and the other transitions that file declares in Rust). | Native |
| `screen/schema.rs` | `RestSchemaUtil` documents served from screens: `entity.json`, `master.json`. `service.swagger` is not the Swagger 2 document. YAML and RAML stay unbuilt. | Partial |
| `screen/mod.rs` | `ScreenFacade`. Also the screen-path half of `MoquiAuthFilter` (anonymous 401, Basic challenge). | Partial |

### Macro libraries

`generate.rs` embeds these. A `macro-template-location` that names a real file replaces the built-in for that mode. `ScreenSupport.js` is never replaced.

| Java FreeMarker | Rust module | Status |
|---|---|---|
| `DefaultScreenMacros.html.ftl`, `ScreenHtmlMacros.ftl` | `template/screen/ScreenHtml.js` | Port |
| `DefaultScreenMacros.csv.ftl` | `template/screen/ScreenCsv.js` | Port |
| `DefaultScreenMacros.text.ftl` | `template/screen/ScreenText.js` | Port |
| `DefaultScreenMacros.xml.ftl` | `template/screen/ScreenXml.js` | Port |
| `DefaultScreenMacros.plain.ftl` | `template/screen/ScreenPlain.js` | Port |
| `DefaultScreenMacros.qvt.ftl` | `template/screen/ScreenQvt.js` | Port |
| `DefaultScreenMacros.xsl-fo.ftl` | `template/screen/ScreenHtmlPrint.js` | Redesign |
| moqui-poi excel widgets (no FTL) | `template/screen/ScreenXlsx.js` and `screen/xlsx.rs` | Port |
| (none) | `template/screen/ScreenQvtStatic.js` | Redesign |
| `framework/template/XmlActions.groovy.ftl` | `template/XmlActions.js` | Port |
| (none) | `template/screen/ScreenSupport.js`, `template/TemplateSupport.js` | Native |
| `DefaultScreenMacros.vuet.ftl` | — | Excluded |
| `DefaultScreenMacros.any.ftl` | — | Excluded |
| `TestScreenHtmlMacros.ftl` | — | Excluded |

`html-print` emits HTML with print CSS. A direct `.xsl-fo` request still returns FO XML. A stored caller that passes render mode `xsl-fo` to mean PDF renders `html-print` and hands it to `PdfRenderer`.

## Web and the HTTP host

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-core/src/web/mod.rs` | `WebFacadeImpl` as an owned request and response: parameters, body, headers, session cookie, redirect checks (`WebUtilities.isSameOriginRedirect`), IP match, CORS origin, credential stripping. No servlet types. | Port |
| `moqui-core/src/web/multipart.rs` | Multipart bodies and `WebUtilities.isExecutable` on an upload. | Port |
| `moqui-core/src/web/error_info.rs` | The `errorThrowable` facts `error/*.xml` reads. | Partial |
| `moqui-core/src/outbound.rs` | Outbound HTTP for `remote-rest` and `http` resources. The server uses `moqui-lib` `RestClient`. The browser installs `fetch`. | Partial |
| `moqui-host/src/http.rs` | `MoquiServlet` and the public HTTP and HTTPS listeners. Screen work runs on the request pool. | Port |
| `moqui-host/src/logging.rs` | `LoggerFacadeImpl`, `ElasticSearchLogger`, `ElasticRequestLogFilter` (ship `moqui_logs` and `moqui_http_log`). File names match Java. Console color is new. | Partial |
| `moqui-host/src/notws.rs` | `NotificationEndpoint` at `GET /notws`. The socket does not hold an execution context. | Port |
| `moqui-host/src/search_proxy.rs` | Jetty `ElasticSearchProxy` and `KibanaProxy` servlets declared in `MoquiDefaultConf.xml`, behind the `MoquiAuthFilter` rules. OpenSearch only. | Port |
| `moqui-host/src/tls.rs` | Public HTTPS certificates from `runtime/cert`. Java terminated TLS at a proxy. | Native |
| `moqui-host/src/edge.rs` | Tokio runtime for PDF, SSH, SSO, and IPP. An execution-context thread never enters it. | Native |
| `moqui-host/src/cluster_edge.rs` | Cluster HTTPS listener and peer client. [Cluster](archive/Cluster.md). | Redesign |
| `moqui-host/src/lib.rs` | Process commands (`run`, `load`, `save-db`, `load-save`, `test`, `lint`, `types`, `convert`). | Partial |
| `moqui/src/main.rs` | Binary entry. Calls `moqui_host::main`. Keeps linked component crates. | Native |

`MoquiContextListener` is factory startup inside `moqui-host/src/lib.rs`. `MoquiAbstractEndpoint` and `NotificationWebSocketListener` are the servlet WebSocket base; `notws.rs` replaces them. `ScreenResourceNotFoundException` is a not-found error from `screen/url.rs`. `web.xml` servlet declarations are the axum routes in `http.rs`.

## Search

README redesigns 1 and 3. The design is [SearchFacade](archive/SearchFacade.md). `ElasticFacade` and `ElasticFacadeImpl` are this section, not an entity datasource.

| Rust file | Primary basis | Status |
|---|---|---|
| `search/mod.rs` | `ElasticFacadeImpl` as `SearchFacade`: one backend, query and aggregation trees. | Redesign |
| `search/conf.rs` | `<search-facade>` (the Java conf was `<elastic-facade>`). | Redesign |
| `search/mapping.rs` | `ElasticFacadeImpl.makeElasticSearchMapping`, via `DataDocument` field types. | Port |
| `search/query.rs` | The query tree scripts pass. Replaces Elastic query builders. | Redesign |
| `search/parse.rs` | Lucene query-string subset the screens send. | Partial |
| `search/es_json.rs` | Elastic query JSON still passed in `nestedQueryMap` to `search#DataDocuments`. | Partial |
| `search/agg.rs` | Elastic aggregations the screens use (filter, date histogram, terms, metrics). | Partial |
| `search/highlight.rs` | Elastic highlight fragments (`encoder: html`). | Port |
| `search/feed.rs` | After-commit assembly and the receive service. Registration of the pending row stays `entity/load_feed/feed.rs`. | Redesign |
| `search/fresh.rs` | Read-your-writes for a search that asks. No Java counterpart. | Native |
| `search/logs.rs` | `ElasticSearchLogger` and `ElasticRequestLogFilter` index fields. The host ships the events. | Port |
| `search/opensearch.rs` | `ElasticFacadeImpl.ElasticClientImpl` HTTP client. | Partial |
| `moqui-host/src/search.rs` | Where the store lives, and `MoquiStart` launching a local OpenSearch install under `runtime/opensearch`. | Partial |
| `search/postgres/mod.rs`, `search/postgres/compile.rs`, `search/postgres/agg.rs` | Postgres full-text and JSON backend. | Native |
| `search/semantic.rs` | Local embeddings. `HashEmbedder` only; no model yet. | Native |
| `search/hybrid.rs` | Reciprocal-rank fusion over lexical, prefix, fuzzy, and semantic branches. | Native |

## Notifications and cluster

| Rust file | Primary basis | Status |
|---|---|---|
| `notify/message.rs` | `NotificationMessageImpl`, `NotificationMessage`. | Port |
| `notify/topic.rs` | `SimpleTopic` for this process. Listeners run on the publishing thread. | Port |
| `notify/mod.rs` | Module spine. The host WebSocket subscribes. | Native |
| `cluster/mod.rs` | Member table, shared certificate material, and fan-out of the cache-invalidation and notification topics. [Cluster](archive/Cluster.md). | Redesign |
| `moqui-host/src/cluster_notify.rs` | Optional Postgres `NOTIFY` transport (`cluster_transport=notify`). | Native |
| `moqui-host/src/ssh/cluster.rs` | Optional SSH transport (`cluster_transport=ssh`). [Ssh](archive/Ssh.md). | Native |

## Mail, print, images, spreadsheet, SSO, SFTP, EDI

The service section names the Rust service bodies. This section is where the add-on classes and the host clients go.

| Rust file | Primary basis | Status |
|---|---|---|
| `service/mail/` | The three Groovy mail scripts and both `EmailServices.xml`. | Port |
| `print.rs` | `moqui-cups` `PrintServices.xml` as a `PrintClient` the host installs. | Port |
| `moqui-host/src/ipp.rs` | CUPS4J calls (CUPS-Get-Printers, Print-Job, Hold, Release, Cancel, Get-Job-Attributes). | Port |
| `service/images.rs` | `moqui-image` service XML. ImageIO is the `image` crate. | Port |
| `service/spreadsheet.rs` | `moqui-poi` read and load services. | Port |
| `screen/xlsx.rs` | `FormListExcelRender`, `ScreenWidgetRenderExcel`. | Port |
| `sso.rs`, `service/sso.rs` | `AuthenticationFlow`, `AuthenticationClientFactory`, `MoquiSecurityGrantedAccessAdapter`, `AuthServices.xml`. Account and group sync stay here. [Sso](archive/Sso.md). | Port |
| `moqui-host/src/sso.rs` | pac4j OIDC and OAuth 2 clients, via huskarl. OAuth 1.0 is not carried. SAML is defined and reports that it is not available. | Partial |
| `sftp.rs` | `org.moqui.sftp.SftpClient`. Host-key handling is stricter than Java (pin, or trust-on-first-use). | Partial |
| `moqui-host/src/ssh/client.rs` | The russh session under `SftpClient`. | Port |
| `moqui-host/src/ssh/server.rs`, `ssh/sftp_fs.rs`, `ssh/shell.rs` | Built-in SSH server. Not in Java. The shell replaces `GroovyShellEndpoint` for admin commands. | Native |
| `moqui-host/src/ssh/mod.rs` | Module spine. | Native |
| `edi.rs` | `EdiHandler.groovy`. Grammars are JavaScript expressions, not Groovy scripts. | Port |

## Tooling and the binary

| Rust file | Primary basis | Status |
|---|---|---|
| `moqui-host/src/lib.rs`, `usage.rs` | `MoquiStart` commands that exist: run, load, save-db, load-save, and the help text. WAR packaging, the executable-WAR classpath, and H2 are not ported. | Partial |
| `moqui/src/main.rs` | Process entry that calls the host. | Native |
| `testkit/mod.rs`, `testkit/run.rs`, `testkit/rewrite.rs` | Spock, reimplemented as JavaScript specs. [TestHarness](archive/TestHarness.md). | Native |
| `moqui-host/src/test_cmd/mod.rs`, `test_cmd/fixture.rs`, `test_cmd/report.rs`, `test_cmd/coverage.rs` | `moqui test` driver, database fixture, JUnit XML, coverage. | Native |
| `typegen/mod.rs`, `typegen/entities.rs`, `typegen/services.rs`, `typegen/screens.rs`, `typegen/ts.rs` | TypeScript declarations from the loaded definitions. [TypeGeneration](archive/TypeGeneration.md). | Native |
| `moqui-host/src/types.rs` | `moqui types`. | Native |
| `moqui-host/src/qvt_report.rs` | `moqui lint --screens --qvt-static`. The report is `screen/statics/report.rs`. | Native |
| `native.rs` | Component JAR on the classpath: linked Rust services, tools, resource schemes, template renderers. [Native-Components](archive/Native-Components.md). | Native |

## Browser host

`crates/moqui-browser` has no Java class. It is a second host beside `moqui-host`. [Wasm](archive/Wasm.md). Against the server host it is Partial: there is no login flow, and service and entity calls run with authorization disabled.

| Rust file | What it is | Status |
|---|---|---|
| `moqui-browser/src/lib.rs`, `moqui-browser/src/main.rs`, `moqui-browser/src/exports.rs`, `moqui-browser/build.rs` | Emscripten module, JSON request exports, link flags. | Native |
| `moqui-browser/src/host.rs` | Factory boot on one thread. The server counterpart is `moqui-host` startup. | Partial |
| `moqui-browser/src/sqlite.rs` | SQLite `DbDriver`. One physical connection, savepoints for nested transactions. | Native |
| `moqui-browser/src/fetch.rs` | Outbound HTTP through `fetch`. Installed as `outbound.rs`'s sender. | Native |
| `moqui-browser/src/api.rs` | JSON ops: `init`, `checkTables`, `loadData`, `callService`, `render`, `find`, `store`, WebSocket deliver and send. | Native |
| `moqui-browser/src/ws.rs` | WebSocket frames between the worker and the module, including `/notws` notifications. | Native |

## Reverse index

Every file under `../moqui/framework/src/main`, `src/start/java/MoquiStart.java`, and the absorbed-component `src` trees. "Where" is the Rust module in the sections above.

### `org.moqui` root and context API

| Source | Where | Status |
|---|---|---|
| `Moqui.java` | `context/factory.rs` | Partial |
| `BaseException.java`, `BaseArtifactException.java` | `context/error.rs` | Port |
| `context/ExecutionContext.java`, `ExecutionContextFactory.java` | `context/factory.rs` | Port |
| `context/ArtifactExecutionFacade.java`, `ArtifactExecutionInfo.java` | `context/artifact.rs`, `user/authz.rs` | Port |
| `context/CacheFacade.java` | `context/cache.rs` | Port |
| `context/ElasticFacade.java` | `search/` | Redesign |
| `context/L10nFacade.java` | `l10n.rs` | Port |
| `context/LoggerFacade.java`, `LogEventSubscriber.java` | `moqui-host/src/logging.rs` | Partial |
| `context/MoquiLog4jAppender.java` | — | Excluded |
| `context/MessageFacade.java`, `MessageFacadeException.java` | `context/message.rs` | Port |
| `context/NotificationMessage.java`, `NotificationMessageListener.java` | `notify/` | Port |
| `context/ResourceFacade.java` | `context/resource.rs` | Port |
| `context/ScriptRunner.java` | `script/` | Partial |
| `context/TemplateRenderer.java` | `script/template.rs` | Redesign |
| `context/ToolFactory.java` | `native.rs` | Native |
| `context/TransactionFacade.java`, `TransactionInternal.java`, `TransactionException.java` | `transaction/` | Port |
| `context/UserFacade.java` | `user/` | Port |
| `context/WebFacade.java`, `WebMediaTypeException.java` | `web/mod.rs` | Port |
| `context/ValidationError.java` | `service/validate.rs` | Port |
| `context/ArtifactAuthorizationException.java`, `ArtifactTarpitException.java`, `AuthenticationRequiredException.java`, `PasswordChangeRequiredException.java`, `SecondFactorRequiredException.java` | `context/error.rs` with `user/authz.rs`, `user/policy.rs`, `user/factor.rs` | Port |
| `context/LlmFacade.java` | — | Excluded |

### `org.moqui.entity`, `etl`, `jcache`, `resource`, `screen`, `service`, `util`

| Source | Where | Status |
|---|---|---|
| `entity/EntityFacade.java`, `EntityValue.java`, `EntityList.java`, `EntityFind.java`, `EntityDataLoader.java`, `EntityDataWriter.java`, `EntityDynamicView.java`, `EntityDatasourceFactory.java`, `EntityCondition.java`, `EntityConditionFactory.java` | `entity/` and `moqui-db` | Port |
| `entity/EntityListIterator.java` | `entity/find.rs` | Port |
| `entity/EntityException.java`, `EntityNotFoundException.java`, `EntityValueNotFoundException.java` | `context/error.rs` | Port |
| `etl/SimpleEtl.java`, `FlatXmlExtractor.java` | `moqui-lib/src/etl.rs` | Port |
| `jcache/MCache.java`, `MCacheConfiguration.java`, `MEntry.java`, `MStats.java` | `moqui-lib/src/cache.rs` | Partial |
| `jcache/MCacheManager.java` | — | Excluded |
| `resource/ResourceReference.java` | `moqui-lib/src/resource.rs`, `context/resource.rs` | Partial |
| `resource/ClasspathResourceReference.java` | `context/locate.rs` | Partial |
| `resource/UrlResourceReference.java` | `context/resource.rs`, `moqui-lib/src/http.rs` | Partial |
| `screen/ScreenFacade.java`, `ScreenRender.java`, `ScreenTest.java` | `screen/` | Port |
| `service/ServiceFacade.java`, `ServiceCall.java`, `ServiceCallSync.java`, `ServiceCallAsync.java`, `ServiceCallJob.java`, `ServiceCallSpecial.java`, `ServiceCallback.java` | `service/call.rs`, `pool.rs`, `jobs.rs` | Port |
| `service/ServiceException.java` | `context/error.rs` | Port |
| `util/MNode.java` | `moqui-lib/src/node/` | Port |
| `util/ObjectUtilities.java` | `moqui-lib/src/object.rs` | Port |
| `util/CollectionUtilities.java` | `moqui-lib/src/collection.rs` | Partial |
| `util/StringUtilities.java` | `moqui-lib/src/string.rs` | Partial |
| `util/ContextStack.java` | `moqui-lib/src/context_stack.rs` | Port |
| `util/ContextBinding.java` | `script/runtime.rs` | Redesign |
| `util/RestClient.java` | `moqui-lib/src/http.rs` | Partial |
| `util/SimpleTopic.java` | `notify/topic.rs` | Port |
| `util/SystemBinding.java` | `moqui-lib/src/env.rs` | Partial |
| `util/WebUtilities.java` | `entity/sensitive.rs`, `web/mod.rs`, `web/multipart.rs`, `screen/sri/mod.rs`, `service/system_sync/message.rs`, `script/runtime/ec_extra.rs` | Partial |
| `util/LiteStringMap.java`, `MClassLoader.java` | — | Excluded |
| `llm/**`, `org.moqui.llm.**` | — | Excluded |

### `org.moqui.impl`

| Source | Where | Status |
|---|---|---|
| `actions/XmlAction.java` | `script/actions.rs`, `walker.rs` | Port |
| `context/ExecutionContextFactoryImpl.groovy`, `ExecutionContextImpl.java` | `context/factory.rs`, `service/request_pool.rs` | Port |
| `context/ContextJavaUtil.java` | `moqui-lib/src/json.rs`, `screen/statics.rs` (`unwrap_map`) | Partial |
| `context/ArtifactExecutionFacadeImpl.groovy`, `ArtifactExecutionInfoImpl.java` | `context/artifact.rs`, `user/authz.rs`, `user/filter.rs` | Port |
| `context/CacheFacadeImpl.groovy` | `context/cache.rs` | Port |
| `context/ElasticFacadeImpl.groovy` | `search/` | Redesign |
| `context/EntityTxCache.groovy`, `TransactionCache.groovy` | `entity/cache.rs` | Port |
| `context/TransactionCacheDb.java` | `entity/cache.rs`, `moqui-host/src/db/sim.rs` | Port |
| `context/TransactionFacadeImpl.groovy` | `transaction/manager.rs` | Port |
| `context/TransactionInternalBitronix.groovy` | `transaction/` as a behavior reference | Partial |
| `context/L10nFacadeImpl.java` | `l10n.rs`, `moqui-lib/src/format.rs` | Port |
| `context/LoggerFacadeImpl.groovy` | `moqui-host/src/logging.rs` | Partial |
| `context/MessageFacadeImpl.groovy` | `context/message.rs` | Port |
| `context/NotificationMessageImpl.groovy` | `notify/message.rs` | Port |
| `context/ResourceFacadeImpl.groovy` | `context/resource.rs` | Port |
| `context/reference/BaseResourceReference.java`, `ComponentResourceReference.java`, `WrapperResourceReference.groovy` | `context/resource.rs` | Port |
| `context/reference/DbResourceReference.groovy` | `entity/dbresource.rs` | Port |
| `context/reference/ContentResourceReference.groovy` | — | Excluded |
| `context/renderer/FtlTemplateRenderer.java`, `GStringTemplateRenderer.groovy`, `FtlMarkdownTemplateRenderer.groovy` | — | Excluded |
| `context/renderer/MarkdownTemplateRenderer.groovy` | `moqui-lib/src/markdown.rs` | Partial |
| `context/renderer/NoTemplateRenderer.groovy` | `script/template.rs` (pass-through class name) | Partial |
| `context/runner/XmlActionsScriptRunner.groovy` | `script/actions.rs` | Port |
| `context/runner/GroovyScriptRunner.groovy`, `JavaxScriptRunner.groovy` | — | Excluded |
| `context/UserFacadeImpl.groovy` | `user/` | Port |
| `context/WebFacadeImpl.groovy` | `web/mod.rs`, `service/system_sync/message.rs` | Port |
| `context/SimFence.java`, `SimTerminatedException.java` | `entity/cache.rs`, `moqui-lib/src/http.rs` | Port |
| `entity/EntityFacadeImpl.groovy`, `EntityDefinition.groovy`, `EntityValueBase.java`, `EntityValueImpl.java`, `EntityListImpl.java`, `FieldInfo.java`, `EntityJavaUtil.java` | `entity/`, `moqui-db` | Port |
| `entity/EntityFindBase.groovy`, `EntityFindImpl.java`, `EntityFindBuilder.java`, `EntityQueryBuilder.java` | `entity/find.rs`, `moqui-db/src/sql/` | Port |
| `entity/EntityListIteratorImpl.java`, `EntityListIteratorWrapper.java` | `entity/find.rs` | Port |
| `entity/EntityCache.groovy` | `entity/cache.rs` | Port |
| `entity/EntityDbMeta.groovy`, `FtsDdl.java`, `FtsSql.java` | `entity/meta.rs` | Port |
| `entity/EntityDataLoaderImpl.groovy`, `OverlayColumnNames.java` | `entity/load_feed/` | Port |
| `entity/EntityDataWriterImpl.groovy` | `entity/data_writer.rs` | Port |
| `entity/EntityDataFeed.groovy` | `entity/load_feed/feed.rs`, `search/feed.rs` | Partial |
| `entity/EntityDataDocument.groovy` | `entity/document.rs` | Port |
| `entity/EntityEcaRule.groovy` | `entity/load_feed/eca.rs`, `service/eca.rs` | Port |
| `entity/EntityDatasourceFactoryImpl.groovy` | `entity/datasource.rs` | Port |
| `entity/EntityDynamicViewImpl.groovy` | `entity/query_rest.rs`, `moqui-db` | Port |
| `entity/EntityConditionFactoryImpl.groovy`, `condition/*` | `moqui-db/src/sql/condition.rs`, `model/condition.rs` | Port |
| `entity/AggregationUtil.java` | `screen/aggregate.rs` | Port |
| `entity/EntitySqlException.groovy` | `context/error.rs` | Port |
| `entity/elastic/ElasticDatasourceFactory.groovy`, `ElasticEntityFind.java`, `ElasticEntityListIterator.java`, `ElasticEntityValue.java`, `ElasticSynchronization.groovy` | — | Excluded |
| `screen/ScreenDefinition.groovy`, `ScreenSection.groovy`, `ScreenWidgets.groovy` | `screen/definition.rs` | Port |
| `screen/ScreenForm.groovy` | `screen/form.rs`, `saved_finds.rs` | Port |
| `screen/ScreenRenderImpl.groovy` | `screen/render.rs`, `sri/` | Port |
| `screen/ScreenUrlInfo.groovy` | `screen/url.rs`, `static_files.rs` | Port |
| `screen/ScreenTree.groovy` | `screen/tree.rs` | Port |
| `screen/ScreenTestImpl.groovy` | `screen/screen_test.rs` | Port |
| `screen/ScreenFacadeImpl.groovy` | `screen/cache.rs`, `info.rs` | Port |
| `screen/ScreenWidgetRender.java`, `ScreenWidgetRenderFtl.groovy` | `screen/generate.rs` and the JS macro modules | Port |
| `screen/WebFacadeStub.groovy` | `screen/render.rs` `make_render` | Port |
| `service/ServiceFacadeImpl.groovy`, `ServiceDefinition.java`, `ParameterInfo.java` | `service/define.rs`, `validate.rs`, `info.rs` | Port |
| `service/ServiceCallImpl.java`, `ServiceCallSyncImpl.java`, `ServiceCallSpecialImpl.groovy` | `service/call.rs` | Port |
| `service/ServiceCallAsyncImpl.groovy` | `service/pool.rs` | Port |
| `service/ServiceCallJobImpl.groovy`, `ScheduledJobRunner.groovy` | `service/jobs.rs`, `schedule.rs` | Port |
| `service/ServiceEcaRule.groovy` | `service/eca_rules.rs`, `seca.rs`, `rule_actions.rs` | Port |
| `service/EmailEcaRule.groovy` | `service/email_eca.rs` | Port |
| `service/RestApi.groovy` | `service/rest.rs`, `screen/schema.rs` | Partial |
| `service/runner/EntityAutoServiceRunner.groovy` | `service/call.rs`, `entity/` | Port |
| `service/runner/InlineServiceRunner.java`, `ScriptServiceRunner.java` | `service/call.rs`, `script/` | Port |
| `service/runner/RemoteRestServiceRunner.groovy` | `service/remote.rs` | Port |
| `service/runner/ServiceRunner.groovy` | `service/define.rs` | Port |
| `service/runner/JavaServiceRunner.groovy`, `RemoteJsonRpcServiceRunner.groovy`, `ServiceJsonRpcDispatcher.groovy` | — | Excluded |
| `tools/MCacheToolFactory.java` | `context/cache.rs` | Port |
| `tools/H2ServerToolFactory.groovy`, `JackrabbitRunToolFactory.groovy`, `JCSCacheToolFactory.groovy`, `SubEthaSmtpToolFactory.groovy` | — | Excluded |
| `util/EdiHandler.groovy` | `edi.rs` | Port |
| `util/ElasticSearchLogger.groovy` | `search/logs.rs`, `moqui-host/src/logging.rs` | Port |
| `util/ElFinderConnector.groovy` | `service/elfinder.rs` | Port |
| `util/JdbcExtractor.java` | `moqui-host/src/db/jdbc_extract.rs` | Port |
| `util/RestSchemaUtil.groovy` | `entity/schema.rs`, `screen/schema.rs` | Partial |
| `util/SimpleSigner.java` | `moqui-lib/src/signer.rs` | Port |
| `util/UserSecurityChecks.groovy` | `service/security_checks.rs` | Port |
| `util/SimpleSgmlReader.groovy`, `MoquiShiroRealm.groovy`, `MoquiWebSecurityManager.groovy` | — | Excluded |
| `webapp/MoquiServlet.groovy` | `moqui-host/src/http.rs` | Port |
| `webapp/MoquiAuthFilter.groovy` | `screen/mod.rs`, `moqui-host/src/search_proxy.rs` | Port |
| `webapp/MoquiContextListener.groovy` | `moqui-host/src/lib.rs` | Partial |
| `webapp/NotificationEndpoint.groovy`, `NotificationWebSocketListener.groovy`, `MoquiAbstractEndpoint.groovy` | `moqui-host/src/notws.rs` | Port |
| `webapp/ElasticRequestLogFilter.groovy` | `moqui-host/src/logging.rs` | Port |
| `webapp/MoquiFopServlet.groovy`, `GroovyShellEndpoint.groovy`, `LlmServlet.groovy`, `A2AServlet.groovy`, `A2ACardServlet.groovy` | — | Excluded |
| `webapp/MoquiSessionListener.groovy` | `user/session.rs` does not copy session bytes | Excluded |
| `webapp/ScreenResourceNotFoundException.groovy` | `screen/url.rs`, `context/error.rs` | Port |
| `llm/**` | — | Excluded |

### Start class, resources, and absorbed components

| Source | Where | Status |
|---|---|---|
| `src/start/java/MoquiStart.java` | `moqui-host/src/lib.rs`, `search.rs` (local OpenSearch). WAR and H2 paths are not ported. | Partial |
| `pollEmailServer.groovy`, `sendEmailMessage.groovy`, `sendEmailTemplate.groovy` | `service/mail/` | Port |
| `MoquiDefaultConf.xml` | `context/conf.rs`. Servlet and proxy entries also `http.rs` and `search_proxy.rs`. | Port |
| `log4j2.xml` | `moqui-host/src/logging.rs` (file names and levels, not Log4j) | Partial |
| `shiro.ini`, `web.xml`, `cache.ccf`, `bitronix-default-config.properties` | — | Excluded |
| `moqui-sso` `AuthenticationClientFactory.groovy` | `moqui-host/src/sso.rs` | Partial |
| `moqui-sso` `AuthenticationFlow.groovy`, `MoquiSecurityGrantedAccessAdapter.groovy` | `sso.rs`, `service/sso.rs` | Port |
| `moqui-sftp` `SftpClient.groovy` | `sftp.rs`, `moqui-host/src/ssh/client.rs` | Partial |
| `moqui-poi` `FormListExcelRender.groovy`, `ScreenWidgetRenderExcel.groovy` | `screen/xlsx.rs` | Port |
| `moqui-fop` `FopToolFactory.groovy`, `HtmlRenderer.groovy`, `HtmlRenderServlet.groovy` | — | Excluded |
| `moqui-image` service XML (no Groovy) | `service/images.rs` | Port |
| `moqui-cups` `PrintServices.xml` (no Groovy; CUPS4J) | `print.rs`, `moqui-host/src/ipp.rs`, `service/tools.rs` | Port |

## Cross-cutting threads

The entity and service review already compares these across phases. They are still the right threads:

1. **Transaction, commit, and entity-cache invalidation.** `TransactionFacadeImpl` enlists in `TransactionCache` / `TransactionCacheDb`. Compare frame, flush, and rollback order across `transaction/manager.rs`, `entity/cache.rs`, `entity/facade.rs`, and `service/call.rs`.
2. **Authorization.** `EntityFacadeImpl` and `ServiceFacadeImpl` gate through `ArtifactExecutionFacade`. Compare where `user/authz.rs` is applied and what it exempts.
3. **Config and defaults.** Values from `MoquiDefaultConf.xml`: cache sizes, page sizes, cron, mail polling, timeouts.
4. **Error type and message text.** `ServiceException` and `EntityException` text that callers or the UI match on. `context/error.rs` and `web` status mapping.
5. **Rule order.** Entity and service ECA order, and how a run-before that changes values affects the running rule. `entity/load_feed/eca.rs`, `service/eca.rs`, `service/rule_actions.rs`.

These are pointers for a later review, not findings:

- **Search feed order.** Pending row after the transaction-cache flush and before commit; assembly after commit. `entity/load_feed/feed.rs`, `search/feed.rs`. README redesign 3.
- **Session flush.** `user/session.rs` against `UserFacadeImpl` session attributes, with the database row from README redesign 7.
- **Cluster fan-out.** `cluster/mod.rs` against the two Hazelcast topics that were kept (entity-cache invalidation, notification). [Cluster](archive/Cluster.md).
- **Screen macro output.** Generated JS against `DefaultScreenMacros.*.ftl`. The golden files under `screen/testdata` and `script/testdata/screen` are the current check.
- **Script context.** What `script/runtime.rs` puts on `ec` against [ContextObjects](archive/ContextObjects.md).
