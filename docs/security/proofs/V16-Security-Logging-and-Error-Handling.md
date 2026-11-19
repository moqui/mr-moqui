# V16 Security Logging and Error Handling

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V16 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V16.1 Security Logging Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-16.1.1 | 2 | Inventory of logs: events, format, storage, access, retention | Gap | no inventory; pieces only: `logging@dir`, `max-files` (MoquiDefaultConf, Run-Deploy), telemetry `log-retention-days` (Monitoring.md), file modes (Attack-Surface-Process); `UserLoginHistory`, `ArtifactAuthzFailure`, `EntityAuditLog` not described as a set | - |

## V16.2 General Logging

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-16.2.1 | 2 | Log entries carry when, where, who and what | Gap | telemetry record has time, host, thread, logger, user, visitor (`crates/moqui-host/src/logging.rs` `event`); the file line (`plain_line`) has no user; `ArtifactAuthzFailure` has no client address | - |
| v5.0.0-16.2.2 | 2 | Synchronized clocks; timestamps in UTC or with offset | Configuration | stamps are UTC: `jiff::Timestamp` in `logging.rs` `event` (file stamp has no zone marker), telemetry epoch millis, `instance@default-time-zone` UTC; clock sync is the host's NTP (operator step) | `logging_tests.rs` `a_log_lines_stamp_is_its_instant_in_utc` (in-process) |
| v5.0.0-16.2.3 | 2 | Logs go only to documented destinations | Framework | sinks fixed in `logging.rs` `event`: stderr, log files, taps (TUI, SSH), dev tools capture, telemetry log; OTLP export only with `telemetry@otlp-endpoint`; named in Attack-Surface-Process and Monitoring.md, not in an inventory (16.1.1) | `logging_tests.rs` `log_lines_are_written_only_to_the_named_files` (in-process) |
| v5.0.0-16.2.4 | 2 | Logs in a common, machine-correlatable format | Framework | telemetry logs are OpenTelemetry log records (`logging.rs` `export_telemetry_log`); OTLP/HTTP export (Monitoring.md); a 500's error id in answer and log (`web/mod.rs` `log_error_id`) | `logging_tests.rs` `a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record` (in-process) |
| v5.0.0-16.2.5 | 2 | Sensitive data kept out of logs or masked | Gap | one secret-name rule for request details, HTTP log query, telemetry attributes, Visit URLs (`crates/moqui-core/src/context/identity.rs` `secret_parameter`); token routes as `:token`; outbound URLs `redacted`; no password in `UserLoginHistory`; rsipstack held at WARN; the subscriber redacts nothing; open: SEC-COM-95 (`pin` not a secret name), SEC-GIT-96 | - |

## V16.3 Security Events

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-16.3.1 | 2 | Every sign-in attempt logged with its method | Gap | `crates/moqui-core/src/user/login.rs` `save_history`: one `UserLoginHistory` row (user, time, visit, success) per user per minute; none for an unknown user; no method or factor field; SSH key login none (SEC-FIL-89 open) | - |
| v5.0.0-16.3.2 | 2 | Refused authorization attempts logged | Framework | `crates/moqui-core/src/user/authz.rs` `record_failure`: an `ArtifactAuthzFailure` row (artifact, action, user, time, deny) for each refused artifact check; a permission check in service code is not recorded (likely) | `a_failed_check_leaves_an_authz_failure_row` (`crates/moqui-core/src/user/authz_tests.rs`) |
| v5.0.0-16.3.3 | 2 | Documented security events and bypass attempts logged | Gap | no list of security events (16.1.1); a web protection ban is logged (`guard.rs`), other refusals only counted; a CSRF refusal is not logged (`screen/render.rs` near `csrf_rejected`); per-client rules off by default; SEC-DAT-83 open (a load turns the audit log off for every thread, Medium lead) | - |
| v5.0.0-16.3.4 | 2 | Unexpected errors and control failures logged | Framework | every 500 logged, on production under an error id (`web/mod.rs` `error_text`, `log_error_id`); an authz record that does not evaluate logged at error (`authz.rs`); outbound TLS and connect failures logged by each caller (likely); a remote-rest service's TLS failure is logged at ERROR but without its reason: `service/call.rs` drops the error's source chain, so the line says only that the POST failed | `web/tests.rs` `an_unexpected_error_and_a_backend_tls_failure_are_logged_at_error`; `user/authz_tests.rs` `an_authz_record_that_does_not_evaluate_is_logged_at_error_naming_it` (in-process) |

## V16.4 Log Protection

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-16.4.1 | 2 | Log output encoded against log injection | Gap | `logging.rs` `plain_line`, `append_extra` write message and fields as given: a newline starts a new file line (`a_second_line_indents_on_the_console_and_stays_flush_in_the_file`), control characters pass; SEC-FIL-92 open (SSH user name logged as sent) | - |
| v5.0.0-16.4.2 | 2 | Logs protected from reading and tampering | Configuration | files 0600, directory 0700 (`logging.rs` through `moqui_lib::private_fs`); `EntityAuditLog` refuses generic writes (`sensitive.rs` `is_service_written_entity`); the server user can change files and rows, `UserLoginHistory` and `ArtifactAuthzFailure` are generic entities; a tamper-proof copy needs the OTLP export to a separate store | `logging_tests.rs` `the_log_files_are_the_server_users_alone` (in-process) |
| v5.0.0-16.4.3 | 2 | Logs shipped to a separate system | Configuration | `telemetry@otlp-endpoint` (OTLP/HTTP, bounded queue; Monitoring.md, not read in code); default keeps logs on the node and in its own database | `logging_tests.rs` `a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record` (in-process) |

## V16.5 Error Handling

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-16.5.1 | 2 | Generic error answers without internals | Framework | `crates/moqui-core/src/web/mod.rs` `error_text`: on production (the default `instance@purpose`) a 500 answers only `internal error, id ...`; other statuses answer `client_text` (no stack or source lines, `crates/moqui-entity/src/error.rs`); off production a 500 answers its message | V16Logging "v5.0.0-16.5.1: a REST refusal in JSON gives a message, no stack or source location"; V16Logging "v5.0.0-16.5.1: a REST refusal asked for as HTML has no stack frame or script"; V16Logging "v5.0.0-16.5.1: the internal error page shows no stack trace and escapes its message" |
| v5.0.0-16.5.2 | 2 | Safe behavior when external resources fail | Framework | outbound 30 s, 10 s to connect (`crates/moqui-lib/src/http.rs`); OTLP export drops on a full queue and never blocks (Monitoring.md); the failure-tolerance rule (AGENTS.md) (likely) | `moqui-lib` `http_tests.rs` `a_call_to_a_service_that_stops_answering_fails_at_its_timeout`; `a_stalled_collector_never_blocks_local_export` (in-process); PASS: to write: in-process |
| v5.0.0-16.5.3 | 2 | Failures fail closed, never open | Framework | `authz.rs`: a record that does not evaluate denies and never allows; a failing authz service fails the check (`authz_service_type`); a failed service rolls back its transaction | `an_allow_whose_filter_map_fails_does_not_allow`, `a_deny_whose_filter_map_fails_denies_despite_an_allow` (`authz_tests.rs`); `a_body_failure_is_recorded_rolled_back_and_returns_empty_results` (`service/call_tests.rs`) |
