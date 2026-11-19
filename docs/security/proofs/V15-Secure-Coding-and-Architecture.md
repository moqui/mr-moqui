# V15 Secure Coding and Architecture

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V15 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V15.1 Secure Coding and Architecture Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-15.1.1 | 1 | Documented deadlines for patching vulnerable dependencies | Gap | none found: `SECURITY.md`, `docs/Build-Test.md` (Forks, Before a release) set no time frame; `deny.toml` ignores RUSTSEC-2023-0071 with a reason and no date | - |
| v5.0.0-15.1.2 | 2 | Inventory of third-party components from trusted sources | Gap | partly: `Cargo.lock`; `deny.toml` `[sources]` (crates.io and four named forks only); webroot `libs/MANIFEST.json`, `NOTICES.md` (`refresh.mjs`: sha256, npm integrity); no SBOM export; `cargo deny` in neither `ci.xml` nor the release list; SEC-OUT-91 open (downloads checked against a same-place sum or none) | - |
| v5.0.0-15.1.3 | 2 | Resource-heavy features documented with availability defenses | Framework | documentation: Limits columns of `docs/security/Attack-Surface-*.md`; web protection table and `request-timeout` 300 s → 504 (`Attack-Surface-Web.md`); script heap and time caps (`Attack-Surface-Scripts.md`); telemetry queue (`docs/tools/Monitoring.md`); no single list (map) | - (documentation) |

## V15.2 Security Architecture and Dependencies

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-15.2.1 | 1 | Dependencies kept within the patching deadlines | Gap | no deadlines to keep (15.1.1); `cargo deny check` runs in no pipeline (`ci.xml`, pre-production skill); RUSTSEC-2023-0071 (rsa via russh, huskarl) ignored | - |
| v5.0.0-15.2.2 | 2 | Defenses against resource exhaustion by heavy features | Gap | partly: web protection connection, request and overload limits (`crates/moqui-host/src/guard.rs`), `request-timeout`, outbound 30 s and 16 MiB (`crates/moqui-lib/src/http.rs`); per-client rules off until `webapp-list/webapp[webroot]@trusted-proxies` or `web-protection@enabled`; open: SEC-COM-81, SEC-COM-82 (Medium leads) | - |
| v5.0.0-15.2.3 | 2 | No test or development code exposed in production | Configuration | build without `devtools` (`./moq build --enterprise`; Run-Deploy production checklist); dev tools open only off `instance@purpose` production (the default); test hooks gated in code (`service/http_fixtures.rs` `allow`; `test_peer::allow`, `mail_capture::allow` (map)); SEC-OUT-90 open (image script builds with `devtools`) | - |

## V15.3 Defensive Coding

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-15.3.1 | 1 | Answers carry only the fields the caller needs | Application | tool: a service returns only declared out-parameters (`docs/dev/JS-Service-Scripts.md`, Results); generic entity REST drops secret and encrypted fields (`crates/moqui-entity/src/entity/sensitive.rs` `generic_access`) and answers the rest of the row | - |
| v5.0.0-15.3.2 | 2 | Outbound calls do not follow redirects unless intended | Framework | `crates/moqui-lib/src/http.rs` `redirect_policy`: at most 5, same host and port unless `redirects(n, true)`, never https to http, the address guard on each hop; many callers follow none (Attack-Surface-Outbound) | `a_redirect_to_another_host_is_followed_only_when_asked` (`crates/moqui-lib/src/http_tests.rs`) |
| v5.0.0-15.3.3 | 2 | Writes accept only the fields each action intends | Gap | SEC-RTE-81 (High, lead): a posted `_isMulti` with `<name>_0` replaces an id a screen pinned, through a whole-context `service-call`; tools: undeclared in-parameters dropped (`JS-Service-Scripts.md`), `UserAccount` identity fields dropped for non-ADMIN (`sensitive.rs` `remove_user_account_identity`) | PASS: mr-mantle `test/pass/PassAuthority.spec.js` "an operation runs its schema service and a field the schema does not list ..." |
| v5.0.0-15.3.4 | 2 | Client address taken only from trusted proxy headers | Configuration | `webapp-list/webapp[webroot]@client-ip-header` with `@trusted-proxies`; default believes no header; a header with no proxy list is believed from any peer, warned at start (`guard.rs` `log_start`), ignored on link-taking node roles (map) | - |
| v5.0.0-15.3.5 | 2 | Strict typing and comparisons, no type juggling | Application | Rust core statically typed; declared service parameters converted to their `type` (`JS-Service-Scripts.md`); tool for scripts: `moqui lint --groovy` hazards (`crates/moqui-devkit/src/lint_hazards.rs`: decimal operators, entity value identity) | PASS: to write: in-process |
| v5.0.0-15.3.6 | 2 | JavaScript guarded against prototype pollution | Application | tool: host globals and helper objects frozen (`script/oxide/engine.rs` `FREEZE_HOST_GLOBALS_JS`), locked template runtime frozen (Attack-Surface-Scripts); nothing guards a script's own objects; framework scripts not swept (`DataTransferBatch.js` sets a `__proto__` header on a plain object, Admin map) | - |
| v5.0.0-15.3.7 | 2 | Defenses against HTTP parameter pollution | Application | `ec.web.parameters` merges query, form and JSON body: query wins over JSON, a repeated name becomes a list (`crates/moqui-core/src/web/mod.rs` `pairs_to_parameters`); cookies and headers are never parameters; tool: `ec.web.secureRequestParameters` leaves the query out (`secure_parameters`); URL parameters refused on a write transition (CSRF rule, map) | PASS: to write: black-box |
