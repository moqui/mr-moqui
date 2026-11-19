# V1 Encoding and Sanitization

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V1 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V1.1 Encoding and Sanitization Architecture

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-1.1.1 | 2 | Decode input once, before any check | Framework | path names decoded once, an escape in a decoded name not again (`screen/url.rs` `cleanup_path_names`); query and form once (`web/mod.rs` `percent_decode`); XML references once (`moqui-lib/src/node/parse.rs` `decode_entity`) | `the_envelope_gives_each_levels_menu_title` (`moqui-core/src/screen/tests/tui.rs`: `100%25` reads `100%`) |
| v5.0.0-1.1.2 | 2 | Escape output last, at its consumer | Gap | HTML met: `${}` escaped at render (`script/template.rs` `$esc`), widgets at write (`ScreenHtml.js` `e`, `A`); terminal not: text reaches the TUI with its control characters (SEC-CLU-87, SEC-CLI-93, Low, open) | mr-pass Smithy spec "Search is reached by its request path ... it escapes what was asked and what it found ..." |

## V1.2 Injection Prevention

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-1.2.1 | 1 | Encoding fit to HTML, attribute and header contexts | Application | met by default: widget text and attributes (`ScreenHtml.js` `e`, `A`), `.html.fancy`, header values (the `http` builder refuses CR/LF, `moqui-host/src/http.rs` `respond`), `web/mod.rs` `attachment_disposition`; not escaped by default: inline `<text>`, `.fancy`, `encode="false"`; tool `html()` (`docs/dev/JS-Fancy-Templates.md` Escaping) | - |
| v5.0.0-1.2.2 | 1 | Encoded values and safe schemes in built URLs | Gap | seen, not in the register: `screen/urls.rs` `parameter_string` uses `moqui_lib::string::url_encode_if_needed`, which leaves text that looks encoded (`%XX`, `+`) as is, so the target decodes it once more (low impact); redirects refuse `javascript:`, `data:` (`web/mod.rs` `is_same_origin_redirect`); a `url-type="plain"` link takes any scheme (`urls.rs` `make_url_by_type`) and the default CSP has no `script-src` | - |
| v5.0.0-1.2.3 | 1 | Encoding for data put into script or JSON | Gap | SEC-TLS-115 (Info: `moqui-tools/src/etl/service.rs` audit JSON built by format string); met: shells write JSON with `<` as `<` (`webroot/screen/includes/PublicShell.html.fancy`, `WebrootVue.qvt.html.fancy`), `encodeHtmlJsSafe` (`script/runtime/encode.rs`), answers by serde_json | - |
| v5.0.0-1.2.4 | 1 | Bound parameters against SQL and query injection | Framework | every value a `?` (`moqui-db/src/sql/condition.rs` `Condition`), unknown select and order fields dropped (`moqui-entity/src/entity/find.rs`), identifiers quoted (`sql/ident.rs` `quote_ident`); raw SQL only by `makeConditionWhere` and the SQL tools (stored SQL: SEC-TLS-88, SEC-TLS-100, open) | V1Encoding "v5.0.0-1.2.4: a SQL-looking value is stored and read back as data"; `postgres_enumeration_insert_select_and_count` (`moqui-db/src/sql/tests.rs`) |
| v5.0.0-1.2.5 | 1 | No shell command injection | Framework | programs started with argument lists; the two shells quote (`moqui-host/src/pdf.rs` `"$0" "$@"`, `search.rs` `opensearch_command`); no script starts a process (SEC-SCR-04, SEC-TLS-02, SEC-OUT-08 fixed); CI steps run a repository's commands by design; SEC-TLS-86 (open: read-only SQL as the embedded superuser) | `no_script_starts_a_process_and_the_registry_token_needs_the_conf_and_the_permission` (`moqui-core/src/service/registry_token_tests.rs`) |
| v5.0.0-1.2.6 | 2 | Directory (LDAP) query injection | N/A | no LDAP client in the code or `Cargo.lock` | - |
| v5.0.0-1.2.7 | 2 | XML path query injection | N/A | no XPath evaluator; only the embedded PostgreSQL's libxml2 (`moqui-drivers/src/xmlshim.rs` symbol list) | - |
| v5.0.0-1.2.8 | 2 | TeX processor hardening | N/A | no LaTeX processor | - |
| v5.0.0-1.2.9 | 2 | Regex metacharacters escaped when patterns use data | Application | tool: none for scripts (Rust code uses `regex::escape`); a request's pattern goes to the linear `regex` crate (`script/host/user.rs` `key_matcher`, SEC-RTE-15 fixed); `matches` is a linear NFA (`moqui-lib/src/regex_match.rs`) | mr-pass Moderation spec "rules test safely and block, hold, flag or censor posts" |

## V1.3 Sanitization

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-1.3.1 | 1 | Rich-text HTML cleaned by a vetted sanitizer | Framework | `moqui-lib/src/safe_html.rs` `clean` (ammonia defaults); a service parameter refuses `<` unless `allow-html`, `safe` cleans (`service/validate.rs` `clean_html`); kept `<img src>` gives SEC-CLI-82 (open) | `the_rich_text_editor_output_survives_the_safe_cleaning` (`service/validate.rs`); mr-pass Forum "write is idempotent and body is sanitized" |
| v5.0.0-1.3.2 | 1 | No eval or dynamic code over user input | Gap | SEC-SCR-81 (High, open: a wiki page of a `fancy` type runs as server script); met: record expressions need `DATABASE_ADMIN` (SEC-SCR-06 fixed), `filterMap` read as JSON (SEC-SCR-01 fixed); `eval` remains for server scripts, which are code | to write: in-process |
| v5.0.0-1.3.3 | 2 | Character and length limits before risky sinks | Application | tools: service parameter checks (`service/validate.rs`: `allow-html`, `Check::TextLength`, `matches`, `number-range`); `script/runtime/encode.rs` `url_safe_chars` | mr-pass Marketing SubmitForm "an answer is kept once; over the client's limit, oversized or naming an unknown field ..." |
| v5.0.0-1.3.4 | 2 | Scriptable SVG from users cleaned | Framework | a user's SVG goes as an attachment with `sandbox` (`web/mod.rs` `user_file_inline`, `is_active_content_type`; DAV SEC-FIL-03 fixed); rich-text images refuse SVG (`service/images.rs` `upload_rich_text_image`); `script/theme/svg.rs` `sanitize_svg` for apps | `a_users_file_asked_inline_is_shown_only_when_passive_and_then_sandboxed` (`moqui-core/src/web/tests.rs`); `svg_and_css_assets` (`script/theme/tests.rs`) |
| v5.0.0-1.3.5 | 2 | User Markdown, CSS and similar markup made safe | Gap | SEC-SCR-81 (High: an `md` wiki page keeps raw HTML, `moqui-lib/src/markdown.rs` passes it); SEC-SCR-90 (Low: a CSS comment hides `url()` from the theme check); `markdownHtml` output uncleaned, callers clean; no XSLT or BBCode | - |
| v5.0.0-1.3.6 | 2 | Outbound request forgery: destinations held to a list | Gap | SEC-RTE-87, SEC-TLS-101, SEC-OUT-84 (Low, open); met: `moqui-lib/src/net.rs` `internal_address` guard, on for resource URLs and script `rest()` under `instance@purpose="production"` (the default), link-local always (`outbound.rs`), stored rows by `deny_private_network` | - |
| v5.0.0-1.3.7 | 2 | Templates never assembled from untrusted text | Gap | SEC-SCR-81 (High: a wiki page's type picks the trusted renderer), SEC-SCR-92 (Low); met: `${` in record templates needs `DATABASE_ADMIN` (SEC-SCR-05, -06, -12 fixed), the locked renderer (`script/template.rs` `render_locked_bounded`) | mr-mantle ThemeAuthoring "a script or an onclick in a template is refused with its file and line ..."; `urls_forms_and_assembled_expressions` (`script/theme/tests.rs`) |
| v5.0.0-1.3.8 | 2 | Java naming lookup injection | N/A | no Java and no JNDI lookup; `server-jndi` and `jndi-jdbc` are only merged as conf (`context/conf.rs`) | - |
| v5.0.0-1.3.9 | 2 | Cache server (memcache) injection | N/A | no memcache or Redis client (`Cargo.lock`); caches are in process | - |
| v5.0.0-1.3.10 | 2 | Format strings from untrusted text | Framework | Rust format strings are compile-time literals; scripts have no printf; `ec.l10n.format` parses a number or date pattern and returns a string as it is (`l10n.rs` `format_value`) | to write: in-process |
| v5.0.0-1.3.11 | 2 | Mail header and mailbox command injection | Framework | messages built by lettre; a call's subject made one line, a From, name or bounce address with CR/LF dropped (`moqui-mail/src/implementation.rs` `CallHeaders::of`), extra headers checked; IMAP strings quoted (`sync.rs` `quoted`); a template's expanded subject relies on lettre's header encoding (likely) | `a_call_gives_its_subject_sender_relay_and_bounce_address`, `a_template_send_adds_the_list_headers_and_keeps_them` (`moqui-mail/src/implementation_tests.rs`) |

## V1.4 Memory, String, and Unmanaged Code

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-1.4.1 | 2 | Memory-safe string and buffer handling | Framework | Rust; server `unsafe` only in `moqui-host` `detach.rs`, `systemd_notify.rs` and the `moqui-drivers` `mysql.rs` FFI; the script engine quickjs-oxide is `#![forbid(unsafe_code)]`; PostgreSQL is its own process | to write: in-process |
| v5.0.0-1.4.2 | 2 | Integer overflow avoided | Framework | Rust overflow is defined, never memory unsafe; money and quantities are `rust_decimal`, overflow an error (`moqui-lib/src/decimal_expr.rs`); release has no `overflow-checks` (`.cargo/config.toml`), so plain arithmetic wraps; not swept (likely) | to write: in-process |
| v5.0.0-1.4.3 | 2 | No use after free or dangling pointers | Framework | Rust ownership; the same `unsafe` sites as 1.4.1 | to write: in-process |

## V1.5 Safe Deserialization

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-1.5.1 | 1 | XML parsers refuse external entities | Framework | `moqui-lib/src/node/parse.rs` refuses DOCTYPE, ENTITY and other declarations (`declaration_error`), decodes the five named and numeric references only; `xml_edit` never expands; IronCalc drops a DOCTYPE (map) | `doctype_and_unknown_entity_report_line_and_column` (`moqui-lib/src/node/tests.rs`) |
| v5.0.0-1.5.2 | 2 | Untrusted data deserialized safely | Framework | JSON into plain values or fixed structs (serde_json, depth 128), gRPC by prost; no polymorphic or binary object format in `Cargo.lock` (no bincode, typetag, YAML); SEC-TLS-95 (a Yjs update crashes the markdown writer, open) is availability | to write: in-process |
