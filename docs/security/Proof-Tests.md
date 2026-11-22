# Security proof tests

Named HTTP proofs for this repository. They record what is tested. They are not a pentest report and not a WAF claim.

- **Where:** [Attack-Surface.md](Attack-Surface.md)
- **Against a standard:** [proofs](proofs/README.md) files the framework's standing under each OWASP ASVS 5.0.0 requirement; the proofs here move there by requirement (plans/Proofs-Initial.md)
- **What:** [OWASP Top 10:2025](https://owasp.org/Top10/) (plus WSTG / ASVS L1 ideas, and API Top 10 for `/rest`)
- **Public tests** are the client specs under `runtime/components/base/framework/test/client/`. Each feature name starts with the OWASP id, and a comment names the Moqui Java pytest it ports.

The line-by-line map of all 143 Java HTTP tests is [Java-Port-Catalog.md](../../runtime/components/base/framework/test/client/Java-Port-Catalog.md): 125 ported, 0 pending, 18 not applicable. This file is the control catalog and the notes on what a green run does and does not prove. Application components (Mantle, Marble, IronStone) are out of scope, as are the Java Selenium smokes and the A2A interop script.

Java's Spock suite (`Security*.groovy`) was not ported. A row below that names only a Spock class is not a claim that a Rust unit test covers it.

## How to run

```sh
./moq test-client framework --serve
```

`--serve` starts `moqui test framework --spec ClientFixtures --db snapshot --serve` on a scratch database, under `conf/MoquiLoadTestConf.xml` (`test/client/suite.js`, and `test/acceptance/client/suite.js` for the acceptance specs). That file is the production conf with the screen, transition, and service tarpits off, so `LoadBasic` (an acceptance test) is not answered 429. `SecurityTarpit`, an acceptance test too (`./moq accept framework`), skips on that conf. A skip there is the expected result of `--serve`.

The cross-origin proofs (`V3WebFrontend` and `V4ApiWebService` in `test/proofs/`) fail when the server allows every origin (the dev conf, `moqui_conf__webapp_list__webapp__WEBROOT__allow_origins=*`), as they should. The client specs run on any conf.

Fixture users come from `runtime/components/base/framework/test/ClientFixtures.spec.js`. They are not demo seed. `sec.ip.v4`, `sec.ip.loop`, and `sec.mfa.totp` are rows only. Against a server `--serve` did not start:

```sh
moqui test framework --spec ClientFixtures --db existing
./moq test-client framework --server https://localhost:8443
```

Features that need a missing fixture user skip and name that command. The other proofs log in as `john.doe` / `moqui` (`--param user=`, `--param password=`). None sends that user a wrong password (the fourth failure locks the account, `max-failures="3"`), and none logs it out over REST (`UserAccount.hasLoggedOut` ends its other sessions). Wrong-password and lockout use `sec.lock.test`.

### Tarpit on the production conf

`runtime/conf/MoquiProductionConf.xml` leaves the tarpits on. Start a scratch server with the fixtures, then run only `SecurityTarpit` against it. Both features must be 429. A skip on this conf means the tarpit or the fixture did not engage.

```sh
MOQUI_CONF=conf/MoquiProductionConf.xml \
  ./moq test framework --spec ClientFixtures --db snapshot \
  --serve 127.0.0.1:0 --ready-file /tmp/moqui-tarpit.url
# when the ready file holds a URL:
./moq test-client framework --acceptance --spec SecurityTarpit --server "$(cat /tmp/moqui-tarpit.url)"
```

Stop the server when the client exits (its standard input is the terminal). The screen proof is the demo `ALL_SCREENS` row (120 hits in 60 seconds) on `GET /apps/tools/Home` as `john.doe`. The transition proof is `sec.tap.user`, 3 hits in 60 seconds, on `POST /apps/setPreference`.

## Catalog

A spec marked (proof) is one of the security proofs in `test/proofs/`, named for its OWASP ASVS chapter and filed by requirement ([security proofs](proofs/README.md)); the rest are the client specs of `test/client/`.

| Control | OWASP | Client spec |
| --- | --- | --- |
| Unauthenticated Tools is the login page | A01 | `SecurityAccess` |
| `/rest/e1` and `/rest/s1` without credentials are 401 | A01 | `SecurityAccess` |
| Removed `/rest/api_key`, `/rest/moquiSessionToken`, `/rest/userInfo` are 404 | A01 | `SecurityAccess` |
| `POST /rpc/json` is 404 | A01 | `SecurityAccess` (the path is gone; the JSON-RPC behaviors below are not applicable) |
| `getPreferences` without a login is 401 and has no preferences | A01 | `SecurityAccess` |
| A static path with `..` does not serve screen XML | A01 | `SecurityAccess` |
| A user with no artifact authz cannot open Tools | A01 | `SecurityAccess` |
| A VIEW-only user cannot open Tools' SQL | A01 | `V8Authorization` (proof) |
| Login `returnTo` off the site is refused; a path on the site is followed | A01 | `V3WebFrontend` (proof) |
| Login `Referer` off the site is refused | A01 | `V3WebFrontend` (proof) |
| Login `returnTo` with `Host: evil.example` does not redirect to that host | A01 | `V3WebFrontend` (proof) |
| CSP (`frame-ancestors`, `form-action`), `X-Frame-Options`, nosniff | A02 | `SecurityHeaders` |
| Session cookie is `moqui.session` (`__Host-moqui.session` over https), HttpOnly, SameSite=Lax | A02 | `V3WebFrontend` (proof) |
| Login HTML is not publicly cacheable | A02 | `V14DataProtection` (proof): `no-store`, never public |
| No `Origin` header means no `Access-Control-Allow-Origin` | A02 | `SecurityHeaders` |
| No H2 console | A02 | `SecurityHeaders` |
| Unknown origin, and its preflight, are 401 with no allow-origin | A02 | `V3WebFrontend` (proof) |
| The server's own origin gets allow-origin and credentials; its preflight is allowed (204) | A02 | `V3WebFrontend` (proof) |
| The server's host on another port or scheme is 401 | A02 | `V3WebFrontend` (proof) (no Java pytest; the origin compare includes scheme, host, and port) |
| REST login needs no session token; an unknown user is 401 with the shared text | A07 | `SecurityAuthn` |
| A POST with the session token runs; without it the preference is not stored | A01 | `V3WebFrontend` (proof) |
| Login issues a new session id | A07 | `SecurityAuthn` |
| `api_key` in the query, or a garbage `api_key` header, does not log in | A07 | `SecurityAuthn` |
| Basic on `/rest/e1`: unknown user 401, valid user is authenticated | A07 | `SecurityAuthn` |
| A REST 401 in JSON has no stack frames | A10 | `V16Logging` (proof) |
| `/notws` from a foreign origin is refused (logged-in session) | A01 | `V4ApiWebService` (proof) |
| An anonymous `/notws` upgrade is 401 | A01 | `V4ApiWebService` (proof) |
| The server's host on another port or scheme is refused for `/notws` | A01 | `V4ApiWebService` (proof) |
| Screen proofs: escaped extra path, FOP without login, login form action, VIEW-only service run, anonymous data import, `qzSign`, `GET` login with a password, `WEB-INF`, `GET /notws`, SQL query string, menu data, FOP `filename`, DataSnapshot download and parent segments, `/resources` put, Tools CSV, remote data import | A01, A03, A05 | `SecurityScreens`; the VIEW-only service run and DataSnapshot download are `V8Authorization` and the FOP `filename` `V5FileHandling` (proof) |
| `/email/NOTAREALID.png` is HTTP 200, `image/png` with no charset, inline, and the 1x1 tracking PNG | A01 | `SecurityScreens` |
| `/htmlr` and `/htmlr/...` are 404, not a PDF and not a stack trace | A10 | `SecurityScreens` (the renderer is not mounted) |
| `/status` from loopback is public JSON (no heap, datasources, or VmVendor) | A01 | `SecurityAccess` |
| `X-Forwarded-For` and `Forwarded` do not change that loopback result | A01 | `SecurityAccess` |
| `/search` and `/dashboard` without a login are 401; without `SearchRemote` the denial is not 401 | A01 | `SecurityAccess` |
| Basic and `api_key` on `/search`: no permission 403, bad password 401, garbage key does not authenticate, `SearchRemote` is past authorization | A01 | `SecurityAccess` |
| Entity REST and service REST authz, method override, CSRF on create, password fields omitted, identity fields, email server host, `UserLoginKey` | A01 | `SecurityRest`; the method override is `V8Authorization` and CSRF on create `V3WebFrontend` (proof) |
| `/rest/entity.json`: anonymous 401, no `REST_SCHEMA` 403, ADMIN 200 on one entity document | A01 | `V13Configuration` (proof) |
| `/rest/sm` HMAC missing or bad is 403; a valid signature needs no login; timestamp outside the window is 403; inside the window is accepted; a replay is rejected; `SmatNone` accepts an anonymous post; an unknown remote is rejected | A01 | `SecurityAccess` (replay, `SmatNone`, unknown remote); `V9SelfContainedTokens` (proof) (missing, bad and late signatures with their controls) |
| Service Run session token, `api_key` and `login_key` headers from the fixture login key, Basic on a screen, shared wrong-password text, OTP pre-auth, MFA, initial admin, short password, login JSON omits credentials, password-reset enumeration, IP allow list, lockout | A07 | `SecurityAuthn`; initial admin and short password are `V6Authentication` (proof) |
| Unauthorized and error HTML have no script tag or stack frame | A10 | `SecurityHeaders` (the unauthorized page); `V16Logging` (proof) (the REST refusal and the internal error page) |
| A SQL-looking preference is stored as data | A05 | `V1Encoding` (proof) |
| Multipart upload: `MZ` and a `#!` script are rejected; a ZIP is accepted | A02 | `SecurityHeaders` |
| Screen tarpit and the `sec.tap.user` transition tarpit are 429 | A01 | `SecurityTarpit` (production conf; skips under `--serve`) |

### Tools database rules

These are server-side proofs, not client specs: framework specs (`./moq test framework --spec Name`, `./moq test tools
--spec Name`), crate tests (`cargo test -p moqui-tools`) and embedded-cluster host tests (`cargo test -p moqui-host
--test it -- name`). The rules are in [Attack-Surface, Tools and System](Attack-Surface-Tools.md#tools-and-system) and
[Database](../tools/Database.md).

| Control | Test |
| --- | --- |
| SQL is read only by default; a write is refused by the database; a write with Allow Changes needs the one-time token, for the same user, once; on production the database name is typed; every statement is audited, a failed one too; Explain Analyze of a write needs the token and is rolled back; cancel stops a running statement, and another user may not cancel it | `sql_runs_are_read_only_by_default_confirmed_audited_and_cancelled` (`crates/moqui-tools/src/sql_tests.rs`); `SqlServices` "a read-only run returns rows … a write needs the confirmation" |
| SQL needs `SQL_RUNNER_WEB`, and with authorization on the service asks for all actions on the Tools app | `SqlServices` "a user without SQL_RUNNER_WEB …", "with authorization on, the service itself asks …" |
| Transaction control is refused, and statements that cannot run in a transaction are known | `statements_that_cannot_run_in_a_transaction_or_control_one_are_known_by_their_first_words` (`sql_tests.rs`) |
| An external database is refused on a production instance with the vague error, and used on a dev instance through a connection row | `sql_runs_on_a_second_database_through_a_connection_row_and_production_refuses_it` (`crates/moqui-host/tests/it/database_external.rs`) |
| A DataDocument field expression needs `DATABASE_ADMIN`; a plain field path and a load with authorization off do not | `DocumentExpression` (framework) "a DataDocument field expression needs DATABASE_ADMIN …" |
| Grid secrets: password fields stay out through a view, also renamed (`pw`), and take no condition or sort; a view with a restricted member (`UserAccountAndGroup`) is refused; restricted entities are refused for reading and saving | `DataGrid` (framework) "secrets stay out through a view, renamed or not …", "a restricted entity is refused …" |
| Grid exports go only to the user's own places unless `DATABASE_ADMIN`; a SQL grid view needs SQL rights; a download of another user's export is refused | `DataGrid` (framework) "an export goes to the user's own place …", "a grid view of SQL is stored only by a user who may run SQL …"; `DataGrid` (tools) "an export download is refused …" |
| A view alias that renames a password field is secret, and `UserGroupMemberUser` is restricted | `a_view_that_renames_a_password_or_reads_a_restricted_entity_is_caught` (`crates/moqui-entity/src/entity/tests.rs`) |
| ETL runs as the user: a worker run denied the target entity loads nothing; the ETL rows, `DatabaseConnection`, group membership and `delete#` targets are refused, also through `update#moqui.etl.EtlMappingField`; another user's run, run id and value map are refused | `a_saved_mapping_is_checked_run_as_a_job_and_rolled_back_on_a_bad_file` (`crates/moqui-host/tests/it/etl_runs.rs`), and `service_targets_that_delete_or_reach_restricted_entities_are_refused` (`crates/moqui-tools/src/etl/run_tests.rs`) |
| ETL expressions need `GROOVY_SHELL_WEB` to save, run and evaluate; locations outside the allowed places are "Not available"; `${}` takes only the date tokens; a password lookup is refused | `etl_runs.rs` (above); `the_transformer_maps_looks_up_formats_and_evaluates` (`run_tests.rs`); `a_location_expands_only_its_date_tokens`, `only_the_import_places_are_readable_without_a_user` (`crates/moqui-tools/src/etl/access.rs`) |
| A load-wrap on a worker runs as the user: an ADMIN not in `ADMIN_ADV` cannot load an `ADMIN_ADV` membership, and `runAsUser` restores the caller's authorization state | `LoadWrap` (tools) |
| A database job is cancelled through its recorded backend, leaves an invalid index, and its rerun drops and builds it again | `a_concurrent_index_job_runs_is_cancelled_through_the_backend_and_runs_again` (`crates/moqui-host/tests/it/database_jobs.rs`) |

HMAC signing in the client is `hmac.sha256Base64` and `hmac.sha256Timestamp` ([JS-Test-Client](../dev/JS-Test-Client.md)). They match `SmatHmacSha256` and `SmatHmacSha256Timestamp`.

### Stored secrets

Crate tests for [Threat model: stored secrets](Attack-Surface-Data.md#threat-model-stored-secrets): `cargo test -p moqui-entity crypto`, and `cargo test -p <crate> <name>` for the others. A test whose name starts with `known_gap_` pins a weakness. When it fails, the weakness changed: update the threat model.

| Claim | Test |
| --- | --- |
| A stored value holds no plaintext, in its text or its bytes | `a_stored_value_does_not_hold_its_plaintext` (`crates/moqui-entity/src/entity/crypto_tests.rs`) |
| The wrong pass, salt or iteration count does not decrypt; an empty pass is refused | `only_the_same_pass_salt_and_iterations_decrypt`, `wrong_or_empty_pass_fails` (`crypto_tests.rs`) |
| A pass that is not printable ASCII is refused | `a_pass_phrase_that_is_not_ascii_is_an_error` (`crypto_tests.rs`) |
| `CryptConfig`'s `Debug` does not show the pass | `debug_leaves_out_the_pass` (`crypto_tests.rs`) |
| An encrypted field reaches SQL as ciphertext and reads back as plaintext | `try_insert_seals_the_password_before_sql` (`crates/moqui-entity/src/entity/tests.rs`) |
| Every bound value of an encrypted field in a query is encrypted | `every_bound_value_of_an_encrypted_field_is_sealed` (`crates/moqui-entity/src/entity/field_rw_tests.rs`) |
| A generated secret is 32 random bytes, different each time; a secret is named by its flag or by `pass`, `pw` or `key` | `generated_secrets_are_32_random_bytes_and_secret_names_are_known` (`crates/moqui-core/src/context/tests.rs`) |
| A generated secret is made once into its store row and stays the same; where it is kept is [Sealed start](#sealed-start) | `a_generated_secret_is_made_once_into_its_row_and_answers_the_conf_reader` (`crates/moqui-core/src/seal/secrets_tests.rs`) |
| Passwords are argon2id, login keys SHA-256, and a legacy hash becomes argon2id | `hash_password_is_an_argon2id_phc_string`, `hash_login_key_stays_sha256_hex` (`crates/moqui-core/src/user/password.rs`), `a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string` (`user/login_tests.rs`) |
| A session cookie with a bad MAC is no session | `a_bad_mac_is_an_unknown_session` (`crates/moqui-core/src/user/session_tests.rs`) |
| An admin reads only whether a secret is set; its audit row has no value | `AdminConnectionServices` "an endpoint change without the secret clears it …" (framework) |
| A client device key file is its user's alone | `a_file_key_is_the_owners_alone_and_can_be_forgotten` (`crates/moqui-client/src/keys_tests.rs`) |
| Gap: equal plaintexts give equal ciphertext | `known_gap_equal_plaintexts_give_equal_ciphertext` (`crypto_tests.rs`) |
| Gap: a shared 16-byte prefix gives a shared first block | `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block` (`crypto_tests.rs`) |
| Gap: a changed ciphertext is not detected; a writer sets the first block | `known_gap_a_changed_ciphertext_is_not_detected` (`crypto_tests.rs`) |
| Gap: AES-128 from 10 PBKDF2 iterations and a fixed salt | `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt` (`crypto_tests.rs`) |
| Gap: DES ciphertext still decrypts | `known_gap_des_ciphertext_still_decrypts_through_the_alternates` (`crypto_tests.rs`) |
| Gap: a value sealed under the public default pass is read as valid | `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid` (`crypto_tests.rs`) |
| Gap: an empty `crypt-pass` turns field encryption off | `known_gap_an_empty_crypt_pass_turns_field_encryption_off` (`crates/moqui-entity/src/entity/tests.rs`) |
| Gap: a new `webapp-list/webapp[webroot]/session-config@cookie-key` ends every session; there is no old key | `known_gap_a_new_session_cookie_key_ends_every_session_cookie` (`session_tests.rs`) |

### Sealed start

Crate tests for [Sealed start](../Run-Deploy.md#sealed-start) and [where secrets live](../Run-Deploy.md#where-secrets-live): `cargo test -p moqui-core seal` (the keeper, the methods, the store), `cargo test -p moqui-host --test it -- sealed_start` (a server on its operator socket), and `cargo test -p <crate> <name>` for the others. None is a `known_gap_` test, and none of the `known_gap_` tests above changed with them: those pin the field format, which still keys from the crypt pass. Files: `seal/tests.rs`, `seal/secrets_tests.rs` and `seal/methods/cloud_tests.rs` are under `crates/moqui-core/src/`; `sealed_start.rs` is `crates/moqui-host/tests/it/sealed_start.rs`.

| Claim | Test |
| --- | --- |
| A dev instance's first start makes the `dev` seal by itself, and the next start opens it | `a_first_start_on_a_dev_instance_makes_the_dev_seal_and_the_next_start_opens_it` (`seal/tests.rs`) |
| A production first start waits for `moqui operator init`; an unattended boot (a tool, `moqui load`) never makes a seal, even with an automatic method | `a_production_first_start_waits_for_init_and_an_unattended_boot_makes_no_seal`, `an_unattended_production_boot_never_makes_a_seal_even_with_an_automatic_method` (`seal/tests.rs`) |
| The `dev` seal is refused on a production instance; the method recorded at init wins over the conf | `the_dev_seal_is_refused_on_a_production_instance`, `the_method_recorded_at_init_wins_over_the_conf` (`seal/tests.rs`) |
| `kms` and `transit` are automatic methods; an unknown method is refused | `kms_and_transit_are_automatic_methods_and_an_unknown_method_is_refused` (`seal/tests.rs`) |
| A sealed server opens no port, and its own user runs only `status` on the operator socket | `a_sealed_server_opens_no_port_and_its_own_user_runs_only_status` (`sealed_start.rs`) |
| `status` answers while an unseal holds the state; a wait ends on unseal or when abandoned | `status_answers_while_an_unseal_holds_the_state`, `a_wait_ends_on_unseal_or_when_abandoned` (`seal/tests.rs`) |
| `seal` drops the key and closes the listeners until `unseal`; a seal by an operator lasts across a restart and drops the shares held | `seal_drops_the_key_tells_the_watchers_and_waits_for_unseal`, `a_seal_by_an_operator_lasts_across_a_restart_until_unseal_and_drops_the_held_shares` (`seal/tests.rs`), `a_dev_instance_makes_its_seal_and_seal_closes_the_listeners_until_unseal` (`sealed_start.rs`) |
| A passphrase seal opens with its passphrase; each wrong one doubles the wait, up to the most set; a production instance refuses a passphrase under the server cost | `a_passphrase_seal_opens_with_its_passphrase_and_a_wrong_one_doubles_the_wait`, `the_wait_after_wrong_entries_stops_at_the_most_set`, `a_production_instance_refuses_a_passphrase_seal_under_the_server_cost` (`seal/tests.rs`) |
| A Shamir seal opens with its threshold of shares and refuses a wrong one when it is entered; a conf naming another method is ignored | `a_shamir_seal_opens_with_its_threshold_of_shares_and_refuses_a_wrong_one_on_entry` (`seal/tests.rs`), `a_shamir_server_counts_a_wrong_share_ignores_another_conf_method_and_unseals_by_its_shares` (`sealed_start.rs`) |
| A `tpm` first start holds the recovery shares once; a failed automatic unseal waits sealed for the recovery shares | `a_tpm_first_start_holds_the_shares_once_and_a_failed_unseal_waits_for_the_recovery_shares` (`seal/tests.rs`), `a_tpm_first_start_holds_its_shares_once_and_a_failed_auto_unseal_waits_for_the_recovery_shares` (`sealed_start.rs`) |
| The recovery shares open any method; `rekey` changes only the method; `rotate-root` rewraps everything, and one stopped before its rename keeps the old root | `the_recovery_shares_open_any_method_and_rekey_changes_only_the_method`, `rotate_root_rewraps_everything_and_one_stopped_before_its_rename_keeps_the_old_root` (`seal/tests.rs`) |
| A recovery bundle restored on a new runtime unseals with the recovery shares, then rekeys and rotates; a bundle that is not sound is refused | `a_bundle_restored_on_a_new_runtime_unseals_with_the_recovery_shares`, `a_restore_refuses_a_bundle_that_is_not_sound` (`seal/tests.rs`), `a_restore_of_the_bundle_on_a_new_runtime_unseals_with_the_shares_then_rekeys_and_rotates` (`sealed_start.rs`) |
| `seal.json` refuses what it does not read; a write stopped before its rename leaves the old file; two processes end with one seal, keep each other's changes, and a stale rekey is refused | `the_seal_file_round_trips_and_refuses_what_it_does_not_read`, `a_write_stopped_before_its_rename_leaves_the_old_file`, `two_processes_making_a_seal_at_once_end_with_one`, `writes_from_two_processes_keep_each_others_changes_and_a_stale_rekey_is_refused` (`seal/tests.rs`) |
| The operator socket, the unseal pacing and the `tpm` program come only from the conf files | `the_socket_pacing_and_tpm_program_settings_come_only_from_the_conf_files` (`seal/tests.rs`) |
| The root key's `Debug` shows nothing | `the_root_key_hides_itself` (`seal/tests.rs`) |
| A bootstrap secret moved to another name does not open; it is kept in `seal.json`, and `seal` drops the opened values | `a_bootstrap_secret_moved_to_another_name_does_not_open` (`seal/tests.rs`), `a_bootstrap_secret_goes_to_seal_json_and_seal_drops_the_opened_values` (`seal/secrets_tests.rs`) |
| A `SecretValue` row holds no plaintext and opens only under its name and generation; the store lists names and generations, never a value | `a_secret_value_is_sealed_under_its_name_and_generation_and_a_moved_row_does_not_open` (`seal/secrets_tests.rs`) |
| A generated secret is made once into its row; the cluster TLS pair is made in one transaction, and a member that lost the race takes the winner's pair | `a_generated_secret_is_made_once_into_its_row_and_answers_the_conf_reader`, `a_generated_pair_is_made_in_one_transaction_and_a_lost_race_takes_the_winners_pair` (`seal/secrets_tests.rs`) |
| A boot with no seal keeps nothing and names `moqui operator init` | `a_boot_with_no_seal_keeps_nothing_and_names_operator_init` (`seal/secrets_tests.rs`) |
| A new crypt pass needs `--force`; the crypt passes and the datasource password are not removed | `the_crypt_pass_is_replaced_only_by_force_and_the_guarded_passes_are_not_removed` (`seal/secrets_tests.rs`) |
| A member whose seal opens none of the stored secrets is refused, with an alert | `a_seal_under_which_no_stored_secret_opens_is_refused_with_an_alert` (`seal/secrets_tests.rs`) |
| A new runtime's `.env` and runtime directory hold no secret; the embedded cluster's password is in `seal.json`; `moqui operator secret put`, `list` and `rm` never print a value | `a_dev_instance_makes_its_seal_and_seal_closes_the_listeners_until_unseal` (`sealed_start.rs`, its `secrets_out_of_the_runtime` check) |
| The embedded cluster given its password keeps no password file, and a start with another password is refused | `a_cluster_with_its_password_given_keeps_no_password_file_and_a_cold_start_opens_with_it` (`crates/moqui-drivers/src/embedded/cluster/tests.rs`) |
| An ACME certificate's key is served from memory; no file holds it | `the_certificate_is_written_where_the_listener_reads_it` (`crates/moqui-host/src/acme/tests.rs`) |
| A script's `System.getProperty` answers no name the store keeps | `a_name_the_store_keeps_is_a_secret_name` (`seal/secrets_tests.rs`), `a_script_gets_nothing_for_a_name_the_secret_store_keeps` (`crates/moqui-core/src/script/runtime_tests.rs`) |
| AWS KMS wraps and unwraps with SigV4-signed calls and the encryption context, by a credential file or the instance role through IMDSv2 | `aws_kms_wraps_and_unwraps_with_a_sigv4_signed_request`, `aws_kms_signs_with_the_instance_roles_session_from_imdsv2` (`seal/methods/cloud_tests.rs`) |
| GCP Cloud KMS wraps and unwraps with the metadata server's token or a service account's | `gcp_cloud_kms_wraps_and_unwraps_with_the_metadata_servers_token_or_a_service_accounts` (`seal/methods/cloud_tests.rs`) |
| Azure Key Vault wraps and unwraps; a wrapped key the vault did not sign is refused before any unwrap, and a key version not of the conf's key is not followed | `azure_key_vault_wraps_and_unwraps_by_client_credentials_or_the_managed_identity` (`seal/methods/cloud_tests.rs`) |
| Transit wraps and unwraps with its token; an unseal the Vault refuses stays sealed | `transit_wraps_and_unwraps_with_the_vault_token_and_a_refused_unseal_stays_sealed` (`seal/methods/cloud_tests.rs`) |
| A credential is a file of root's or a systemd credential, never a conf literal; the root key and a credential go only over https or to the loopback | `a_credential_is_a_file_or_a_systemd_credential_never_a_conf_literal` (`seal/methods/cloud_tests.rs`) |
| Where the root key goes is read from the conf files only; an environment name's value is ignored | `the_cloud_settings_are_read_from_the_conf_files_only` (`seal/methods/cloud_tests.rs`) |

### Vaults

Tests for [Vaults](Vaults.md) and the vault rows of [Threat model: stored secrets](Attack-Surface-Data.md#threat-model-stored-secrets). Crates: `cargo test -p moqui-vault` (the crypto, and the known-answer vectors in `crates/moqui-vault/testdata/vectors.json`), `cargo test -p moqui-vault-server` (the server's rules on the in-memory entity facade, with the vault crate's own session as each client), `cargo test -p moqui-client vault`, `cargo test -p moqui-client-tauri vault`, `cargo test -p moqui-core vault_policy`. Specs: `./moq test framework --spec VaultServices`; `./moq test-client framework --offline --spec VaultModule`; `./moq test-client framework --serve --spec VaultShared` (and `VaultLease`, `VaultRecovery`); `./moq test mr-boss --spec VaultCommons`; `./moq test-client mr-boss --offline --spec VaultLock`. The vault crate's files are under `crates/moqui-vault/src/`, the server's under `crates/moqui-vault-server/src/tests/`.

| Claim | Test |
| --- | --- |
| AES-256-GCM opens only with the same key and associated data; the associated data is length-prefixed; a changed byte fails; nonces are fresh | `seal_opens_with_the_same_key_and_associated_data`, `changed_associated_data_fails`, `associated_data_is_length_prefixed`, `tampered_bytes_fail`, `nonces_are_fresh` (`aead_tests.rs`) |
| Bodies are padded to 256-byte steps, and one over 64 KiB is refused | `bodies_pad_to_256_byte_steps`, `bodies_over_64_kib_are_refused` (`aead_tests.rs`) |
| An item key or body moved to another item, vault or key version, or an operational item under another type, binding or name, does not open | `an_item_key_or_body_moved_to_another_item_vault_or_version_fails`, `an_operational_item_under_another_type_binding_or_name_fails` (`vault_tests.rs`), `names_without_a_label_never_encode_as_labelled_ones` (`names_tests.rs`) |
| HPKE matches the CFRG vector; a wrap opens only with its key and names; a low-order public key is refused | `matches_the_cfrg_vector_for_x25519_sha256_aes256gcm_base`, `a_wrap_opens_only_with_its_key_and_names`, `a_low_order_public_key_is_refused` (`hpke_tests.rs`) |
| Argon2id below the floor or above the ceiling is refused, stored or asked for; a wrap under the target opens and is wrapped again | `parameters_under_the_floor_are_refused`, `parameters_over_the_ceiling_are_refused`, `under_the_target_signals_a_rewrap` (`kdf_tests.rs`), `stored_parameters_outside_the_floor_and_ceiling_are_refused`, `a_wrap_under_the_target_opens_and_is_wrapped_again` (`personal_tests.rs`) |
| Keys are cleared on drop; no `Debug` shows a key, a private key, a seed or a share | `random_keys_differ_and_zeroize_clears`, `debug_never_shows_the_key` (`keys_tests.rs`), `debug_hides_the_private_key` (`hpke_tests.rs`), `a_seed_round_trips_and_debug_is_redacted` (`sign_tests.rs`), `debug_never_shows_a_share_value` (`shamir_tests.rs`) |
| Base32 and hex decode in constant time, and a bad character anywhere is refused | `the_arithmetic_maps_agree_with_the_alphabet_for_every_character`, `a_bad_character_anywhere_is_refused` (`text_tests.rs`) |
| Every k shares give the secret; fewer do not, and k − 1 shares look the same for any secret; a typo in a share is caught | `every_k_subset_recovers_the_secret`, `fewer_than_k_shares_do_not_give_the_secret`, `k_minus_one_shares_look_the_same_for_any_secret`, `a_typo_in_a_share_is_caught` (`shamir_tests.rs`) |
| A personal key opens with its passphrase or its recovery code, bound to its id and public keys | `the_passphrase_opens_and_a_wrong_one_fails`, `the_recovery_code_opens_and_binds_the_key_id_and_public_keys` (`personal_tests.rs`), `a_personal_key_is_made_locked_and_opened_by_its_passphrase` (`ops_tests.rs`) |
| A fingerprint is six words over both public keys; a key check covers only the key it checked | `a_fingerprint_is_six_words_over_both_public_keys`, `a_key_check_is_signed_and_covers_only_the_key_checked`, `a_lease_key_check_covers_only_that_lease_key` (`fingerprint_tests.rs`) |
| A member-list chain is accepted only signed by managers and anchored in a checked key, trusted only from its first checked signer; a server cannot add its own member or key; a wrap is taken only from a manager to a current member; a rollback, a fork, and a list naming one key twice are refused | `a_chain_signed_by_managers_and_anchored_in_a_checked_key_is_accepted`, `a_chain_with_no_checked_signer_is_refused`, `nothing_before_the_first_checked_signer_is_trusted`, `a_server_cannot_add_its_own_member_or_key`, `a_wrap_is_accepted_only_from_a_manager_to_a_current_member`, `a_remembered_version_refuses_rollbacks_and_forks`, `a_fork_that_goes_higher_is_refused`, `a_member_list_naming_one_key_twice_is_refused` (`keyversion_tests.rs`) |
| An entry the recovery key signs only gives one member a new key | `a_recovery_entry_only_gives_one_member_a_new_key` (`keyversion_tests.rs`) |
| The server deposits only to a deposit key a manager of the current version signed, and cannot read the item back | `the_server_deposits_only_to_a_signed_deposit_key_and_cannot_read_back` (`vault_tests.rs`), `a_deposit_key_is_accepted_only_for_the_current_version_from_a_manager`, `a_deposit_key_survives_its_signers_recovery_only` (`keyversion_tests.rs`) |
| A lease past its time limit or over four hours, changed after signing, unsigned, or signed by an unchecked lease key is refused; a binding pointed at another item fails at the unwrap | `an_expired_lease_or_one_over_four_hours_is_refused`, `a_lease_changed_after_signing_or_unsigned_is_refused`, `a_lease_signed_by_an_unchecked_lease_key_is_refused`, `a_binding_pointed_at_another_item_fails_at_the_unwrap` (`lease_tests.rs`) |
| Machine access opens only its item for its environment | `machine_access_opens_only_its_item_for_its_environment` (`machine_tests.rs`) |
| A delivery opens only for its recipient with a good signature, and is filed once | `a_delivery_opens_only_for_its_recipient_with_a_good_signature`, `a_delivery_id_is_filed_once` (`delivery_tests.rs`) |
| k shares give the recovery key, and fewer or wrong ones do not; an unsigned or replaced recovery key is refused; a session takes the holders' shares sealed to its ephemeral key | `k_shares_give_the_recovery_key_and_fewer_or_wrong_ones_do_not`, `an_unsigned_or_replaced_recovery_key_is_refused`, `a_session_takes_k_holders_shares_sealed_to_its_ephemeral_key` (`recovery_tests.rs`) |
| The code makes the checked-in vectors; the CFRG vector is computed, not copied | `the_vectors_are_what_the_code_makes`, `the_cfrg_vector_is_computed_not_copied` (`vectors_tests.rs`) |
| In the module: a vault key opens only through the checked chain; nothing is wrapped to an unchecked key, and a key check needs the fingerprint the user compared | `a_vault_key_opens_only_through_the_checked_chain`, `nothing_is_wrapped_to_an_unchecked_key_and_a_check_needs_the_right_fingerprint` (`ops_tests.rs`) |
| In the module: an item's first write after a rotation gets a new item key; a list view's summary holds no masked field; a header is sealed by its key version and a rotation seals it again | `items_seal_open_and_get_a_new_key_after_a_rotation`, `a_summary_holds_no_secret_and_an_old_item_key_is_replaced`, `a_header_is_sealed_by_its_version_and_a_rotation_reseals_it` (`ops_tests.rs`) |
| A single-secret vault opens only with its password or generated key; a generated key wraps by HKDF | `a_password_opens_a_single_secret_vault_and_a_wrong_one_fails`, `a_generated_key_opens_a_single_secret_vault_and_another_fails` (`vault_tests.rs`), `a_generated_key_wraps_and_opens_a_single_secret_vault` (`ops_tests.rs`) |
| In the module: a lease is approved only under a checked lease key and within its time limit | `a_lease_is_approved_only_under_a_checked_lease_key_and_its_time_limit` (`ops_tests.rs`) |
| In the module: k holders' shares combine there and rewrap the vault key to the member's new key, and nothing of the recovery goes back to the page or stays after; a holder who is also a member with that key version open recovers without forgetting it first: the recovery key's wrap must open the same key (another is refused), and the version stays open after the rekey | `a_recovery_of_k_holders_combines_in_the_module_and_rewraps_to_the_new_key` (`ops_tests.rs`) |
| In the module: device memory opens the personal key only with the key both halves make, and only for its memory, user, time and personal key | `device_memory_opens_the_personal_key_only_with_its_key_and_memory` (`ops_tests.rs`) |
| No row of a personal vault holds an item's name, type or value in the clear | `no_row_of_a_personal_vault_holds_an_items_name_type_or_value_in_the_clear` (`vaults.rs`) |
| A non-member gets no wrap and no body (the vault, its items and history are "not found"); a reader does not write | `a_non_member_gets_no_wrap_and_no_body` (`vaults.rs`) |
| A single-secret vault has no members and no recovery; an operational vault needs `VAULT_ADMIN` and keeps recovery on or two members | `a_single_secret_vault_refuses_members_and_recovery_and_an_operational_vault_needs_its_rules` (`vaults.rs`), `a_single_secret_vault_gets_no_recovery_and_an_operational_vault_keeps_it_or_two_members` (`recovery.rs`) |
| A deletion needs a recent sign-in, and a deleted vault's id is never given again | `a_deletion_needs_a_recent_sign_in_and_a_deleted_vaults_id_is_not_given_again` (`vaults.rs`) |
| A removal's rotation keeps the sealed header; a single-secret vault's new wrap needs a strong, recent sign-in and the floor | `a_rotation_keeps_the_header_an_old_item_key_is_replaced_and_a_secret_wrap_is_stored` (`vaults.rs`) |
| A rename (`update#VaultHeader`) seals the header again, by a manager or a single-secret vault's owner, from the revision they read, ciphertext only, with a `VeHeaderChanged` event; a writer, an older revision, a header sealed under another key version and one that is not ciphertext are refused | `a_rename_seals_the_header_again_for_a_manager_from_the_revision_read` (`vaults.rs`) |
| A removal rotates the key in one compare-and-swap; the removed member has no wrap, and a write at the old key version is refused | `a_removed_member_has_no_wrap_and_a_write_at_the_old_key_version_is_refused` (`members.rs`) |
| Nothing is wrapped to a changed personal key before it is checked again | `nothing_is_wrapped_to_a_changed_personal_key_before_it_is_checked_again` (`members.rs`) |
| A key version or wrap no manager signed is refused by the server, and by a member's client when a database writer stores it | `a_key_version_or_wrap_no_manager_signed_is_refused_by_the_server_and_the_client` (`members.rs`) |
| Demoting a manager, or changing a manager's key, remakes in the same change the wraps they signed | `a_demoted_managers_wraps_are_remade_in_the_same_change` (`members.rs`) |
| The wrapped personal key goes only to its user after a strong, recent sign-in; a new key or passphrase needs the same; a key check is stored only signed by its maker's active key | `a_wrapped_personal_key_needs_a_strong_recent_sign_in`, `keys_are_replaced_and_rewrapped_after_a_strong_sign_in_and_checks_are_signed_by_their_maker` (`personal.rs`) |
| A delivery goes only to its recipient, signed by its sender's key | `a_delivery_goes_only_to_its_recipient` (`deliveries.rs`) |
| A lease opens only the items it names, and nothing it held opens after it ends | `a_lease_opens_only_the_items_it_names_and_nothing_after_it_ends` (`leases.rs`) |
| An approval needs the approve role and a recent sign-in; a requester does not approve their own lease while another approver exists; a demoted approver's approval does not open | `an_approval_without_the_approve_role_or_a_recent_sign_in_is_refused`, `an_approver_does_not_approve_their_own_lease_and_a_demoted_approver_does_not_open_one` (`leases.rs`) |
| A lease whose event cannot be written does not open | `a_lease_whose_event_cannot_be_written_does_not_open` (`leases.rs`) |
| A lease runs only on the cluster member that asked; a start ends only that member's leases; a restart ends them | `a_lease_approved_through_one_member_runs_on_another`, `the_registry_takes_the_cluster_member_id_and_ends_only_its_own_leases`, `a_restart_ends_the_members_leases` (`leases.rs`) |
| Machine access opens only its items for its environment, goes stale with a new item key, and a grant never approved ends; the machine-run hook opens only what the environment holds | `machine_access_opens_only_its_items_for_its_environment`, `machine_access_goes_stale_with_a_new_item_key_and_an_unapproved_grant_ends`, `the_machine_run_hook_opens_only_what_the_environment_holds_and_ends_its_lease` (`leases.rs`) |
| A client refuses a changed lease and one signed by an unchecked lease key; a binding pointed at another item fails at the unwrap | `a_client_refuses_a_changed_lease_and_an_unchecked_lease_key`, `a_binding_pointed_at_another_item_fails_at_the_unwrap` (`leases.rs`) |
| A deposit key with no manager's signature is refused; a removal takes a deposited item in | `the_server_refuses_a_deposit_key_with_no_managers_signature`, `a_removal_takes_in_a_deposited_item` (`leases.rs`) |
| A leased value is never in a job's stored parameters | `a_leased_value_is_not_in_a_jobs_stored_parameters` (`leases.rs`) |
| An operation or item name with a control character is refused | `control_characters_are_refused_in_operations_and_item_names` (`leases.rs`) |
| Reveal goes only to the reveal role, with an event | `a_reveal_goes_only_to_the_reveal_role_and_is_an_event` (`leases.rs`) |
| k recovery holders recover a vault to a member's new key and every member is told; k − 1 cannot | `k_holders_recover_a_vault_to_a_members_new_key_and_every_member_is_told` (`recovery.rs`) |
| Each holder's step needs `VAULT_RECOVERY` and a recent sign-in; any person of the member list may cancel; a recovery key's user id acts in no vault | `a_holders_step_without_vault_recovery_or_a_recent_sign_in_is_refused`, `a_member_cancels_a_recovery_and_the_recovery_prefix_is_reserved` (`recovery.rs`) |
| A device memory is recorded and read only by a device's key, listed and ended by its own user; one ended, or for a replaced personal key, gives no secret | `a_device_memory_is_recorded_by_a_device_key_and_ended_by_its_user` (`device_memory.rs`) |
| In the apps, device memory ends at its time and when the device is revoked, needs a key store, an open key and a time in range, and waits for the device's lock; a lock in the middle leaves nothing | `device_memory_ends_at_its_time_limit`, `device_memory_ends_when_the_device_is_revoked`, `device_memory_needs_a_key_store_an_open_key_and_a_time_in_range`, `a_key_that_needs_the_device_lock_waits_for_it`, `a_lock_in_the_middle_leaves_nothing_behind` (`crates/moqui-client/src/vault/tests.rs`) |
| In the apps, rollback memory is the app's, not the page's; sessions are per page and lock when idle or after a sleep | `the_apps_memory_of_a_vault_is_the_modules_not_the_pages`, `sessions_are_per_page_and_lock_when_idle`, `a_sleep_is_the_wall_clock_running_ahead_of_the_monotonic_one` (`crates/moqui-client/src/vault/tests.rs`) |
| On the phone, device memory needs the lock screen or a biometric at each use, and ends at its limit and on revocation | `device_memory_on_the_phone_needs_the_lock_and_ends_at_its_limit_and_on_revocation` (`crates/moqui-mobile/src/keystore_tests.rs`) |
| In the apps, only a page whose document is a vault screen's calls the vault; leaving, or loading another screen or frame, locks it | `only_the_vault_screens_are_vault_screens`, `a_page_may_call_the_vault_only_from_a_vault_screens_document` (`crates/moqui-client-tauri/src/vault_tests.rs`) |
| The apps never load the server's vault module | `the_servers_vault_module_is_never_loaded_in_the_app` (`crates/moqui-client-tauri/src/scheme_tests.rs`) |
| The desktop's native vault is the app windows', not the shell's | `the_native_vault_is_an_app_windows_command` (`crates/moqui-desktop/src/client/windows_tests.rs`) |
| `moqui vault` keeps the highest member-list entry per server and vault | `the_state_keeps_the_highest_entry_and_reads_back` (`crates/moqui-host/src/vault_cmd/state_tests.rs`) |
| A request that resolves to a vault screen (one of `vault@screen-names` among the screens its path names, not an extra-path name), the `/qapps` page's document too, gets the site's `screen-render` policy with `vault@screen-policy` added and keeps `X-Frame-Options`; a directive the site sets is only given the worker's `blob:` and `'wasm-unsafe-eval'`; the page is told it is a vault screen's | `a_vault_screen_and_its_qapps_page_get_the_vault_policy_and_an_extra_path_name_does_not` (`crates/moqui-core/src/screen/tests/vault_pages.rs`), `the_vault_policy_adds_to_the_sites_policy_and_never_loosens_it`, `a_path_mentions_a_vault_screen_by_a_decoded_name` (`crates/moqui-core/src/web/vault_policy_tests.rs`) |
| A file asked for with an integrity hash as its version (`?v=`) gets the year-long cache only when the hash is the file's | `a_file_under_the_screen_is_served_with_a_type_a_validator_and_a_cache_rule` (`crates/moqui-core/src/screen/tests/files.rs`) |
| A vault, its items and history are not found to a non-member, whose write is refused; a delivery is listed and filed only by its recipient; a wrapped personal key needs a password and a second factor, recently | `VaultServices` (framework) "a vault, its items and an item's history are not found to a non-member …", "a delivery is listed and filed only by its recipient", "a wrapped personal key is sent only after a strong, recent sign-in …" |
| The module in its browser worker makes the native vectors; an item seals and opens there and a lock forgets every key; a vault key crosses to a second worker only through a checked member list; in the apps the client calls the native vault and loads no module | `VaultModule` (framework, offline; cases in `test/client/pages/VaultModule.page.js`) |
| Over REST: what the server serves of a personal vault holds no item's name, type or value; a client refuses a key version, wrap or delivery no one signed rightly, and a changed key not checked again; a removed member gets no wrap | `VaultShared` (framework) |
| Over REST: an approver checks the lease key and approves in their client, and the member that asked holds the lease; a revoked lease ends and a second approval is refused | `VaultLease` (framework) |
| Over REST: two of two holders recover a member's vault to the member's new key | `VaultRecovery` (framework) |
| Commons › Vault: a single-secret vault refuses a member and a recovery key; a private contact in the user's vault never reaches CardDAV | `VaultCommons` (mr-boss) |
| Commons › Vault in the browser: a private contact shows under Contacts and no request carries it; an encrypted export opens only as an export; a refused removal leaves no key version; a masked value shows only when asked, and the vault locks after the idle time | `VaultLock` (mr-boss, offline) |
| Commons › Vault: leaving the vault screens for anything but them and Commons › Contacts locks the vault; a browser keeps the item revisions it saw per user and vault; a rename sends only ciphertext and is refused from a moved revision | `VaultLock` (mr-boss, `test/client/Commons/VaultLock.spec.js`) |
| A recovery key's holders, and the groups that hold `VAULT_RECOVERY`, are not told to a user without `VAULT_RECOVERY` | `VaultCommons` (mr-boss, `test/Commons/VaultCommons.spec.js`) |

### Not applicable

These Java HTTP tests have no port. The reason is the behavior they call.

| Java test | Reason |
| --- | --- |
| Six `/groovysh` tests (HTTP GET, anonymous upgrade, admin eval, anonymous-after-admin, VIEW-only, same-origin admin) | No Groovy shell, no `/groovysh` route and no conf endpoint for it. |
| Eight JSON-RPC tests, including `api_key` in a JSON body, array bodies, and `reset#Password` over `/rpc/json` | `/rpc/json` is 404 on purpose (`SecurityAccess`). |
| `test_entity_sync_put_needs_all_authz`, `test_entity_sync_put_with_all_authz_stores`, `test_entity_sync_put_does_not_store_user_group_permission` | Those proofs call `put#EntitySyncData` over `/rpc/json`. No REST stand-in was added. |
| `test_rest_schema_anonymous_is_401`, `test_rest_schema_does_not_set_acao_wildcard` | Both hit `/rest/entity.swagger`, which 404s before auth. Swagger 2 is not built. The schema gate is the `/rest/entity.json` rows in `SecurityRest`. |

### In-process proofs that were not ported

Java's Spock classes check these without an HTTP client. This pass did not look for a matching Rust unit test, and this catalog does not claim one.

- Conf and utility flags: SubEtha factory absent, `upload-executable-allow` default false (the HTTP magic check is ported), shipped `moqui_conf__entity_facade__crypt_pass` still `CHANGEME` until startup replaces it, password and login-key hash types, `allow-html` `safe` and `any`, HSTS on `screen-secure` only when the request is HTTPS, `email_allowed_hosts` empty, HMAC replay cache declared `distributed`, percent-encoded query names dropped, `getClientIp` address shapes.
- Integrity: component zip-slip and a contained-entry expand, XML external entities in `MNode`, ElFinder `joinUnderRoot` and hashed `..`, `create#UserAccount` ignoring extra password fields, DataSnapshot `disableEntityEca` restoring `ADMIN` membership, denied entity find writing `ArtifactAuthzFailure`.
- Authn beyond the HTTP rows: lockout re-enable after `disable-minutes`, password-reset token single use and expiry, `hasLoggedOut`, `loginUserKey` stored as a hash, WebSocket handshake `api_key` in the header versus the query, a handshake not keeping the previous user, `AUTHZA_ALL` without `SQL_RUNNER_WEB` or `GROOVY_SHELL_WEB` (the GroovyShell screen is removed; its permission name stays sealed so no grant can be made to it).
- Login history stored, and a failed login not storing the password.
- `/llm/v1` CSRF, a dummy `login_key` or Basic not skipping it, and an empty `allowed-path` failing closed (`LlmServletTests`). A2A with `llm-facade/a2a@enabled` true. HOLD `TransactionCacheDb` not writing production rows.
- Dependency versions that do not exist here (Log4j, Shiro) and the H2 TCP server.

## Recorded differences

Kept on purpose, and asserted by the specs. The parity notes are [Parity-Diffs](../../plans/Parity-Diffs.md) (P9-06 and the client-test-harness table).

- A preflight is 204 with an empty body.
- The session cookie is `moqui.session`, and `__Host-moqui.session` over https (SEC-WEB-05).
- A missing or wrong session token is 401 `Session token is missing or is not valid`.
- An anonymous `/notws` upgrade is 401. The foreign-origin proof uses a logged-in session.
- CORS and `/notws` compare scheme, host, and port. The same host on another port or scheme is refused.
- An IP-refused REST login is 401 and not logged in. The body names the user and the client address (`check_ip_allowed`). The spec does not require Java's shared invalid-login sentence. `X-Forwarded-For` does not supply the allow-list address while `webapp-list/webapp[webroot]@client-ip-header` is empty.

`WebUtilities.isSameOriginRedirect` with no host argument uses the webapp `http-host`, which is empty in the default conf, so an absolute `returnTo` is refused. The login screen calls that one-argument form. The request `Host` header is not a configured host. `SecurityHeaders` is the proof.

Three `SecurityScreens` assertions are tighter than the Python ones they port. A VIEW-only service run counts only 401 or 403 (a 302 would be the run succeeding). An anonymous data import counts a 302 only when `Location` is the login. A `GET` of the login with the demo password must still leave the session on the login page.

## Proof strength

- `--serve` does not prove the tarpit. `SecurityTarpit` skips while the status is not 429, which is correct for the load-test conf and is a failed proof on the production conf.
- `/status` is requested from loopback. The three features prove the public JSON shape and that forwarded headers do not hide it. They do not prove that a TCP source outside `webapp-list/webapp[webroot]@status-ips` is rejected.
- The HMAC replay keys are rows of `moqui.security.SignatureReplay`, which every server of a cluster sees; the proof runs one server, so it does not show a replay refused on a second one (`replay_store_tests` `a_replay_key_is_taken_once_across_members` does).
- With `search-facade@backend` at its default `postgres`, `/search` and `/dashboard` are 404 after authentication. The `SearchRemote` feature asserts the response is neither 401 nor 403.
- The `/resources` feature posts `cmd=put` at the volume root `v0_cm9vdA` and expects JSON containing `write not allowed` or `error`, and not `changed`. That is the root-refusal on every conf. It does not prove a production-only block of writes under `component://`.
- The menu feature passes when the body is not the Tools menu, including when the status is 500. An anonymous `GET /menuData/apps/tools` logged `cannot read property 'extraPathNameList' of undefined` (`screen_host.rs`). The in-process menu test expects 401 once `getMenuData` runs. The HTTP proof does not require that status.
- The email pixel proof is `GET /email/NOTAREALID.png`: status 200, `image/png`, inline, and the PNG magic. The client harness records the first four bytes as hex (`89504e47`). The full 68-byte image, and marking a known message `ES_VIEWED` with `receivedDate` set once, are the unit test `view_email_message_sends_the_png_and_marks_a_known_message`. That is Java's `markEmailMessageViewed` side effect. The Spock method that calls it was not ported.
- `/htmlr` features prove the path is 404. They do not prove an HTML renderer's error page.
- Positive controls sit next to the refusals: a session token that stores a preference, a same-origin CORS allow, a valid HMAC and a timestamp inside the window, `SmatNone` ingest, a loopback-allowed user logging in, an ADMIN schema document, a catch-all entity ALL that creates, a Tools CSV for a user who may open Tools, and a ZIP upload that is accepted. A refusal alone would still pass if the feature had stopped working.
- `requireFixture` skips only on HTTP 401. An MFA password-only login is 200 with `secondFactorRequired` and no `loggedIn`, so those features run. The success feature consumes the single-use code and is last. `ClientFixtures` refreshes the code, and `--db snapshot` is a fresh database.
- The lockout feature is last in `SecurityAuthn` and does not log `sec.lock.test` in afterwards.
