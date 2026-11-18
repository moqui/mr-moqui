# Conf Paths: settings by XML path, overridden from the environment

Status 2026-11-18: phases 0 to 2 built (the path module, the conf schema, the overrides with `getConfValue`, `setConf`,
the conf view's names and `moqui conf env`), 3a (entity facade, datasources, database list), 3b (web, logging, caches,
telemetry, reports, PDF, component deploy), 3c (SSH, terminal, git, git CI, SMTP, mail delivery, the mail client) and
3d (search, LLM, A2A, MCP, gateway): no conf attribute of the default conf refers to a property now. Phase 4 has the
instance (§6.1), acme (§6.24), client-sync (§6.13) and chat (§6.14) elements; its other areas and phases 5 to 8 to do ([§7](#7-phases)). Until phase 7 a name table may hold both kinds of name:
`conf_env::setting_or_property` reads a selector (it has an `@`) from the conf and anything else as a property.

The conf was configured through about 330 flat `default-property` entries (279 in `MoquiDefaultConf.xml`, the rest in
`runtime/conf/**` and component `MoquiConf.xml` files). Attributes referred to them by `${name}` and `${a ?: b}`,
readers expanded those when they read the attribute, boot set each as a process property unless `-D`, the environment
or `runtime/.env` had it, and code read them by name (226 `get_prop_or_env` calls, about 60 `System.getProperty` reads
in scripts, and name tables). 123 of them fed attributes; the other 209 had no element at all (XMPP, SIP, TURN, calls,
chat, channels, ACME, cluster, edge, push and more). Two configuration systems ran side by side, and the flat names
hid the structure the XML already has.

This plan makes it one system: every setting is an attribute of a conf element, with its default written literally in
the conf, and any attribute can be overridden or added from the environment by a name built from its XML path. The
same path in selector form is used in docs, scripts, live settings and the conf view. It departs from Moqui Java on
purpose.

Sections: [1 names](#1-paths-and-names) · [2 schema](#2-the-conf-schema) · [3 overrides](#3-applying-overrides) ·
[4 readers](#4-readers-scripts-and-display) · [5 rules for the move](#5-rules-for-each-move) ·
[6 mapping](#6-mapping-old-names-to-attributes) · [7 phases](#7-phases) · [8 decisions](#8-decisions)

## 1. Paths and names

**Selector form**, for docs, scripts (`ec.factory.getConfValue`), live settings and reports:
`webapp-list/webapp[webroot]@http-port`, `telemetry/artifact[AT_SERVICE]@spans`,
`webapp-list/webapp[webroot]/response-header[request,X-Frame-Options]@value`. A keyed element takes its key values in
brackets, comma separated, in the order of its key attributes. This is the live-settings key form already
(`live_settings/catalog.rs`), and the conf reload's unit paths take it too.

**Environment form**: `moqui_conf__` then each element from the root, `__` between segments; a keyed element's key
values follow it, one segment each; the last segment is the attribute.

| Selector | Environment name |
|---|---|
| `webapp-list/webapp[webroot]@http-port` | `moqui_conf__webapp_list__webapp__WEBROOT__http_port` |
| `entity-facade/datasource[transactional]@host` | `moqui_conf__entity_facade__datasource__TRANSACTIONAL__host` |
| `entity-facade/datasource[transactional#clone1]@host` | `moqui_conf__entity_facade__datasource__TRANSACTIONAL_CLONE1__host` |
| `entity-facade@crypt-pass` | `moqui_conf__entity_facade__crypt_pass` |
| `ssh-server@port` | `moqui_conf__ssh_server__port` |
| `instance@purpose` | `moqui_conf__instance__purpose` |
| `llm-facade/endpoint[anthropic]@api-key` | `moqui_conf__llm_facade__endpoint__ANTHROPIC__api_key` |
| `telemetry/artifact[AT_SERVICE]@spans` | `moqui_conf__telemetry__artifact__AT_SERVICE__spans` |

- **Matching** ignores case, in the prefix and every segment, and ignores every character but letters and digits
  within a segment: `http-port`, `http_port`, `HTTP_PORT` and `httpPort` are one name, and so are
  `transactional#clone1` and `TRANSACTIONAL_CLONE1`.
- **The canonical spelling** (docs, `.env` lines the server writes, the conf view, `moqui conf env`): element and
  attribute segments in lower case as in the XML, `-` written `_`; key values in capitals, so they stand out, each run
  of other characters written `_`.
- `MOQUI_CONF`, which names the runtime conf file, is unchanged and is not one of these names.
- **Additions.** A missing single element is created; a missing keyed element is created with its key exactly as the
  name writes it (`ANALYTICS` makes the key `ANALYTICS`: a lower-case key is written in lower case, or the element goes
  in a conf file); a missing attribute is added in lower case with `-`. What a name cannot spell (camelCase,
  punctuation in a key) can be matched but not created.
- **Refused**, each with an ERROR naming the variable, which is then skipped: a name whose keyed children are two that
  normalize alike; a sequence element (`load-data`, the `component-list` entries) that has no key to pick one; text
  content; and of two variables in one source that name the same attribute, the second in sorted order.
- **Logged**: each override at INFO, as "sets" or "adds", never with a secret's value; an attribute added to an
  element the files have is a WARN (most often a misspelled name).
- **Precedence**: a process property (`-D`), then the OS environment, then `runtime/.env`. An override wins over every
  conf file: the default conf, the components' and the runtime conf. An empty value is not applied (as before), so an
  override cannot clear a value; a conf file can.

The parser, the formatter, normalization and the walk over an `MNode` tree are in `moqui-lib`
(`node/path.rs`), given the key attributes of each element by the caller: the library knows nothing of the conf.

## 2. The conf schema

`crates/moqui-core/src/context/conf_schema.rs` (the conf reload's catalog, moved and completed) is the tree of the
conf's elements: how each is told apart (single, keyed by attributes, or a sequence), its reload class and the classes
of its attributes, and now the marks on its attributes:

- `secret`: never logged, masked in the conf view and reload reports, null to `getConfValue`, refused to
  `addPropertyHeader`;
- `generate`: a value ending in `CHANGEME` at boot is replaced by 32 random bytes in base64, written to `runtime/.env`
  under the attribute's canonical name;
- the pair and check rules kept from the property code: the cluster TLS certificate and key are generated, or must be
  set, together; the session cookie key is checked (32 bytes in base64, not `CHANGEME`).

The merge in `conf.rs` takes its keys from the schema. A top-level element the schema does not know (a component's
own, [§6.27](#627-component-elements)) merges as a settings element: its attributes override, its children merge by
`name` or `id` when they have one and are appended when not; its env path is read the same way. The XSD ends
`moqui-conf` with a lax `xs:any` for those.

## 3. Applying overrides

- The `.env` names are read from the runtime's own `runtime/.env` at each merge, so what the server wrote there since
  boot is seen, and a factory never takes another runtime's (factories of tests share one process's tables). The
  file is also loaded into the dotenv table before the first merge, for the properties. `MOQUI_CONF` still ignores it.
- `conf_env::apply_overrides` runs at the end of `load_merged_conf`, `merge_component_confs` and `merge_files`, so
  component discovery, the port check, the SSH listen target and `before_init` see the overrides, and so does each
  re-merge of a conf reload. It writes nothing. Its result (selector, env name, source, secret) is kept with the conf
  for the conf view, the startup report and `moqui conf env`.
- `conf_env::generate_secrets` runs at boot, and on a reload that brings a new `generate` attribute.
- Runtime writers (edge pairing and unpairing, the Edge Apply wizard, the DKIM key made on first use, the testkit) use
  `ExecutionContextFactory::set_conf_value(selector, value, persist)` and `clear_conf_value`: the factory keeps its
  set of runtime overrides on top of the environment's, builds the live conf again (a new conf generation, so
  conf-derived state is rebuilt) and with `persist` updates `runtime/.env` under the canonical name.
- The conf reload no longer has a properties group: a re-merge applies the same overrides, and what used to be a fixed
  property is a restart class on its attribute.
- A conf attribute that still holds `${` (outside `actions` and script text) refuses the boot, naming the attribute:
  `${}` is no longer expanded, and a stale `${entity_ds_crypt_pass}` must never become a crypt pass.

## 4. Readers, scripts and display

- Rust reads attributes through the factory's helpers (`conf_setting`, `conf_flag`, `conf_number`, `webapp_setting`)
  or once into a settings struct at start; no reader expands `${}`.
- Scripts: `ec.factory.getConfValue(selector)` answers the attribute's value, or null when it is not set or is a
  secret; `ec.factory.isConfValueSet(selector)` says whether one is set, a secret too, without its value. `System.getProperty` refuses every `moqui_conf__` name; process properties and the environment are as before.
  `RestClient.addPropertyHeader` takes a conf selector for a credential (`pass@client-secret`).
- The testkit: `setConf(selector, value)` and `withConf(selector, value, fn)`, beside `setProperty` and `withProperty`.
- System › Conf shows each attribute's environment name, and for an overridden one the name and source that set it;
  secrets are masked by the schema mark and the name rule.
- `moqui conf env` lists every attribute of the merged conf with its environment name, value (secrets masked) and
  source. `./moq -D` completes from it.
- Runtime state the server sets for scripts stays as process properties, not conf: `webapp_notification_endpoint`,
  `webapp_terminal_endpoint`, the resolved `xmpp_rooms_domain`, `xmpp_upload_url` and `xmpp_upload_max_bytes`, and
  `cluster_ssh_host_key_fingerprint` and `cluster_ssh_address`.

## 5. Rules for each move

- The default goes on its attribute in `MoquiDefaultConf.xml`, written literally; the XSD documents the attribute.
- The reader reads the conf; the `default-property` and its `${}` references go.
- `runtime/conf/**` files set the attribute where they set the property.
- A `${a ?: b}` becomes a literal default, or a fallback in code that the XSD documents (the content datasource takes
  the transactional's server and credentials; `smtp-server@authserv-id` empty is `@hostname`).
- One property that fed several attributes becomes one attribute, and the others take it in code
  (`entity-facade@crypt-pass` for the `decrypt-alt` entries; `llm-facade@default-model` for the `bind` entries with no
  model; `webapp@http-host` for an empty `@https-host`).
- Names built at runtime become keyed elements (`pass-authority/client[forum]@sites`,
  `email-client/oauth-provider[google]@client-id`).
- No migration: an instance from before this change regenerates its secrets (`./moq clean-db`, `./moq load`), pairs
  its edge links again and gets new SSH host keys.

## 6. Mapping old names to attributes

The working record of the move. Each line is old name → selector; `(gen)` marks a generated secret, `(secret)` a
secret that is not generated. Defaults are those of the old `default-property` unless the line says otherwise.

### 6.1 Instance

New top-level `instance`.

| Old | Selector |
|---|---|
| `instance_purpose` | `instance@purpose` (production; the dev conf dev) |
| `instance_id` | `instance@id` |
| `node_role` | `instance@node-role` |
| `default_locale` | `instance@default-locale` |
| `default_time_zone` | `instance@default-time-zone` |
| `database_time_zone` | `entity-facade@database-time-zone` (UTC) |
| `initial_admin_enabled` | `user-facade/login@initial-admin-enabled` |
| `system_home_dashboard` | `screen-facade@system-home-dashboard` |

### 6.2 Logging

| Old | Selector |
|---|---|
| `moqui_log_level` | `logging@level` (read before boot from `-D` and the OS environment, as before) |
| `moqui_log_color` | `logging@color` |
| `moqui_log_dir` | `logging@dir` |

### 6.3 Web

| Old | Selector |
|---|---|
| `webapp_http_host` | `webapp-list/webapp[webroot]@http-host` (an empty `@https-host` takes it) |
| `webapp_http_port` | `webapp-list/webapp[webroot]@http-port` |
| `webapp_https_port` | `webapp-list/webapp[webroot]@https-port` |
| `webapp_https_enabled` | `webapp-list/webapp[webroot]@https-enabled` |
| `webapp_https_listen_port` | `webapp-list/webapp[webroot]@https-listen-port` |
| `webapp_http_bind` | `webapp-list/webapp[webroot]@http-bind` |
| `webapp_allowed_hosts` | `webapp-list/webapp[webroot]@allowed-hosts` |
| `webapp_http2` | `webapp-list/webapp[webroot]@http2` |
| `webapp_allow_origins` | `webapp-list/webapp[webroot]@allow-origins` |
| `webapp_handle_cors` | `webapp-list/webapp[webroot]@handle-cors` |
| `webapp_status_ips` | `webapp-list/webapp[webroot]@status-ips` |
| `webapp_require_session_token` | `webapp-list/webapp[webroot]@require-session-token` |
| `webapp_upload_executable_allow` | `webapp-list/webapp[webroot]@upload-executable-allow` |
| `webapp_client_ip_header` | `webapp-list/webapp[webroot]@client-ip-header` |
| `webapp_trusted_proxies` | `webapp-list/webapp[webroot]@trusted-proxies` |
| `webapp_dev_marker_color` | `webapp-list/webapp[webroot]@dev-marker-color` |
| `webapp_qvt_static_only` (undeclared) | `webapp-list/webapp[webroot]@qvt-static-only` |
| `session_cookie_key` | `webapp-list/webapp[webroot]/session-config@cookie-key` (gen) |
| `webapp_web_protection` | `webapp-list/webapp[webroot]/web-protection@enabled` |
| `webapp_max_connections`, `webapp_header_timeout`, `webapp_write_timeout`, `webapp_shutdown_timeout`, `webapp_http2_max_streams` | `.../web-protection/connections@max`, `@header-timeout`, `@write-timeout`, `@shutdown-timeout`, `@http2-max-streams` |
| `webapp_max_body`, `webapp_max_upload`, `webapp_anonymous_upload_paths`, `webapp_body_timeout`, `webapp_upload_idle_timeout`, `webapp_request_timeout` | `.../web-protection/requests@max-body`, `@max-upload`, `@anonymous-upload-paths`, `@body-timeout`, `@upload-idle-timeout`, `@request-timeout` |
| `webapp_max_spool` | `.../web-protection/requests@max-spool` |
| `elasticsearch_url`, `_user`, `_password` in the SearchProxy servlet | its `init-param` values left empty take `search-facade/cluster` |
| `kibana_host`, `kibana_port` | the DashboardProxy `init-param[proxyTo]@value`, literally `http://127.0.0.1:5601` |

### 6.4 Caches and transactions

| Old | Selector |
|---|---|
| `entity_tx_cache` | `transaction-facade@use-transaction-cache` |
| `entity_lock_track` | `transaction-facade@use-lock-track` |
| `entity_statement_timeout` | `transaction-facade@use-statement-timeout` |
| `screen_tx_cache` | `screen-facade@tx-cache` |
| `service_tx_cache` | `service-facade@tx-cache` |
| `script_module_cache` | `resource-facade@script-module-cache` |
| `scheduled_job_check_time` | `service-facade@scheduled-job-check-time` |

### 6.5 Data loading

| Old | Selector |
|---|---|
| `entity_empty_db_load` | `tools@empty-db-load` |
| `entity_on_start_load_types` | `tools@on-start-load-types` |
| `entity_on_start_load_components` | `tools@on-start-load-components` |
| `entity_add_missing_runtime`, `entity_add_missing_startup` | `entity-facade/datasource[...]@runtime-add-missing`, `@startup-add-missing`, on each datasource |

### 6.6 Entity facade and datasources

The connection moves onto `datasource` attributes: `host`, `port`, `database`, `user`, `password`, `sslmode`,
`sslrootcert`. A datasource's own literal `inline-jdbc/xa-properties` is still read after them (a conf from Java), and
the `database-list` entries keep their dialect settings but lose their `${entity_ds_*}` connection templates.
`database-conf-name` empty, `h2` or `embedded` is the embedded cluster.

| Old | Selector |
|---|---|
| `entity_sql_name_style` | `entity-facade@sql-name-style` (java) |
| `entity_ds_crypt_pass` | `entity-facade@crypt-pass` (gen); a `decrypt-alt` with no `crypt-pass` takes it |
| `entity_ds_crypt_pass_old` | `entity-facade@crypt-pass-old` (secret, default `MoquiDefaultPassword:CHANGEME`, never generated); a `decrypt-alt` with `old="true"` takes it |
| `entity_ds_crypt_read_plain` | `entity-facade@crypt-read-plain` (false) |
| `entity_ds_db_conf` | `entity-facade/datasource[transactional]@database-conf-name` |
| `entity_ds_host`, `_port`, `_database`, `_schema`, `_user`, `_password` (secret), `_sslmode`, `_sslrootcert` | `entity-facade/datasource[transactional]@host`, `@port`, `@database`, `@schema-name`, `@user`, `@password`, `@sslmode`, `@sslrootcert` |
| `entity_ds_url`, `moqui_runtime` | gone: the H2 URL; H2 is not a driver here |
| `content_ds_db_conf` | `entity-facade/datasource[content]@database-conf-name` (empty shares the transactional database) |
| `content_ds_host`, `_port`, `_database`, `_schema`, `_user`, `_password` (secret), `_sslmode`, `_sslrootcert` | `entity-facade/datasource[content]@...` as for transactional; empty takes the transactional datasource's, but `@database` (moqui_content) |
| `entity_ds_c1_*` | `entity-facade/datasource[transactional#clone1]@...` (`@disabled` true, `@database-conf-name` mysql, `@port` 3306) |
| `ORIENTDB_HOME` | gone with the Java OrientDB example |

### 6.7 Search

| Old | Selector |
|---|---|
| `search_backend` | `search-facade@backend` |
| `elasticsearch_url`, `_user`, `_password` (secret), `_index_prefix` | `search-facade/cluster@url`, `@user`, `@password`, `@index-prefix` |
| `opensearch_run` | `search-facade/cluster@run` |

### 6.8 Telemetry, reports, PDF, component deploy

| Old | Selector |
|---|---|
| `telemetry_enabled`, `telemetry_sample_ratio` | `telemetry@enabled`, `@sample-ratio` |
| `report_save_roots` | `report@save-roots` |
| `moqui_chromium_path` | `screen-facade/pdf@chromium-path` |
| `component_deploy_enabled` | `component-deploy@enabled` |
| `component_deploy_attestation_key` | `component-deploy@attestation-secret` (secret) |
| `component_deploy_public_urls` | `component-deploy@public-urls` |

### 6.9 SSH and terminal

| Old | Selector |
|---|---|
| `ssh_port`, `ssh_host`, `ssh_public_host`, `ssh_public_port` | `ssh-server@port`, `@host`, `@public-host`, `@public-port` |
| `ssh_host_key` | `ssh-server@host-key` (gen) |
| `terminal_command_line`, `terminal_host_sessions`, `terminal_remote` | `terminal@command-line`, `@host-sessions`, `@remote` |

### 6.10 Git

| Old | Selector |
|---|---|
| `git_public_url`, `git_shared_storage` | `git-server@public-url`, `@shared-storage` |
| `git_lfs_token_key` | `git-server@lfs-token-key` (gen) |
| `git_lfs_token_key_previous` | `git-server@lfs-token-key-previous` (secret) |
| `git_ci_enabled`, `git_ci_server_url` | `git-ci@enabled`, `@server-url` |

### 6.11 SMTP listener and mail delivery

| Old | Selector |
|---|---|
| `smtp_listen_port`, `smtp_listen_host`, `smtp_allowed_addresses`, `smtp_hostname`, `smtp_authserv_id` | `smtp-server@listen-port`, `@listen-host`, `@allowed-addresses`, `@hostname`, `@authserv-id` (empty on an edge node or in internet mode takes `@hostname`) |
| `smtp_tls_cert_file`, `smtp_tls_key_file` | `smtp-server@tls-cert-file`, `@tls-key-file` |
| `smtp_internet`, `mail_rspamd_url`, `mail_arc_trusted_sealers` | `smtp-server@internet`, `@rspamd-url`, `@arc-trusted-sealers` |
| `edge_mail_mta`, `edge_mail_relay_host`, `edge_mail_relay_port` | new `mail-delivery@mta`, `@relay-host`, `@relay-port` |
| `mail_delivery_retry_seconds`, `mail_delivery_days`, `mail_mx_require_tls`, `mail_mx_override`, `mail_dns_offline` | `mail-delivery@retry-seconds`, `@days`, `@mx-require-tls`, `@mx-override`, `@dns-offline` |
| `mail_dkim_key` | `mail-delivery/dkim@key` (secret; made on first use, persisted) |
| `mail_dkim_selector`, `mail_dkim_domains` | `mail-delivery/dkim@selector`, `@domains` |
| `mail_capture_sends` (undeclared; specs) | `email-client@capture-sends` |

### 6.12 Mail client

| Old | Selector |
|---|---|
| `email_oauth_redirect_uri` | `email-client@oauth-redirect-uri` |
| `email_oauth_google_client_id`, `_client_secret` (secret) | `email-client/oauth-provider[google]@client-id`, `@client-secret` |
| `email_oauth_microsoft_client_id`, `_client_secret` (secret), `_tenant` | `email-client/oauth-provider[microsoft]@client-id`, `@client-secret`, `@tenant` |

`EmailOAuthProvider.propertyPrefix` becomes `confProviderId`, the provider's conf id (`google`, `microsoft`).

### 6.13 Offline clients

New `client-sync`.

| Old | Selector |
|---|---|
| `mail_change_keep_days`, `client_request_keep_days` | `client-sync@mail-change-keep-days`, `@request-keep-days` |
| `client_upload_expire_hours`, `_min_chunk_bytes`, `_max_count`, `_max_mb` | `client-sync@upload-expire-hours`, `@upload-min-chunk-bytes`, `@upload-max-count`, `@upload-max-mb` |

### 6.14 Chat

New `chat`.

| Old | Selector |
|---|---|
| `chat_server_name`, `chat_to_device_retention_days`, `chat_device_active_days`, `chat_device_auto_expire_days`, `chat_spam_purge_days` | `chat@server-name`, `@to-device-retention-days`, `@device-active-days`, `@device-auto-expire-days`, `@spam-purge-days` |
| `chat_store_key_secret` | `chat@store-key-secret` (gen) |
| undeclared `chat_attachment_*`, `chat_changes_keep_days`, `chat_edit_window_minutes`, `chat_link_previews`, `chat_agent_entry_services` | `chat@...` by the same names without `chat_`, declared with today's code defaults |

### 6.15 Channels (SMS, MMS, bridges)

New `channels`.

| Old | Selector |
|---|---|
| `channels_webhooks_enabled`, `channels_webhook_max_bytes`, `channels_public_url`, `channels_media_minutes` | `channels@webhooks-enabled`, `@webhook-max-bytes`, `@public-url`, `@media-minutes` |
| `channels_media_key` | `channels@media-key` (gen) |
| `channels_edge_webhooks_per_minute`, `_per_client_minute`, `channels_edge_unverified_max_bytes`, `_unverified_waiting` | `channels/edge-limits@webhooks-per-minute`, `@webhooks-per-client-minute`, `@unverified-max-bytes`, `@unverified-waiting` |

### 6.16 Push

New `push`.

| Old | Selector |
|---|---|
| `web_push_vapid_key` | `push/web@vapid-key` (gen) |
| `web_push_subject` | `push/web@subject` |
| `fcm_service_account` (secret), `fcm_project_id`, `fcm_base_url` | `push/fcm@service-account`, `@project-id`, `@base-url` |

### 6.17 Load runner

| Old | Selector |
|---|---|
| `load_runner_targets`, `_max_runs`, `_max_users` | new `load-runner@targets`, `@max-runs`, `@max-users` |

### 6.18 Cluster

New `cluster-member` (`cluster` is already `search-facade`'s child).

| Old | Selector |
|---|---|
| `cluster_tls_cert`, `cluster_tls_key` | `cluster-member@tls-cert`, `@tls-key` (gen, as a pair) |
| `cluster_https_port`, `cluster_base_url`, `cluster_transport`, `cluster_protocol`, `cluster_bind_address` | `cluster-member@https-port`, `@base-url`, `@transport`, `@protocol`, `@bind-address` |

### 6.19 Edge node and link

`edge` (existing), and its new children `node` (the edge node a local node pairs with) and `front-node` (the front an
edge node pairs with).

| Old | Selector |
|---|---|
| `edge_link_key` | `edge@link-key` (gen) |
| `edge_link_url`, `edge_offline_page`, `edge_call_legs_per_pairing` | `edge@link-url`, `@offline-page`, `@call-legs-per-pairing` |
| `edge_ssh_tunnel_port`, `_host`, `edge_smtp_tunnel_host`, `edge_xmpp_port`, `_tls_port`, `_host` | `edge@ssh-tunnel-port`, `@ssh-tunnel-host`, `@smtp-tunnel-host`, `@xmpp-port`, `@xmpp-tls-port`, `@xmpp-host` |
| `edge_node_url`, `_key` (secret), `_pin`, `_pairing`, `_id` | `edge/node@url`, `@key`, `@pin`, `@pairing`, `@id` (written by pairing, persisted) |
| `edge_calls_enabled`, `edge_sip_enabled` | `edge/node@calls-enabled`, `@sip-enabled` (written by the wizard) |
| `front_node_*` | `edge/front-node@...` as for `node` |
| `pass_edge_client` | `edge/node@pass-client` |

### 6.20 XMPP

New `xmpp-server` with child `upload`.

| Old | Selector |
|---|---|
| `xmpp_domain`, `_rooms_domain`, `_upload_domain` | `xmpp-server@domain`, `@rooms-domain`, `@upload-domain` |
| `xmpp_port`, `_host`, `_tls_port`, `_tls_host`, `_s2s_port`, `_websocket_enabled` | `xmpp-server@port`, `@host`, `@tls-port`, `@tls-host`, `@s2s-port`, `@websocket-enabled` |
| `xmpp_resume_seconds`, `_fast_token_days`, `_pep_max_bytes`, `_block_max_jids`, `_push_max_registrations`, `_turn_enabled`, `_turn_per_hour` | `xmpp-server@resume-seconds`, `@fast-token-days`, `@pep-max-bytes`, `@block-max-jids`, `@push-max-registrations`, `@turn-enabled`, `@turn-per-hour` |
| `xmpp_scram_decoy_key` | `xmpp-server@scram-decoy-key` (gen) |
| `xmpp_upload_url`, `_max_bytes`, `_seconds`, `_unattached_hours`, undeclared `xmpp_upload_root` | `xmpp-server/upload@url`, `@max-bytes`, `@seconds`, `@unattached-hours`, `@root` |
| `xmpp_upload_secret` | `xmpp-server/upload@secret` (gen; `System.secretCodeHash` takes its selector) |

### 6.21 SIP

New `sip-server` with children `limits` and `registrar`.

| Old | Selector |
|---|---|
| `sip_port`, `_host`, `_tls_port`, `_tls_host`, `_tls_ca_file`, `_rtp_ports`, `_rtp_host`, `_public_address`, `_domain` | `sip-server@port`, `@host`, `@tls-port`, `@tls-host`, `@tls-ca-file`, `@rtp-ports`, `@rtp-host`, `@public-address`, `@domain` |
| `sip_request_rate`, `_request_burst`, `_max_transactions`, `_max_connections`, `_max_connections_per_source` | `sip-server/limits@request-rate`, `@request-burst`, `@max-transactions`, `@max-connections`, `@max-connections-per-source` |
| `sip_registrar_enabled`, `_min_expires`, `_max_expires`, `_default_expires`, `_nat_expires` | `sip-server/registrar@enabled`, `@min-expires`, `@max-expires`, `@default-expires`, `@nat-expires` |

### 6.22 Call media and TURN

New `media-server` with child `turn-server`.

| Old | Selector |
|---|---|
| `rtc_media_port`, `_host`, `_tcp_port`, `rtc_public_address`, undeclared `rtc_cascade_enabled` | `media-server@port`, `@host`, `@tcp-port`, `@public-address`, `@cascade-enabled` |
| `turn_port`, `_tls_port`, `_tls443_host`, `_host`, `_public_host`, `_relay_ports`, `_credential_hours`, `_external_urls`, `_max_allocations`, `_max_kbps`, `_allowed_peers` | `media-server/turn-server@port`, `@tls-port`, `@tls443-host`, `@host`, `@public-host`, `@relay-ports`, `@credential-hours`, `@external-urls`, `@max-allocations`, `@max-kbps`, `@allowed-peers` |
| `turn_auth_secret` | `media-server/turn-server@auth-secret` (gen) |

### 6.23 Calls and voicemail

New `calls` with children `queue`, `recording` and `captions`; new `voicemail`.

| Old | Selector |
|---|---|
| `call_ring_seconds`, `call_guest_joins_per_minute`, `call_transfer_ring_seconds`, `call_hold_music_location`, undeclared `call_direct_path`, `call_forward_ring_seconds`, `call_forward_max_hops` | `calls@ring-seconds`, `@guest-joins-per-minute`, `@transfer-ring-seconds`, `@hold-music-location`, `@direct-path`, `@forward-ring-seconds`, `@forward-max-hops` |
| `call_queue_tick_seconds`, `call_queue_prompt_location` | `calls/queue@tick-seconds`, `@prompt-location` |
| `call_recording_*` (9) | `calls/recording@enabled`, `@ack-required`, `@policy-revision`, `@retention-days`, `@queue-frames`, `@segment-mb`, `@segment-minutes`, `@max-mb`, `@prompt-audio` |
| `call_captions_*` (9) | `calls/captions@enabled`, `@provider`, `@chunk-seconds`, `@queue`, `@workers`, `@timeout-seconds`, `@language`, `@store`, `@prompt-audio` |
| `voicemail_*` (6) | `voicemail@max-seconds`, `@prompt-location`, `@trash-days`, `@transcribe`, `@transcribe-language`, `@pin-min-digits` |

### 6.24 ACME

| Old | Selector |
|---|---|
| `acme_directory_url`, `acme_directory_ca`, `acme_challenge_port` | new `acme@directory-url`, `@directory-ca`, `@challenge-port` |
| `acme_account_key` | `acme@account-key` (gen) |

### 6.25 LLM, A2A, MCP, gateway

| Old | Selector |
|---|---|
| `llm_openai_url`, `llm_openai_api_key` (secret) | `llm-facade/endpoint[default]@url`, `@api-key` |
| `llm_openai_model` | `llm-facade/model[default]@name` |
| `llm_anthropic_api_key`, `llm_xai_api_key`, `llm_gemini_api_key`, `llm_glm_api_key`, `llm_deepseek_api_key` | `llm-facade/endpoint[anthropic\|xai\|gemini\|glm\|deepseek]@api-key` (secret) |
| `llm_dev_model` | `llm-facade@default-model`, which a `bind` with no `model` takes |
| `llm_log_content` | `llm-facade/content-log@enabled` |
| `a2a_enabled`, `a2a_public_url`, `a2a_oidc_issuer`, `a2a_oidc_audience` | `llm-facade/a2a@enabled`, `@public-url`, `llm-facade/a2a/security@oidc-issuer`, `@oidc-audience` |
| undeclared `a2a_agent_name`, `_description`, `_version` | `llm-facade/a2a@agent-name`, `@agent-description`, `@agent-version` |
| `mcp_enabled`, `mcp_public_url`, `mcp_request_state_key` (secret) | `llm-facade/mcp@server-enabled`, `@public-url`, `@request-state-key` |
| `llm_gateway_enabled` | `llm-facade/gateway@enabled` |

### 6.26 PASS authority and backups

| Old | Selector |
|---|---|
| `pass_authority_grpc_enabled`, `pass_verification_template`, `pass_sso_flows` | new `pass-authority@grpc-enabled`, `@verification-template`, `@sso-flows` |
| `pass_client_<id>_sites`, `_callback`, `_secret` (secret) | `pass-authority/client[<id>]@sites`, `@callback`, `@secret` |
| `embedded_db_backup_scheduled`, `embedded_db_backup_dir` | new `database-backup@scheduled`, `@dir` |
| `moqui_devtools_projects` | `dev-reload@projects` |

### 6.27 Component elements

Each component declares its own top-level element, with its defaults, in its `MoquiConf.xml`; a component reading
another's setting gets null when that component is absent.

| Old | Selector |
|---|---|
| `comms_app_root` (basic-comms, mr-boss overrides) | `comms@app-root` |
| `demo_shared_user_ids`, `demo_test_login` | `demo@shared-user-ids`, `@test-login` |
| `dev_aide_shared`, `dev_aide_site_component` | `dev-aide@shared`, `@site-component` |
| `mantle_content_root`, `mantle_content_large_root` | `mantle@content-root`, `@content-large-root` |
| `pass_backend_url`, `pass_client_id`, `pass_client_secret` (secret), `pass_backend_transport`, `pass_backend_grpc_url`, `pass_backend_web_url`, `pass_backend_insecure`, `pass_notification_endpoint` | `pass@backend-url`, `@client-id`, `@client-secret`, `@backend-transport`, `@backend-grpc-url`, `@backend-web-url`, `@backend-insecure`, `@notification-endpoint` |
| `pass_media_root`, `pass_import_dir`, `pass_scanner_service`, `pass_classifier_service`, `pass_test_scanner`, `pass_route_domains`, `pass_sender_addresses`, `pass_mail_template_ids`, `pass_preview_frame_origins`, `pass_mbox_max_message_bytes`, `pass_authority_name` | `pass@media-root`, `@import-dir`, `@scanner-service`, `@classifier-service`, `@test-scanner`, `@route-domains`, `@sender-addresses`, `@mail-template-ids`, `@preview-frame-origins`, `@mbox-max-message-bytes`, `@authority-name` |
| `smithy_github_api`, `smithy_agreement_max_commits`, `mantle.calendar.email.from` (mr-mantle) | `mantle@smithy-github-api`, `@smithy-agreement-max-commits`, `@calendar-email-from` |

## 7. Phases

One commit each, on master.

0. This plan.
1. `node/path.rs` (selector and environment forms, normalization, the walk) with its tests; `conf_schema.rs` (the
   catalog moved and completed), and the merge taking its keys from it, with no change in behaviour.
2. The override mechanism ([§3](#3-applying-overrides)): `.env` read early, overrides applied in the merges, attribute
   secrets generated, `set_conf_value`, `getConfValue`, `setConf` and `withConf`, the conf view, `moqui conf env`,
   `-D` with these names. `default-property` still works beside it.
3. The settings that `${}` referenced, to literal attributes: (a) entity facade, datasources, database list;
   (b) web, telemetry, tools, logging, reports, PDF, component deploy; (c) SSH, terminal, Git, Git CI, SMTP, mail
   client; (d) search, LLM, A2A, MCP, gateway. Each moves the code that read the same names as properties, and the
   `runtime/conf` files. After (a) a dev runtime needs `./moq clean-db` and `./moq load`.
4. The settings that had no element, one commit per section of [§6](#6-mapping-old-names-to-attributes), with their
   script readers, specs and runtime writers.
5. Component elements: the generic merge rule and the framework's own (comms, demo, dev-aide); then a commit in each
   add-on repository (mr-pass, mr-mantle, mr-boss, mr-marble).
6. Scripts, packaging, the desktop app, CLI help and the rest of the test harness.
7. Removal: `default-property`, `${}` expansion in conf readers, and the boot refusal of `${` in conf attributes.
8. Docs: [Run-Deploy](../docs/Run-Deploy.md) "Properties" becomes "Settings and the environment"; the other guides,
   the security docs and the `runtime/conf` READMEs; the README's redesign list; [Parity-Diffs](Parity-Diffs.md);
   the names in [Sealed-Secrets](Sealed-Secrets.md) §2. Then this plan is archived.

## 8. Decisions

| Decision | Choice | Why |
|---|---|---|
| Environment names | `moqui_conf__` and the full path, a keyed element's key as the next segment | One name per attribute, never ambiguous, found with grep; a descendant shortcut would give an attribute several names |
| Case | Elements and attributes in lower case, key values in capitals; matching ignores case | The structure reads as the XML does and the chosen element stands out |
| Component settings | A component's own top-level element | Short hierarchical names, the same mechanism as the framework's, no second property system |
| Old names | Clean break: no migration of names, in code or `.env` | Before 0.1.0 the storage layout is not stable; a regenerated `.env` and database is the documented reset |
| Overrides | Applied into the merged tree | Readers before the factory and the ~40 that parse the conf text see final values unchanged; secrets are masked at each display through one rule |
| `${}` in conf attributes | No longer expanded; a leftover refuses the boot | Two mechanisms would remain otherwise; refusing protects data from a literal `${...}` crypt pass |
| Runtime state for scripts | Stays process properties | It is not configuration |
