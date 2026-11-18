# Attack surface: data and stored secrets

The database, field encryption, generated secrets, and the threat model for stored secrets. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Database

The transactional datasource's `database-conf-name` `embedded` (the default; also empty and `h2`) selects the embedded Postgres cluster. There is no H2 driver and no H2 TCP server.

Embedded Postgres is on when the entity datasource is embedded and the build has `embedded-postgres` (Dev, Lite, Maximal). The cluster directory is `runtime/db/embedded` and the data directory is `runtime/db/embedded/data`, mode `0700`. On Unix there is no TCP: `listen_addresses` is empty. The socket directory is the data directory, or `moqui-pg-<FNV-1a hash of the data directory path>` under the temp directory (`/tmp` when the temp path is not shell-safe) when the data directory path has a character outside `[A-Za-z0-9/._+@%:,-]`. That directory is created with mode `0700` when it is missing; the start refuses one that is not a directory of this user with mode `0700` (`embedded::socket_dir_private`; `crates/moqui-drivers/src/embedded.rs`, `launch`, `socket_dir_for`). `PORT` 5432 names the socket file `.s.PGSQL.5432`. A second postmaster (search, or the sim overlay) uses the same socket recipe. The client uses the bootstrap superuser and the password in the cluster `pgpass` file, over the Unix socket with no TLS. This tree does not set `unix_socket_permissions`. `pg_hba.conf` is what `initdb --auth password` writes (`postgresql_embedded` 0.22): method `password` for local and loopback connections. On Windows the same default listens on `127.0.0.1` and an ephemeral port.

The superuser password is the text of `runtime/db/embedded/pgpass`. MR Moqui writes it with mode `0600` before the setup (which then leaves it), or makes it `0600` when it is there (`embedded::private_password_file`), and each start reads it back. The wire client (`crates/moqui-drivers/src/pgwire.rs`, `authenticate`) answers a cleartext, an MD5 and a SCRAM-SHA-256 request with the same password, and uses no channel binding. `pg_dump`, `pg_restore`, `pg_basebackup` and `pg_verifybackup` get the password in `PGPASSWORD` of the child process. A `pg_dump` writes into a partial file made `0600` before it runs (`backup::private_partial`); a published backup directory is `0700` and its files `0600` (`crates/moqui-drivers/src/backup.rs`).

External Postgres or MySQL is off unless the transactional datasource's `database-conf-name` is `postgres`, `mysql`, or `mysql8`. This process is a client. Omitted ports default to 5432 and 3306. TLS is on unless the datasource's `sslmode` is `disable` or `disabled`, and there is no fallback from TLS to a plain connection. An empty the datasource's `sslmode`, `verify-full`, `allow`, `prefer` and any unknown value check the certificate's authority and host name. `verify-ca` checks the authority only. `require` encrypts and accepts any certificate, unless the datasource's `sslrootcert` is set, and then it checks as `verify-ca`. Without the datasource's `sslrootcert` the trusted authorities are the `webpki-roots` set (`crates/moqui-entity/src/entity/datasource.rs`, `tls_verify`; `crates/moqui-drivers/src/pg_tls.rs`; `crates/moqui-drivers/src/mysql.rs`, `ssl_opts`).

## Field encryption

`entity-facade@crypt-pass` ships as `MoquiDefaultPassword:CHANGEME`, a generated secret of the conf schema. The algorithm is `PBEWithHmacSHA256AndAES_128`, salt `20201202`, 10 iterations. The same secret encrypts an OAuth refresh token and a client secret stored on `EmailOAuthProvider`, the SSO client secrets (`OauthFlow.secret`, `OidcFlow.secret`), the SAML keystore and private key passwords, and a calendar publish link's token (`CalendarShare.publishToken`, still found by value since the encryption is deterministic). Alternate decrypt still tries `PBEWithMD5AndDES` with the current pass and with `entity-facade@crypt-pass-old`, and the AES algorithm with the old pass. The old pass ships as the same `CHANGEME` sentinel and is not replaced by the generator. What this encryption protects, and its known gaps, are in [Threat model: stored secrets](#threat-model-stored-secrets).

## Secrets

Startup, before listeners, replaces each secret `default-property` that still ends in `CHANGEME` after the environment and `runtime/.env` are read. The value is 32 CSPRNG bytes, base64, written into `runtime/.env`. Each write replaces the file through a sibling of mode `0600`, so the file is `0600` after any write. An unwritable `.env` fails startup. `entity-facade@crypt-pass-old` is the exception: the sentinel is kept. `cluster-member@tls-cert` and `cluster-member@tls-key` are generated as a pair when `cluster-tls` is on, which it is in every server package. The same path fills every other secret `default-property` that ships as `CHANGEME`: `acme@account-key`, `ssh-server@host-key`, `git-server@lfs-token-key`, `chat@store-key-secret`, `turn_auth_secret`, `channels@media-key`, `xmpp_upload_secret`, `xmpp_scram_decoy_key`, `webapp-list/webapp[webroot]/session-config@cookie-key`, `push/web@vapid-key` and `edge_link_key`. The key is then on the same host as the data it protects; see [Threat model: stored secrets](#threat-model-stored-secrets).

A `default-property` is a secret when `is-secret="true"`, or when its name contains `pass`, `pw` or `key` and `is-secret` is not `false` (`crates/moqui-core/src/context/factory.rs`, `is_secret`). The boot log names a secret property without its value and prints every other property with its value. The System configuration view (`crates/moqui-core/src/context/conf_reload/view.rs`) masks the `value` of a secret property, any attribute named or ending in `password`, `pass`, `secret`, `api-key`, `token-key`, `state-key` or `private-key`, and the expansion of a `${name}` whose name is a secret by the name rule. `component-deploy@attestation-secret` has no `is-secret` attribute and is a secret by its name. `llm-facade/endpoint[xai]@api-key`, `llm-facade/endpoint[anthropic]@api-key`, `llm-facade/endpoint[gemini]@api-key`, `llm-facade/endpoint[glm]@api-key` and `llm-facade/endpoint[deepseek]@api-key` are read by `${}` in `MoquiDefaultConf.xml` and are not declared there as a `default-property`. A script reads any property or environment value, a secret one too, with `System.getProperty(name)` (`crates/moqui-core/src/script/host/globals.rs`, `get_property`).

## Entity authorization and row filters

A find adds `EntityFilter` conditions only when all of these hold (`crates/moqui-core/src/user/filter.rs`): an execution context is entered, the entity name does not start with `moqui.`, authorization is not disabled on the context, `AT_ENTITY` authorization is enabled, and the artifact frame on top of the stack was authorized by an `ArtifactAuthz` record that has `ArtifactAuthzFilter` rows. So these finds get no filter:

- a find with authorization disabled (a service or job that disables it, a data document feed, an edge replication read);
- a find of a `moqui.*` entity;
- a find whose authorizing frame has no record, or a record with no filter set.

For a find that qualifies, each filter set of the record is taken in turn:

- A set whose `applyCond`, on `ArtifactAuthzFilter` or on `EntityFilterSet`, evaluates false is skipped, and its filters are not applied.
- An `EntityFilter` applies to a find of its `entityName`, and to a find of a view that has that entity as a direct member. Other finds do not evaluate its `filterMap`.
- On a view the condition goes on the alias of the filtered member field (`map_conditions`). With two members of the filtered entity aliased (two `Party` members), each member is held to the whole map on its own and the members' conditions are ANDed (`conditions_for`): `MANTLE_PRIVATE_CONTACT` becomes `(from is null or from = me) and (to is null or to = me)`, so a row with one shared party and one of the caller's own personal contacts is its owner's (SEC-DAT-11, fixed). With no such alias the find gets `1 = 0`, unless the set has `allowMissingAlias="Y"`, and then that field's condition is left out, logged as a warning the first time for each view and field (`filter.rs` `note_missing_alias`, SEC-DAT-11).
- An error while evaluating an `applyCond` or a `filterMap`, an unknown comparison, or a missing alias without `allowMissingAlias` adds `1 = 0` to the find and logs `entity filter failed closed`.
- A null value in a `filterMap` is `IS NULL`. An empty list is `1 = 0`.

The conditions are added by `one`, `list`, `count` and `iterator`, and so by the read of `updateAll` and `deleteAll` (`crates/moqui-entity/src/entity/find.rs`, `combined`). A create, update or delete of a value by its primary key is not filtered. A find that a filter may change is not read from the shared entity cache. A view that is not in the entity model (a data document's view) is filtered through its own definition (`apply_view_filters`).

The seeded sets: `MANTLE_USER_ORG` has no `applyCond`. `MANTLE_ACTIVE_ORG` has `applyCond="activeOrgId"` and `allowMissingAlias="Y"`. `MANTLE_PRIVATE_CONTACT` has `allowMissingAlias="N"` (`runtime/components/addon/mr-mantle/data/MantleSetupData.xml`), and the views over `Party` alias `privateOwnerUserId`. `COMMERCE_STORE` has `applyCond="ec.user.context.commerceStoreIds != null"` and `allowMissingAlias="Y"` (`runtime/components/addon/mr-boss/data/BossSetupData.xml`). `BOSS_PROJECT_CLIENT` has `allowMissingAlias="N"`.

## Finds from request parameters

`EntityFind::search_form_map` (`crates/moqui-entity/src/entity/search_form.rs`) turns a parameter map into conditions, order and paging. For every field of the entity, and every alias of a view, it reads `<field>`, `<field>_op` (`equals`, `like`, `contains`, `begins`, `empty`, `in`), `<field>_not`, `<field>_ic`, `<field>_period` with `_poffset` and `_pdate`, and `<field>_from` and `<field>_thru`. `orderByField` is a comma list of field names. `pageIndex` and `pageSize` page the find; the default page is 20 rows and a request's `pageSize` is held to `rest-max-page-size` (1000). `pageNoLimit=true` (a screen's CSV, XLSX, print or text export) removes the offset and reads at most `entity-facade` `screen-max-rows` (100,000). A limit the caller's code set on the find is its own (SEC-DAT-09, fixed). Values are bound parameters. Request input takes no condition and no order on a password field (by name or origin), and no order, pattern, range or period on an encrypted field (`GenericAccess::hidden`); an `equals` on an encrypted field stays for a screen's or script's own find (its bound value is encrypted before SQL, so it matches the stored value) and is refused on a generic find (`EntityFind::generic_input`: entity REST, Service REST entity resources, the data grid, `searchFormMap`'s `genericInput`, an `entity-find` whose entity name is expanded). A field in a screen's `skip-fields` is refused as an `orderByField` from the input too. The caller's own defaults and default order still may name any field (plans/Security-Review.md SEC-DAT-01, fixed).

Its callers:

- A screen's `<search-form-inputs>` (`searchFormInputs`, `crates/moqui-core/src/script/host/entities.rs`). The default input is the whole script context, request parameters included, and no skip list is passed.
- Entity REST (`crates/moqui-entity/src/entity/query_rest.rs`): [REST](Attack-Surface-Routes.md#rest). It removes `pageNoLimit`, reads 100 rows when no page is asked for, and replaces a `pageSize` above `rest-max-page-size` (1000) with it.
- A Service REST entity resource (`crates/moqui-core/src/service/rest.rs`, `find_entity`, `count_entity`). It drops `pageNoLimit` from the request, reads 100 rows when no page is asked for and caps a `pageSize` at `rest-max-page-size` (SEC-RTE-14). A screen's `pageSize` is held to it as well, through `search_form_map`.
- The Tools data grid (`crates/moqui-tools/src/grid.rs`). It refuses a sort, filter or condition on a `UserAccount` password field or an encrypted field (`hidden_value`).

## Entity definitions from database rows

An entity name the file model does not have is looked up as a `moqui.entity.view.DbViewEntity` row or as `DataDocument.<dataDocumentId>` (`crates/moqui-entity/src/entity/derived.rs`). The definition rows are read with authorization disabled and outside the caller's transaction. A find of such a view is not read from the entity cache. A `DbViewEntityAlias` row gives the alias name, member alias, field and `functionName`; a `DataDocumentField` row gives a `functionName` too. A `DbViewEntityAlias` or `DataDocumentField` `functionName` must be one of `derived.rs` `ROW_VIEW_FUNCTIONS` (min, max, sum, avg, count, count-distinct, upper, lower); another refuses the view (logged, not found) or the document (`check_row_function`, SEC-DAT-04). The report dataset services take the same list (`crates/moqui-report/src/business.rs` `FUNCTIONS`). A view defined in XML is code, and its function is checked only as one word of ASCII letters, digits and `_` (`crates/moqui-db/src/sql/view.rs`, `check_word`). The Tools screens that write these rows are in [Tools and System](Attack-Surface-Tools.md#tools-and-system).

## Export, snapshots and EntitySync

`ec.entity.makeDataWriter()` (`crates/moqui-entity/src/entity/data_writer.rs`) writes entities as XML, JSON or CSV to text, a file, a directory or a zip. Without `full(true)` it skips the restricted entities (one message naming them) and writes no password or encrypted field, in dependents and masters too (`RecordGuard::File`); with it, it writes every field as the facade reads it, encrypted fields decrypted and `UserAccount` password hashes. The script host's `makeDataWriter` sets `full` only for a holder of the sealed `ADMIN_BACKUP_DOWNLOAD` (`ADMIN` in the seed), so Tools Export and Snapshots are whole for such a holder and carry no secrets for anyone else (SEC-TLS-05, fixed). A target file is created new (`File::create_new`) with the mode the process umask gives, and a zip target's missing parent directories are created.

`org.moqui.impl.EntityServices.export#EntityDataSnapshot` writes `runtime/db/snapshot/<baseFilename>.zip`; with no entity list it writes every entity. The service refuses a `baseFilename` that is not one safe path segment (`WebUtilities.isSafeSinglePathSegment`, SEC-TLS-07). `import#EntityDataSnapshot` checks `zipFilename` with `isSafeSinglePathSegment`. A zip data file is read entry by entry; a nested zip is at most 256 MiB and four levels deep. The data loader refuses identity-admin, secret-config and audit entities (`crates/moqui-entity/src/entity/sensitive.rs`, `is_restricted_generic_entity`) only when `restrictSensitiveEntities` is set; `UserAccount` is not on that list. `put#EntitySyncData` sets it, and a snapshot import does not. The Tools screen that starts, downloads, uploads and imports snapshots is in [Tools and System](Attack-Surface-Tools.md#tools-and-system).

`org.moqui.impl.EntitySyncServices.get#EntitySyncData` and `put#EntitySyncData` are reached at `/rest/s1/moqui/entity/syncs/data/get` and `.../put` (`runtime/components/base/tools/service/moqui.rest.xml`), under the `/moqui` REST path authorization, which the seed gives to `ADMIN`. `get` takes `entityIncludeList` from the caller, with entity names and filter maps, skips a restricted entity (logged), and returns the records as `entity-facade-xml` text built in memory with no password field and no restricted related record (`RecordGuard::Replica`); encrypted fields are decrypted, as a replica needs them, and `recordThreshold` is at most 10,000. `put` loads `entityData` with the restricted-entity check, refuses the service-written entities, a password field (`LoadOptions::refuse_password_fields`), `moqui.service.job.*` and service elements (`crates/moqui-core/src/service/system_sync/entity_sync.rs`; SEC-DAT-08). An outbound sync posts JSON to `<targetServerUrl>/rest/s1/moqui/entity/syncs/data/<op>` with `Authorization: Basic` of `targetUsername` and the decrypted `targetPassword`; the target is held by the row rule of `allowPrivateNetwork` ([The internal-address guard](Attack-Surface-Outbound.md)).

## Logs and spans

`crates/moqui-host/src/logging.rs` writes each event as given; it has no redaction of its own. `moqui.log` and `moqui-error.log` are created `0600`, and a new log directory `0700` (`logging::Rolling`, `moqui_lib::private_fs`); an existing file keeps its mode until it rolls. SQL text in a span (`db.query.text`) and in the slow-statement warning goes through `redact_sql` (`crates/moqui-entity/src/entity/exec.rs`): quoted literals become `?` and the text is cut near 4 KiB. Entity values are bound parameters and are not in the SQL text. A span's error text is cut at 4,096 characters and passed through `redact_text` (`crates/moqui-core/src/telemetry/mod.rs`), which replaces the value after `password`, `authorization`, `api_key`, `access_token`, `refresh_token`, `token` or `secret` followed by `:` or `=`. A decrypt failure logs the entity name, the field name and the error, not the value.

## Threat model: stored secrets

This section says what a stored secret is protected from, and what it is not protected from. Each claim names its test; [Proof-Tests, Stored secrets](Proof-Tests.md#stored-secrets) lists them all. A test whose name starts with `known_gap_` pins a weakness: when the weakness is fixed, the test fails, and this section must change with it.

The short answer: field encryption keeps secrets out of a stolen database dump or a database backup that does not include `runtime/.env`. It does not protect them from anyone who has that file, the host, or code that runs in the server. It is not suitable as a personal password manager.

### Assets

| Asset | Where | Protection |
| --- | --- | --- |
| Encrypted fields | Entity fields with `encrypt="true"`: 64 fields in `runtime/components` (47 in the base framework, 12 in `addon/mr-mantle`, 5 in `dev/dev-aide`), listed in [Encrypted fields](#encrypted-fields): OAuth refresh and access tokens, client secrets, mail and SMTP passwords, private keys and their passwords, API keys, bearer tokens, shared secrets, auth JSON, card and gift-card numbers and PINs, party identification values, edge replica rows | AES-128-CBC under `moqui_conf__entity_facade__crypt_pass` ([Field encryption](#field-encryption)) |
| Generated secrets | `runtime/.env`: `moqui_conf__entity_facade__crypt_pass`, `webapp-list/webapp[webroot]/session-config@cookie-key`, `ssh-server@host-key`, `cluster-member@tls-cert` and `cluster-member@tls-key`, `git-server@lfs-token-key`, `acme@account-key`, `chat@store-key-secret`, `turn_auth_secret`, `channels@media-key`, `xmpp_upload_secret`, `xmpp_scram_decoy_key`, `push/web@vapid-key`, `edge_link_key`. `mail-delivery/dkim@key` is made at first need and written there too | Mode `0600` on Unix ([Secrets](#secrets)) |
| Pairing secrets | `runtime/.env`: `edge_node_key`, `edge_node_pin`, `front_node_key`, `front_node_pin`, written by pairing (`crates/moqui-core/src/edge/mod.rs`). `runtime/db/edge_member_key`: a cluster member's own Ed25519 link key | `.env` mode `0600`; the key file mode `0600` (`crates/moqui-host/src/edge_link/member_key.rs`) |
| External secrets | Set by the operator in `runtime/.env`, the environment (`/etc/moqui/environment` under `systemd/moqui.service`) or a conf file: the datasources' `password` (`entity-facade/datasource[transactional]@password`, the content and clone ones), `search-facade/cluster@password`, `email-client/oauth-provider[google]@client-secret`, `email-client/oauth-provider[microsoft]@client-secret`, `push/fcm@service-account`, `llm-facade/endpoint[default]@api-key` and the other `llm_*_api_key` properties, `llm-facade/mcp@request-state-key`, `git-server@lfs-token-key-previous`, `component-deploy@attestation-secret` | As the operator stores them. `runtime/conf/MoquiDevConf.xml` holds the transactional datasource's `password` values in clear in its commented database examples |
| Session key | `webapp-list/webapp[webroot]/session-config@cookie-key`; other signing keys derive from it as HMAC-SHA256 of a label (`derived_cookie_key`, `crates/moqui-core/src/user/session.rs:3050`). Its SHA-256 is stored as the cluster member's session-key fingerprint (`crates/moqui-core/src/cluster/mod.rs`, `configured_fingerprints`) | Kept only in `.env` and process memory |
| Derived keys | The field AES key (PBKDF2 of `moqui_conf__entity_facade__crypt_pass`). The ACME account key (a P-256 scalar from SHA-256 of `acme@account-key`, `crates/moqui-host/src/acme/order.rs`). The MCP `requestState` key (`llm-facade/mcp@request-state-key`, else a key derived from `webapp-list/webapp[webroot]/session-config@cookie-key`, else a random key of the process, `crates/moqui-llm/src/mcp/state.rs`). A chat store key for each device (HMAC-SHA256 of user, device and revision under `chat@store-key-secret`, `crates/moqui-core/src/service/chat_keys.rs`) | Process memory; made again from the stored secret |
| Password and login-key hashes | `UserAccount.currentPassword`, `UserLoginKey` | argon2id for passwords; SHA-256 for login keys ([Login lockout and passwords](Attack-Surface-Web.md#login-lockout-and-passwords)) |
| Embedded database password | `runtime/db/embedded/pgpass`, and `PGPASSWORD` of a `pg_dump` or backup child process | The file is `0600`; the socket is in the `0700` data directory, or in a socket directory under temp that must be this user's with mode `0700` ([Database](#database)) |
| Key files | `runtime/cert/<name>/privkey.pem`: an ACME certificate's private key, written from the encrypted `AcmeCertificate.privateKeyPem` (`crates/moqui-host/src/acme/store.rs`). `runtime/conf/qz-private-key.pem` or `runtime/classes/qz-private-key.pem`: the QZ Tray signing key (`crates/moqui-core/src/web/mod.rs`) | `privkey.pem` is written mode `0600` (`crates/moqui-host/src/tls.rs`, `write_private`); the QZ key is as the operator stores it |
| Snapshot and export files | `runtime/db/snapshot/*.zip` and any full data writer output (a holder of `ADMIN_BACKUP_DOWNLOAD`): every entity's fields decrypted, and `UserAccount` password hashes ([Export, snapshots and EntitySync](#export-snapshots-and-entitysync)) | `0600`, new directories `0700` (`data_writer::write_new`) |
| Client device keys | Desktop and terminal clients | OS key store, else `FileKeyStore` files mode `0600` (`crates/moqui-desktop/src/client/keystore.rs:131`, `crates/moqui-client/src/keys.rs:48`) |

### Encrypted fields

`encrypt="true"` is on 66 field elements outside XML comments in `runtime/components`, which are 65 fields: `PartyIdentification.idValue` is declared in the entity and again in an `extend-entity`. No component other than these three has one. One more, `UserLoginHistory.passwordUsed`, is inside a comment.

- Base framework, 47 (`runtime/components/base/framework/entity/`):
  - `AgentEntities.xml`: `AgentPushConfig.token`, `.authenticationJson`, `AgentRemoteA2a.authJson`, `AgentMcpServer.authJson`, `AgentMcpUserToken.tokenJson`.
  - `BasicEntities.xml`: `EmailServer.mailPassword`, `.smtpPassword`, `.oauthRefreshToken`, `EmailServerAccessToken.accessToken`, `EmailOAuthProvider.clientSecret`.
  - `ChatEntities.xml`: `ChatBridge.configJson`. `DatabaseEntities.xml`: `DatabaseConnection.password`.
  - `EdgeEntities.xml`: `EdgeReplicaValue.pkJson`, `.fieldsJson`, `EdgeReplicaConflict.itemKey`, `.pkJson`, `.fieldsJson`.
  - `EntityEntities.xml`: `EntitySync.targetPassword`. `GitCiEntities.xml`: `GitCiSecret.secretValue`, `GitCiEnvironment.targetToken`. `LlmEntities.xml`: `LlmEndpoint.apiKey`. `MonitorEntities.xml`: `ScrapeTarget.bearerToken`.
  - `ResourceEntities.xml`: `GitRepository.accessToken`, `GitWebhook.secret`, `GitRepositoryMirror.credential`.
  - `SecurityEntities.xml`: `UserAuthcFactor.factorOption`, `WebPushSubscription.authSecret`, `OauthFlow.secret`, `OidcFlow.secret`, `SamlFlow.keystorePassword`, `.privateKeyPassword`.
  - `ServerEntities.xml`: `DeployedComponent.accessToken`, `InstanceHost.password`, `.tlsClientKey`, `InstanceImage.password`, `InstanceImageTypeEnv.envValue`, `DatabaseHost.adminPassword`, `AppInstanceEnv.envValue`, `AcmeCertificate.privateKeyPem`.
  - `ServiceEntities.xml`: `SystemMessageRemote.password`, `.privateKey`, `.sharedSecret`, `.sendSharedSecret`.
  - `VoiceEntities.xml`: `VoiceTrunk.authJson`, `SipDeviceVerifier.ha1`. `XmppEntities.xml`: `XmppFastToken.token`, `XmppPushRegistration.optionsJson`.
- mr-mantle, 13 (`runtime/components/addon/mr-mantle/entity/`):
  - `AccountingAccountEntities.xml`: `BankAccount.accountNumber` (a plain number stored before is encrypted by `migrate#BankAccountNumbers` at a seed load), `FinancialAccount.finAccountCode`, `.finAccountPin`, `CreditCard.cardNumber`, `.cardSecurityCode`, `GiftCard.cardNumber`, `.pinNumber`, `GiftCardFulfillment.cardNumber`, `.pinNumber`.
  - `EventRegistrationEntities.xml`: `EventRegistrationPolicy.entryCodeKey`. `PartyEntities.xml`: `Person.mothersMaidenName`, `PartyIdentification.idValue` (again in `InvoiceWorkflowEntities.xml`). `WorkEffortEntities.xml`: `CalendarShare.publishToken`.
- dev-aide, 5 (`runtime/components/dev/dev-aide/entity/DevAideEntities.xml`): `DevAideServer.envPriorJson`, `.gatewayKey`, `.pendingVerifier`, `DevAideCredential.token`, `DevAideTaskSource.token`.

Seven of them also have `enable-audit-log="update"`, so a change writes the old and the new value to `EntityAuditLog`, each encrypted again: `SystemMessageRemote.password`, `.privateKey`, `.sharedSecret`, `.sendSharedSecret`, `EmailServer.mailPassword`, `.smtpPassword` and `PartyIdentification.idValue`.

Fields that hold a bank or tax identifier or a token and are neither encrypted nor hashed: `BankAccount.routingNumber`, `TaxAuthorityParty.partyTaxId`, `Employee.taxAccountNumbers` and `Employment.taxAccountNumbers`, `PartyCarrierAccount.accountNumber`, `PayPalAccount.expressCheckoutToken` (mr-mantle); `UserAccount.passwordHint`, `HttpSession.sessionToken`, `ResourceLock.lockToken` and `RecoveryAction.leaseToken` (base framework).

### Where a decrypted value goes

The entity facade decrypts at each row read and encrypts at each write (`crates/moqui-entity/src/entity/field_rw.rs`). An `EntityValue` in memory holds plaintext.

| Place | Form |
| --- | --- |
| Entity cache, shared and transaction | Plaintext, in process memory |
| `EntityAuditLog.oldValueText`, `newValueText` | Ciphertext made again from the plaintext, cut at 4,000 characters; plaintext when `crypt-pass` is empty |
| Entity export and snapshots (`makeDataWriter`) | Left out unless the writer is full (`ADMIN_BACKUP_DOWNLOAD`); a full writer writes plaintext and `UserAccount` password hashes as stored |
| Data documents | Not carried: a `DataDocumentField` on an encrypted or `UserAccount` password field is left out of the document, its view, the feed and the index, with a warning naming it (`document.rs` `document_info`, SEC-DAT-02). The `MantleParty` document no longer lists `identifications:idValue`; documents indexed before keep it until the `mantle` index is rebuilt. A report dataset refuses an encrypted field (`crates/moqui-report/src/business.rs`) |
| `get#EntitySyncData` reply and an EntitySync push | Plaintext in `entity-facade-xml` text; password fields left out |
| Entity REST and Service REST entity resources | Not carried: each encrypted field is answered as `<field>IsSet` (true or false), in nested records too, and the five `UserAccount` password fields are removed ([Attackers](#attackers), administrator row) |
| Cluster cache invalidation | Not carried: entity name and primary key only |
| Edge replication | Plaintext JSON of the set's fields inside the link message; encrypted again on the edge node as `EdgeReplicaValue.fieldsJson`. `VoiceTrunk.authJson` is in the `voiceTrunks` set |
| Tools data grid | Masked as `********`; not editable, sortable or filterable |
| LLM data tools | Not carried: an encrypted field is left out of the field list (`crates/moqui-llm/src/tools/data.rs`) |
| SQL spans and the slow-statement log | Not carried: values are bound, and a bound value of an encrypted field is ciphertext |
| `pg_dump` and physical backups | Ciphertext, as stored |
| Scripts and services | Plaintext through any entity read the caller is authorized for |
| ACME key file | Plaintext PEM from `AcmeCertificate.privateKeyPem`, in `runtime/cert/<name>/privkey.pem` |
| A read that fails to decrypt | The placeholder `_DECRYPT_FAILED_`, which a write refuses. A stored value with no colon that is not hex is the placeholder too, unless `entity-facade` `crypt-read-plain` (`entity-facade@crypt-read-plain`) is on for a migration: then it is returned as stored, each read logged |

### Attackers

| Attacker | What is protected | What is not protected | Tests |
| --- | --- | --- | --- |
| Stolen database dump or database backup, without `.env` | Encrypted fields are ciphertext; the stored text and its bytes do not hold the plaintext. Password hashes are argon2id | Which secrets are equal, and which share their first 16 bytes. Login-key hashes are fast SHA-256, but a login key is random and expires in 144 hours. Fields of an install with an empty `crypt-pass` are plaintext. The embedded dump is of the whole `moqui` database, a `search` schema included. A snapshot or export file is not ciphertext | `a_stored_value_does_not_hold_its_plaintext`, `try_insert_seals_the_password_before_sql`, `hash_password_is_an_argon2id_phc_string`, `hash_login_key_stays_sha256_hex`, `known_gap_equal_plaintexts_give_equal_ciphertext`, `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block`, `known_gap_an_empty_crypt_pass_turns_field_encryption_off` |
| Stolen host disk or host backup, with `.env` | Password hashes | Every encrypted field: the key is in `.env` beside the data (`runtime/db/embedded` is on the same disk). One pass guess costs 10 PBKDF2 iterations. With `webapp-list/webapp[webroot]/session-config@cookie-key` and the session table, a live session's cookie can be forged | `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt`, `only_the_same_pass_salt_and_iterations_decrypt` |
| Database write access, no key (tamper) | A random change to a stored value usually fails at padding or UTF-8 | A change is not detected: with no MAC, a writer sets the first block of a value to text of their choice, or cuts a value to its tail. A writer stores a value sealed under the public default pass, and the server reads it as valid. Text with no colon that is not hex reads as `_DECRYPT_FAILED_`, logged, unless `crypt-read-plain` is on for a migration (`field_rw.rs`, `decrypt_fields`, SEC-DAT-10) | `known_gap_a_changed_ciphertext_is_not_detected`, `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid` |
| Host root, or a reader of process memory | Nothing | Every secret: the pass, the session key and decrypted values are in memory, and no buffer is zeroed (no `zeroize` or `secrecy` in `crates/`) | None: there is no control to test |
| Administrator of the instance | The administration connection and provider services treat a secret as write-only: a read answers only whether it is set, and their audit row of a change holds no value ([Administration services](Attack-Surface-Admin.md#administration-services)). Entity REST and Service REST results omit the five `UserAccount` password fields (`currentPassword`, `resetPassword`, `passwordSalt`, `passwordHashType`, `passwordBase64`), also where a view alias renames one. Entity REST refuses a find of an identity-admin, secret-config or audit entity, and of a view over one (`UserAuthcFactor`, `EmailServer`, `SystemMessageRemote` and `DatabaseConnection` among them). Service REST refuses a read of an identity-admin or audit entity. The Tools data grid shows an encrypted field as `********`. No entity REST or Service REST result holds an encrypted field's value: an administrator reads whether it is set (`<field>IsSet`), in nested records too, and a related entity the restricted rule refuses is left out of dependents and masters (plans/Security-Review.md SEC-DAT-03, SEC-RTE-06, fixed). The tools `email/servers` resource answers only servers no mail account owns. An export or snapshot without `ADMIN_BACKUP_DOWNLOAD` holds no password or encrypted field. `System.getProperty` answers a script no secret property | The entity audit log holds the old and new ciphertext of the seven encrypted fields that have `enable-audit-log`. The entity facade decrypts for any caller with read access, so code that runs in the server (a service, a script, an ETL expression) reads plaintext. An export or snapshot by a holder of `ADMIN_BACKUP_DOWNLOAD` (`ADMIN` by the seed) holds every field decrypted, and `get#EntitySyncData` every encrypted field. `ADMIN_ADV` can open a terminal on the host | `AdminConnectionServices` "an endpoint change without the secret clears it …" (`runtime/components/base/framework/test/AdminConnectionServices.spec.js`), `SecurityRest` "MOQUI_API … can list users and the JSON omits password fields", "… omits UserAccount password fields" (`runtime/components/base/framework/test/client/SecurityRest.spec.js`; [Proof-Tests](Proof-Tests.md#catalog)); `entity_resources_answer_encrypted_fields_as_set_and_read_a_page_at_most` (`rest_tests.rs`), `generic_reads_leave_out_restricted_related_rows_and_answer_encrypted_fields_as_set` (moqui-entity) |
| Compromised client device | The device key is in the OS key store when there is one, else in a file only its user can read | Anyone who runs as that user reads the key and signs in as the device | `a_file_key_is_the_owners_alone_and_can_be_forgotten` (`crates/moqui-client/src/keys_tests.rs`), `the_variable_chooses_and_a_development_run_keeps_its_keys_in_a_file` (`keystore.rs`) |
| Network attacker | TLS to the client (rustls) and to an external database (the datasource's `sslmode`). The session cookie is HttpOnly, SameSite=Lax, Secure over https, and a bad MAC is no session | Plain HTTP when no TLS listener or proxy is in front | `a_bad_mac_is_an_unknown_session`, `SecurityHeaders` (cookie flags) |

### Controls that hold

- Field encryption with the wrong pass, salt or iteration count does not give the plaintext (`wrong_or_empty_pass_fails`, `only_the_same_pass_salt_and_iterations_decrypt`).
- An empty pass is refused by `encrypt` and `decrypt`, and a pass that is not printable ASCII is refused (`crates/moqui-entity/src/entity/crypto.rs:104`; `wrong_or_empty_pass_fails`, `a_pass_phrase_that_is_not_ascii_is_an_error`).
- `CryptConfig`'s `Debug` does not show the pass (`crypto.rs:50`; `debug_leaves_out_the_pass`).
- Every bound value of an encrypted field in a query is encrypted before SQL (`every_bound_value_of_an_encrypted_field_is_sealed`).
- Startup replaces each secret `CHANGEME` default with 32 CSPRNG bytes, a new value each time, written to `.env` with mode `0600` (`crates/moqui-core/src/context/factory.rs:1823`, `:1974`; `crates/moqui-lib/src/env.rs:147`). A `default-property` is a secret by `is-secret="true"`, or by `pass`, `pw` or `key` in its name (`factory.rs:1954`). Tests: `generated_secrets_are_32_random_bytes_and_secret_names_are_known`, `generated_secret_is_stable_and_unwritable_env_fails`, `update_dotenv_file_replaces_in_place_appends_and_removes`.
- Password hashes are argon2id, and a legacy hash becomes argon2id at the next sign-in (`a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string`).

### Known gaps

| Gap | Where | Test |
| --- | --- | --- |
| Constant IV: encryption is deterministic. Equal plaintexts give equal ciphertext, and a shared 16-byte prefix gives a shared first block. Lookup by value (`CalendarShare.publishToken`) depends on this | `crypto.rs:28`, `:61` | `known_gap_equal_plaintexts_give_equal_ciphertext`, `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block` |
| No MAC or AEAD: a changed value is not detected, and a writer controls the first block through the `<iv>:<ct>` form | `crypto.rs` `decode_stored` | `known_gap_a_changed_ciphertext_is_not_detected` |
| AES-128 from PBKDF2-HMAC-SHA256 with 10 iterations and a fixed salt from the conf (`20201202`) | `crypto.rs:36`, `MoquiDefaultConf.xml` `entity-facade` | `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt` |
| PBEWithMD5AndDES still decrypts, with the current pass and with the old pass | `crypto.rs:35`, `decrypt-alt` in `MoquiDefaultConf.xml` | `known_gap_des_ciphertext_still_decrypts_through_the_alternates` |
| `entity-facade@crypt-pass-old` keeps the public sentinel and is a `decrypt-alt`, so a value sealed under `MoquiDefaultPassword:CHANGEME` is read as valid | `factory.rs:1959`, `MoquiDefaultConf.xml` | `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid` |
| An empty `crypt-pass` turns field encryption off with no error | `crates/moqui-entity/src/entity/facade.rs:478` | `known_gap_an_empty_crypt_pass_turns_field_encryption_off` |
| The key is in `runtime/.env`, on the same host and often in the same backup as the database | [Secrets](#secrets) | None: deployment |
| One key for all fields, with no key id or version in the stored value; changing the pass needs the `_old` alternate and a rewrite of every row | `crypto.rs:61` | `known_gap_equal_plaintexts_give_equal_ciphertext` shows the format has no key id |
| `webapp-list/webapp[webroot]/session-config@cookie-key` has no `_old` key: a new key ends every session and every value signed with a derived key | `session.rs:3058` | `known_gap_a_new_session_cookie_key_ends_every_session_cookie` |
| Stored text that is not ciphertext (no colon, not hex) is read as the value of an encrypted field while `crypt-read-plain` is on for a migration | `field_rw.rs` `decrypt_fields` | `plain_text_in_an_encrypted_field_is_read_only_for_a_migration` |
| An encrypted field leaves the facade decrypted in a full export or snapshot and in EntitySync data | [Where a decrypted value goes](#where-a-decrypted-value-goes) | None |
| Bank routing, tax and carrier account numbers are stored in clear | [Encrypted fields](#encrypted-fields) | None |
| No memory zeroing of passes, keys or decrypted values | `crates/` | None: absence |
| The server sees every plaintext: there is no client-side (zero-knowledge) encryption | Entity facade | None: absence |

### Inspiration

Two designs show what MR Moqui does not do yet.

**HashiCorp Vault** (server-side secrets):

- A barrier encrypts all storage with AES-256-GCM. A random nonce on each write gives different ciphertext for equal values, and the GCM tag detects a change.
- The keyring is versioned. `vault operator rotate` adds a key term; old terms still decrypt, and new writes use the new term.
- A root key encrypts the keyring. Shamir's secret sharing (k of n unseal keys) or a KMS or HSM (auto-unseal) seals the root key. The key is not on the disk beside the data.
- Leases and dynamic secrets: Vault makes a short-lived credential on request and revokes it when its lease ends.
- Transit encrypts as a service: the application never holds the key. A rewrap moves ciphertext to the newest key version without exposing the plaintext. Convergent encryption is an opt-in for lookup by value.
- Response wrapping hands a secret over through a single-use token.
- Audit devices log each request with secret values as HMAC-SHA256. A request fails when no audit device can write it (fail closed).
- `mlock` keeps secret memory out of swap.

**Bitwarden** (client-side, zero-knowledge):

- The client derives a master key from the master password with PBKDF2-SHA256 (600,000 iterations by default) or Argon2id.
- HKDF stretches the master key into an encryption key and a MAC key.
- A random user key encrypts the vault. The stretched master key wraps the user key, so a password change rewraps one key, not every item.
- Each field is AES-256-CBC with HMAC-SHA256, encrypt-then-MAC, with a random IV.
- An organization key is shared by encrypting it to each member's RSA public key.
- The client sends only a hash of the master key, and the server hashes it again with a random salt. The server never sees the master password or a plaintext item.

**What MR Moqui can take from them** (also in [Remaining-Work](../../plans/Remaining-Work.md#sessions-notifications-and-security)):

| Improvement | From | Closes |
| --- | --- | --- |
| A v2 stored format: AEAD (AES-256-GCM or XChaCha20-Poly1305), a random nonce, and a key id, such as `v2:<kid>:<nonce>:<ct+tag>`. Decrypt keeps the Java forms, decrypt-only | Vault barrier, Bitwarden EncString | Determinism, no MAC, AES-128, weak PBE |
| A blind index for a lookup-by-value field: an HMAC of the value under its own key, in a separate column | Vault convergent Transit | Lookup by value without deterministic ciphertext |
| Envelope keys: random data keys wrapped by a root key from a KMS, OpenBao or Vault Transit, a TPM, or `systemd-creds`, never from `.env` | Vault seal | The key beside the data |
| A versioned keyring with rotation, and a rewrap job that rewrites old rows to the newest key | Vault rotate and rewrap | One unversioned key; `webapp-list/webapp[webroot]/session-config@cookie-key` with no `_old` |
| Skip a `decrypt-alt` whose pass is the public `_old` sentinel; PBEWithMD5AndDES decrypt-only, and whether to drop it decided after the first release | Vault keyring terms | DES; planted values under the public pass |
| Refuse to start when an entity has `encrypt="true"` fields and `crypt-pass` is empty | Vault seal (no storage until unsealed) | Encryption off with no error |
| `zeroize` (and `secrecy` types) for passes, keys and decrypted values; `mlock` where available | Vault `mlock` | Secrets left in memory |
| An optional client-side vault for personal secrets: a user key wrapped by a key from the user's password (Argon2id), items AEAD-encrypted in the client | Bitwarden | The server sees plaintext |

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- The search index: who may query it, with what filters, and whether `index#DataDocuments` drops any field of a document (`crates/moqui-core/src/search/`) was not read.
- Whether Tools `Data/Browse` AutoFind and AutoEdit screens and the entity auto forms show an encrypted field's value was not read.
- Whether a database error's text (`postgres <context>: <message>`) reaches an HTTP client was not traced.
- The entity cache's size limits and what a `distributed` cache type means for entity values (`cache.rs`) were not read.
- Which record's filters apply to a find below an inherited authorization with several matching records (`ec.top_authz_info()`, `user/authz.rs`) was not read.
- `filterOrgIds` unset under `MANTLE_USER_ORG`: whether the evaluation fails (closed) or gives null (`IS NULL`) was not traced.
- A data document expression field (`pq-expression`) is evaluated on read, without the encrypted and password fields: which engine runs it and who may write such a `DataDocumentField` were not traced.
- Whether each consumer of a generated secret (`push/web@vapid-key` and the others) takes 32 random bytes in base64 as its key was not traced.
- Whether any System screen other than the configuration view lists properties or the environment unmasked was not read.
- The hashed credential fields (`tokenHash`, `credentialHash`, `passwordHash` and the like, in the base framework and mr-mantle) were not listed here: the hash function of each was not read.
- `ServiceJobRun.parameters` and queued-call JSON hold whatever the caller passed: whether a secret parameter is masked there was not traced.
- `Cargo.lock` was not checked against the advisory database.
