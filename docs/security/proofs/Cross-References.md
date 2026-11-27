# Cross-references

The [matrix](README.md) is filed by OWASP ASVS 5.0.0. This file maps five other standards onto it: for each item of
the other standard, the ASVS 5.0.0 requirements that answer to it, each with the framework's status and proof as the
chapter files give them. A reader who works from one of these standards finds here where mr-moqui stands, and
follows the requirement to its chapter file for the evidence. The plan is
[Proofs-Initial](../../../plans/Proofs-Initial.md), phase P6.

Status 2026-11-19: statuses and proofs are those of the chapter files at mr-moqui `cc338a81f`. Where a chapter file
has changed since, the chapter file is right.

Sections: [the standards](#the-standards) · [where the mappings come from](#where-the-mappings-come-from) ·
[reading the tables](#reading-the-tables) · [OWASP Top 10](#owasp-top-102025) ·
[API Security Top 10](#owasp-api-security-top-10-2023) · [Top 10 for LLM Applications](#owasp-top-10-for-llm-applications-2025)
· [NIST SP 800-63B-4](#nist-sp-800-63b-4) · [CWE Top 25](#cwe-top-25-2025) · [thin spots](#thin-spots) ·
[notices](#notices)

## The standards

Checked 2026-11-19 on each standard's own page.

| Standard | Version | License |
| --- | --- | --- |
| [OWASP Top 10](https://top10.owasp.org/2025) | 2025, final | CC BY 3.0, as the site states |
| [OWASP API Security Top 10](https://api-security.owasp.org/editions/2023/en/0x11-t10/) | 2023, still the latest | CC BY-SA 4.0, as the [project page](https://owasp.org/www-project-api-security/) states (the edition's pages state none) |
| [OWASP Top 10 for LLM Applications](https://genai.owasp.org/llm-top-10/) | 2025, still the latest | CC BY-SA 4.0, as the site states |
| [NIST SP 800-63B-4](https://pages.nist.gov/800-63-4/sp800-63b.html) | final, July 2025 ([CSRC](https://csrc.nist.gov/pubs/sp/800/63/b/4/final), [doi:10.6028/NIST.SP.800-63b-4](https://doi.org/10.6028/NIST.SP.800-63b-4)); it supersedes SP 800-63B | a work of the US government, not subject to copyright in the United States ([NIST](https://www.nist.gov/open/copyright-fair-use-and-licensing-statements-srd-data-software-and-technical-series-publications)) |
| [OWASP ASVS](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0), the matrix's spine | 5.0.0 (May 2025) | CC BY-SA 4.0 |
| [CWE Top 25](https://cwe.mitre.org/top25/archive/2025/2025_cwe_top25.html) | 2025, published December 2025; the latest on [the Top 25 page](https://cwe.mitre.org/top25/) | royalty-free use under MITRE's [terms of use](https://cwe.mitre.org/about/termsofuse.html), with its copyright notice (see [notices](#notices)) |

This file copies from each standard only its item ids, its item names as the standard lists them, and links. Every
other word is ours, so the file is CC0 like the rest of mr-moqui. A standard's own text is behind its link.

## Where the mappings come from

ASVS 5.0.0 publishes no mapping to any of these five. Its chapter for users of 4.0 says it dropped the direct CWE and
NIST mappings of 4.x and means to link other standards through the OWASP Common Requirement Enumeration
([For users of 4.0](https://github.com/OWASP/ASVS/blob/v5.0.0/5.0/en/0x05-For-Users-Of-4.0.md)). Two pieces remain,
and this file uses both:

- Its V6 chapter points sections at sections of NIST SP 800-63B revision 3 (V6.2 at §5.1.1.2, V6.5 at §5.1.2 to
  §5.1.5.2, V6.6 at §5.1.3.3, and others). The NIST table follows those pointers to the matching sections of
  revision 4 and adds the rows they leave out.
- Its repository keeps an export, made before the release, of CWE ids for the draft's numbering
  ([`v5.0.be_cwe_mapping.json`](https://github.com/OWASP/ASVS/blob/v5.0.0/5.0/mappings/v5.0.be_cwe_mapping.json)),
  dropped from the release. Translated to 5.0.0 ids with the repository's own
  [`mapping_v5.0.be_to_v5.0.0.yml`](https://github.com/OWASP/ASVS/blob/v5.0.0/5.0/mappings/mapping_v5.0.be_to_v5.0.0.yml),
  it is shown beside the CWE table as a check; a draft requirement may differ from the released one.

Everything else is our mapping, made by reading each ASVS requirement against the other standard's item: the Top 10,
the API and LLM Top 10s entirely, the CWE rows the export does not give, and the NIST sections beyond ASVS's pointers.

## Reading the tables

One row for each pair of an item and a requirement; the item's name and the closing note are on its first row. A
requirement may answer to several items. The matrix holds levels 1 and 2 only; a level 3 counterpart is named in the
note. A matrix row that answers to no item most directly (most of V10 and V17, among others) is only in its chapter
file.

| Column | Holds |
| --- | --- |
| Item | The standard's id and name. |
| Requirement | The ASVS 5.0.0 id, linked to its chapter file, where the topic, evidence and register entries are. |
| Status | The framework's status from the chapter file: Framework, Configuration, Application, Gap or N/A ([statuses](README.md#statuses)). |
| Proof | The framework's proof from the row's Proof cell, shortened (below). |
| Where it is thin | What a reader of that standard should know of mr-moqui's coverage, in our words. |

In the Proof column:

- `V8Authorization` and the like: a black-box proof spec in
  [`runtime/components/base/framework/test/proofs/`](../../../runtime/components/base/framework/test/proofs); its
  features for the requirement start with the requirement's id.
- *(deployment)*: the proof checks a setting and runs only against a deployment; *opt-in* needs a `--param` (see the
  [index](README.md#running-the-proofs)).
- a name with *(spec)*: an in-process framework spec in `runtime/components/base/framework/test/`.
- a `snake_case` name: a Rust test, with its file.
- *part*: the test shows only part of the requirement; for a Gap or Application row, the part that is met.
- *regression test*: a `test/client` spec the row cites, not filed as a proof.
- *document*: a documentation requirement, which no test shows.
- *none*: the framework has no proof of the row; *to write* where the chapter file says one is to be written.

PASS's statuses and proofs are in its own matrix and are not repeated here, except where a note names one.

## OWASP Top 10:2025

The ten categories, each to the requirements that answer most directly. A requirement that fits two categories is
listed under both.

| Item | Requirement | Status | Proof | Where it is thin |
| --- | --- | --- | --- | --- |
| A01:2025 Broken Access Control | [v5.0.0-8.1.1](V8-Authorization.md) | Application | document | Deny by default (8.2.1), record access (8.2.2) and field access (8.2.3) are Gaps with open register entries. Outbound requests have an internal-address guard but no allowlist (1.3.6). A same-origin GET with no parameters still runs an acting transition (3.5.3). |
|  | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none |  |
|  | [v5.0.0-8.2.2](V8-Authorization.md) | Gap | none |  |
|  | [v5.0.0-8.2.3](V8-Authorization.md) | Gap | none |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |
|  | [v5.0.0-8.4.1](V8-Authorization.md) | Application | none |  |
|  | [v5.0.0-3.4.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-3.5.1](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-3.5.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-3.5.3](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.7.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-4.4.2](V4-API-and-Web-Service.md) | Framework | `V4ApiWebService` |  |
|  | [v5.0.0-5.3.2](V5-File-Handling.md) | Gap | none |  |
|  | [v5.0.0-1.3.6](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-13.2.5](V13-Configuration.md) | Configuration | `an_http_location_reaches_an_internal_address_only_off_production` (`context/resource/tests.rs`) |  |
|  | [v5.0.0-15.3.2](V15-Secure-Coding-and-Architecture.md) | Framework | `a_redirect_to_another_host_is_followed_only_when_asked` (`moqui-lib` `http_tests.rs`) |  |
| A02:2025 Security Misconfiguration | [v5.0.0-13.4.1](V13-Configuration.md) | Configuration | `V13Configuration`; `traversal_hidden_and_odd_names_are_refused` (`static_files.rs`) | The browser headers are sent on screen answers only, not on host routes (3.4.1, 3.4.4, 3.4.6), and the default CSP has no source allowlist (3.4.3). Only the session cookie has a prefix, and only over https (3.3.1, 3.3.3). TRACE and the referrer policy need a setting. Every pool of the embedded database is its superuser (13.2.2, SEC-TLS-86). |
|  | [v5.0.0-13.4.2](V13-Configuration.md) | Framework | part: `V16Logging` (its 16.5.1 feature) |  |
|  | [v5.0.0-13.4.3](V13-Configuration.md) | Framework | `V13Configuration` |  |
|  | [v5.0.0-13.4.4](V13-Configuration.md) | Configuration | `V13Configuration` (deployment) |  |
|  | [v5.0.0-13.4.5](V13-Configuration.md) | Framework | `V13Configuration` |  |
|  | [v5.0.0-15.2.3](V15-Secure-Coding-and-Architecture.md) | Configuration | `V15SecureCoding` |  |
|  | [v5.0.0-13.2.2](V13-Configuration.md) | Gap | none |  |
|  | [v5.0.0-13.2.3](V13-Configuration.md) | Framework | `generated_secrets_are_32_random_bytes_and_secret_names_are_known` (`context/tests.rs`) |  |
|  | [v5.0.0-6.3.2](V6-Authentication.md) | Framework | `V6Authentication`; `only_a_loopback_peer_with_no_proxy_header_is_local` (`setup_tests.rs`) |  |
|  | [v5.0.0-1.5.1](V1-Encoding-and-Sanitization.md) | Framework | `doctype_and_unknown_entity_report_line_and_column` (`moqui-lib` `node/tests.rs`) |  |
|  | [v5.0.0-3.3.1](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.3.3](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.3.4](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-3.4.1](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.4.3](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.4.4](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.4.5](V3-Web-Frontend-Security.md) | Configuration | `V3WebFrontend` (deployment) |  |
|  | [v5.0.0-3.4.6](V3-Web-Frontend-Security.md) | Gap | none |  |
| A03:2025 Software Supply Chain Failures | [v5.0.0-15.1.1](V15-Secure-Coding-and-Architecture.md) | Gap | none | No patching deadlines, no SBOM export, and `cargo deny` runs in no pipeline. Integrity of externally hosted assets (3.6.1) and pinned repositories (15.2.4) are level 3, outside the matrix. |
|  | [v5.0.0-15.1.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
|  | [v5.0.0-15.2.1](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
|  | [v5.0.0-3.7.1](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
| A04:2025 Cryptographic Failures | [v5.0.0-11.1.1](V11-Cryptography.md) | Gap | none | The weakest area of the matrix: stored field encryption is AES-128-CBC with a constant IV and no MAC, its key comes from 10 PBKDF2 iterations, and DES and MD5 still decrypt (the `known_gap_` tests in `crypto_tests.rs`). The QZ Tray signature is SHA-1. Key management is not written down, and secrets sit in `runtime/.env` with no vault (13.3.1). Transport is proved; plain http is the default until the deployer sets TLS. |
|  | [v5.0.0-11.1.2](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.2.1](V11-Cryptography.md) | Framework | `cryptography_comes_from_the_validated_libraries_and_none_is_home_made` (`tls_tests.rs`) |  |
|  | [v5.0.0-11.2.2](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.2.3](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.3.1](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.3.2](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.3.3](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.4.1](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.4.2](V11-Cryptography.md) | Framework | `hash_password_is_an_argon2id_phc_string` (`password.rs`) |  |
|  | [v5.0.0-11.4.3](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.4.4](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.5.1](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-11.6.1](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-12.1.1](V12-Secure-Communication.md) | Framework | `the_https_listener_speaks_tls_13_and_12_only_preferring_13` (`tls_tests.rs`) |  |
|  | [v5.0.0-12.1.2](V12-Secure-Communication.md) | Framework | `the_https_listener_enables_only_aead_suites_with_forward_secrecy` (`tls_tests.rs`) |  |
|  | [v5.0.0-12.1.3](V12-Secure-Communication.md) | Framework | `a_member_without_the_cluster_certificate_is_refused` (`cluster_edge/tests.rs`) |  |
|  | [v5.0.0-12.2.1](V12-Secure-Communication.md) | Configuration | `V12SecureCommunication` (deployment) |  |
|  | [v5.0.0-12.2.2](V12-Secure-Communication.md) | Configuration | `V12SecureCommunication` (deployment) |  |
|  | [v5.0.0-12.3.1](V12-Secure-Communication.md) | Configuration | `the_cluster_port_speaks_only_tls_and_no_sender_falls_back_to_clear_text` (`cluster_edge/tests.rs`) and five more |  |
|  | [v5.0.0-12.3.2](V12-Secure-Communication.md) | Configuration | `the_outbound_http_client_checks_the_servers_certificate_and_name` (`tls_tests.rs`) |  |
|  | [v5.0.0-12.3.3](V12-Secure-Communication.md) | Configuration | `the_cluster_port_speaks_only_tls_and_no_sender_falls_back_to_clear_text` (`cluster_edge/tests.rs`) |  |
|  | [v5.0.0-12.3.4](V12-Secure-Communication.md) | Gap | none |  |
|  | [v5.0.0-4.4.1](V4-API-and-Web-Service.md) | Configuration | `V4ApiWebService` (deployment) |  |
|  | [v5.0.0-13.3.1](V13-Configuration.md) | Gap | none |  |
| A05:2025 Injection | [v5.0.0-1.1.1](V1-Encoding-and-Sanitization.md) | Framework | `the_envelope_gives_each_levels_menu_title` (`screen/tests/tui.rs`) | Escaping is on by default, but raw `<text>`, `.fancy` and `encode="false"` output is the builder's (1.2.1). URL building (1.2.2), JSON in scripts (1.2.3), Markdown and CSS (1.3.5) and templates from stored text (1.3.7) are Gaps, and terminal output keeps control characters (1.1.2). Formula injection (1.2.10) is level 3. |
|  | [v5.0.0-1.1.2](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-1.2.1](V1-Encoding-and-Sanitization.md) | Application | none |  |
|  | [v5.0.0-1.2.2](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-1.2.3](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-1.2.4](V1-Encoding-and-Sanitization.md) | Framework | `V1Encoding`; `postgres_enumeration_insert_select_and_count` (`moqui-db` `sql/tests.rs`) |  |
|  | [v5.0.0-1.2.5](V1-Encoding-and-Sanitization.md) | Framework | `no_script_starts_a_process` (`framework_gaps_tests.rs`) |  |
|  | [v5.0.0-1.2.6](V1-Encoding-and-Sanitization.md) | N/A | - |  |
|  | [v5.0.0-1.2.7](V1-Encoding-and-Sanitization.md) | N/A | - |  |
|  | [v5.0.0-1.2.8](V1-Encoding-and-Sanitization.md) | N/A | - |  |
|  | [v5.0.0-1.2.9](V1-Encoding-and-Sanitization.md) | Application | none |  |
|  | [v5.0.0-1.3.1](V1-Encoding-and-Sanitization.md) | Framework | `the_rich_text_editor_output_survives_the_safe_cleaning` (`service/validate.rs`) |  |
|  | [v5.0.0-1.3.2](V1-Encoding-and-Sanitization.md) | Framework | `WikiPageTypes`, `DocumentExpression` (specs) |  |
|  | [v5.0.0-1.3.3](V1-Encoding-and-Sanitization.md) | Application | none |  |
|  | [v5.0.0-1.3.4](V1-Encoding-and-Sanitization.md) | Framework | `a_users_file_asked_inline_is_shown_only_when_passive_and_then_sandboxed` (`web/tests.rs`) |  |
|  | [v5.0.0-1.3.5](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-1.3.7](V1-Encoding-and-Sanitization.md) | Gap | part: `urls_forms_and_assembled_expressions` (`script/theme/tests.rs`) |  |
|  | [v5.0.0-1.3.8](V1-Encoding-and-Sanitization.md) | N/A | - |  |
|  | [v5.0.0-1.3.9](V1-Encoding-and-Sanitization.md) | N/A | - |  |
|  | [v5.0.0-1.3.10](V1-Encoding-and-Sanitization.md) | Framework | `a_format_string_from_untrusted_text_resolves_to_nothing_but_its_text` (`l10n_tests.rs`) |  |
|  | [v5.0.0-1.3.11](V1-Encoding-and-Sanitization.md) | Framework | `a_call_gives_its_subject_sender_relay_and_bounce_address` (`moqui-mail` `implementation_tests.rs`) |  |
|  | [v5.0.0-3.2.2](V3-Web-Frontend-Security.md) | Application | none (to write) |  |
| A06:2025 Insecure Design | [v5.0.0-2.1.1](V2-Validation-and-Business-Logic.md) | Application | none | Business rules are the application's; the framework gives the tools (service parameter checks, status flows, `forUpdate`, `record#Attempt`, tarpits). Per-client rate limits need web protection on. |
|  | [v5.0.0-2.1.2](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.1.3](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.2.3](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.3.1](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.3.2](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.3.3](V2-Validation-and-Business-Logic.md) | Framework | `a_body_failure_is_recorded_rolled_back_and_returns_empty_results` (`service/call_tests.rs`) |  |
|  | [v5.0.0-2.3.4](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.4.1](V2-Validation-and-Business-Logic.md) | Configuration | `V2Validation` (deployment, opt-in) |  |
|  | [v5.0.0-15.1.3](V15-Secure-Coding-and-Architecture.md) | Framework | document |  |
|  | [v5.0.0-15.2.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
| A07:2025 Authentication Failures | [v5.0.0-6.2.1](V6-Authentication.md) | Framework | `V6Authentication`; `password_acceptable_rejects_secret_and_accepts_a_mixed_password` (`policy.rs`) | All of V6 and V7 answers to A07, and the OAuth and OIDC client rows of V10; these are the rows that bear most, and the others are in the NIST table below and the chapter files. No common or breached password screening (6.2.4, 6.2.12) and composition rules on by default (6.2.5). No absolute session lifetime (7.3.2). A re-authentication keeps the session id (7.2.4). |
|  | [v5.0.0-6.2.4](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.2.5](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.2.12](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.3.1](V6-Authentication.md) | Configuration | `V6Authentication` (deployment, opt-in); regression test: `SecurityAuthn` (the account lock) |  |
|  | [v5.0.0-6.3.2](V6-Authentication.md) | Framework | `V6Authentication`; `only_a_loopback_peer_with_no_proxy_header_is_local` (`setup_tests.rs`) |  |
|  | [v5.0.0-6.3.3](V6-Authentication.md) | Configuration | `V6Authentication` (deployment) |  |
|  | [v5.0.0-6.3.4](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.4.1](V6-Authentication.md) | Framework | `ResetPassword` (spec); `the_old_password_of_a_change_is_behind_the_login_gate` (`login_tests.rs`) |  |
|  | [v5.0.0-6.4.3](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.5.2](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.6.3](V6-Authentication.md) | Framework | `V6Authentication` (wrong codes at once); regression test: `SecurityAuthn` (wrong codes in turn) |  |
|  | [v5.0.0-6.8.2](V6-Authentication.md) | Gap | part: `an_id_token_is_taken_only_when_its_signature_is_there_and_verifies` (`sso_tests.rs`) |  |
|  | [v5.0.0-7.2.1](V7-Session-Management.md) | Framework | `V7SessionManagement`; `a_bad_mac_is_an_unknown_session` (`session_tests.rs`) |  |
|  | [v5.0.0-7.2.4](V7-Session-Management.md) | Gap | none |  |
|  | [v5.0.0-7.3.2](V7-Session-Management.md) | Gap | none |  |
|  | [v5.0.0-7.4.1](V7-Session-Management.md) | Framework | `V7SessionManagement`; `a_logout_ends_the_session_and_its_attributes` (`session_tests.rs`) |  |
|  | [v5.0.0-7.4.2](V7-Session-Management.md) | Framework | `every_session_of_a_user_ends_and_a_disabled_account_signs_out` (`session_tests.rs`) |  |
|  | [v5.0.0-7.5.1](V7-Session-Management.md) | Gap | none |  |
| A08:2025 Software or Data Integrity Failures | [v5.0.0-1.5.2](V1-Encoding-and-Sanitization.md) | Framework | `untrusted_data_is_deserialized_only_into_plain_values_and_fixed_types` (`web/tests.rs`) | Stored ciphertext is not authenticated (11.3.3). `/rest/sm` takes any message type on one remote's check (9.2.2, SEC-RTE-86). The mail client reads its ID token unsigned (SEC-MAIL-98). |
|  | [v5.0.0-15.3.3](V15-Secure-Coding-and-Architecture.md) | Framework | `MultiPin` (spec) |  |
|  | [v5.0.0-9.1.1](V9-Self-contained-Tokens.md) | Framework | `V9SelfContainedTokens`; `tokens_are_signed_and_expire` (`lfs.rs`); `a_request_state_is_refused_on_tamper_expiry_another_principal_or_another_request` (`mcp/tests.rs`) |  |
|  | [v5.0.0-9.1.2](V9-Self-contained-Tokens.md) | Framework | `an_id_token_is_taken_only_signed_with_an_algorithm_of_the_allowlist_never_none` (`sso_tests.rs`) |  |
|  | [v5.0.0-9.1.3](V9-Self-contained-Tokens.md) | Framework | `the_keys_of_an_id_token_come_only_from_the_providers_jwks_never_its_header` (`sso_tests.rs`) |  |
|  | [v5.0.0-9.2.2](V9-Self-contained-Tokens.md) | Gap | none |  |
|  | [v5.0.0-11.3.3](V11-Cryptography.md) | Gap | none |  |
|  | [v5.0.0-3.5.5](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
| A09:2025 Security Logging and Alerting Failures | [v5.0.0-16.1.1](V16-Security-Logging-and-Error-Handling.md) | Gap | none | No log inventory (16.1.1). A sign-in is logged once a minute per user, never for an unknown user, and with no method (16.3.1). Log lines are not encoded against injection (16.4.1). ASVS reaches alerting only through shipping logs to a separate system (16.4.3). |
|  | [v5.0.0-16.2.1](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
|  | [v5.0.0-16.2.2](V16-Security-Logging-and-Error-Handling.md) | Configuration | `a_log_lines_stamp_is_its_instant_in_utc` (`logging_tests.rs`) |  |
|  | [v5.0.0-16.2.3](V16-Security-Logging-and-Error-Handling.md) | Framework | `log_lines_are_written_only_to_the_named_files` (`logging_tests.rs`) |  |
|  | [v5.0.0-16.2.4](V16-Security-Logging-and-Error-Handling.md) | Framework | `a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record` (`logging_tests.rs`) |  |
|  | [v5.0.0-16.2.5](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
|  | [v5.0.0-16.3.1](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
|  | [v5.0.0-16.3.2](V16-Security-Logging-and-Error-Handling.md) | Framework | `a_failed_check_leaves_an_authz_failure_row` (`authz_tests.rs`) |  |
|  | [v5.0.0-16.3.3](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
|  | [v5.0.0-16.3.4](V16-Security-Logging-and-Error-Handling.md) | Framework | `an_unexpected_error_and_a_backend_tls_failure_are_logged_at_error` (`web/tests.rs`) |  |
|  | [v5.0.0-16.4.1](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
|  | [v5.0.0-16.4.2](V16-Security-Logging-and-Error-Handling.md) | Configuration | `the_log_files_are_the_server_users_alone` (`logging_tests.rs`) |  |
|  | [v5.0.0-16.4.3](V16-Security-Logging-and-Error-Handling.md) | Configuration | `a_logged_line_reaches_a_configured_collector_as_an_opentelemetry_log_record` (`logging_tests.rs`) |  |
| A10:2025 Mishandling of Exceptional Conditions | [v5.0.0-16.5.1](V16-Security-Logging-and-Error-Handling.md) | Framework | `V16Logging` | Generic error answers, rollback and fail-closed authorization are proved. The Gaps are resource bounds: a multipart upload held in memory (SEC-RTE-82), nested archives in the loader (SEC-DAT-91), the media and signaling paths (SEC-COM-87, SEC-COM-82). A last-resort handler (16.5.4) is level 3. |
|  | [v5.0.0-16.5.2](V16-Security-Logging-and-Error-Handling.md) | Framework | `a_call_to_a_service_that_stops_answering_fails_at_its_timeout` (`moqui-lib` `http_tests.rs`) |  |
|  | [v5.0.0-16.5.3](V16-Security-Logging-and-Error-Handling.md) | Framework | `an_allow_whose_filter_map_fails_does_not_allow` (`authz_tests.rs`) |  |
|  | [v5.0.0-2.3.3](V2-Validation-and-Business-Logic.md) | Framework | `a_body_failure_is_recorded_rolled_back_and_returns_empty_results` (`service/call_tests.rs`) |  |
|  | [v5.0.0-13.4.2](V13-Configuration.md) | Framework | part: `V16Logging` (its 16.5.1 feature) |  |
|  | [v5.0.0-15.2.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
|  | [v5.0.0-5.2.1](V5-File-Handling.md) | Gap | none |  |
|  | [v5.0.0-5.2.3](V5-File-Handling.md) | Gap | none |  |
|  | [v5.0.0-17.2.4](V17-WebRTC.md) | Gap | none |  |
|  | [v5.0.0-17.3.2](V17-WebRTC.md) | Gap | none |  |

## OWASP API Security Top 10 (2023)

Read for the framework's REST surfaces: service REST `/rest/s1` and entity REST `/rest/e1` (with `/rest/m1`, and
`/rest/v1` as a deprecated alias), on by default; the PASS authority's REST form
`/rest/s1/pass/v1/sites/{siteId}/{route}`, whenever mr-mantle is loaded; and `/a2a`, `/mcp` and the model gateway,
off by default. Their rules are in [Attack-Surface-Routes](../Attack-Surface-Routes.md),
[Attack-Surface-Apps-PASS](../../../runtime/components/addon/mr-pass/docs/security/Attack-Surface-Apps-PASS.md#how-pass-reaches-the-authority) and
[Attack-Surface-AI](../Attack-Surface-AI.md). A note marked *(map)* is taken from those files and is not a proof.

| Item | Requirement | Status | Proof | Where it is thin |
| --- | --- | --- | --- | --- |
| API1:2023 Broken Object Level Authorization | [v5.0.0-8.2.2](V8-Authorization.md) | Gap | none | Entity REST rows are held to the caller only by `EntityFilterSet` row filters and checks in services, which the application writes. A2A tasks and MCP approvals are found by the caller's user id (map, not a proof). The PASS authority holds a delegation to its client and site (mr-mantle `PassAuthority`). |
|  | [v5.0.0-8.4.1](V8-Authorization.md) | Application | none |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |
| API2:2023 Broken Authentication | [v5.0.0-6.3.1](V6-Authentication.md) | Configuration | `V6Authentication` (deployment, opt-in); regression test: `SecurityAuthn` (the account lock) | `/a2a`, `/mcp` and the gateway refuse the account password and take credentials only over TLS or from this machine (map). The per-client sign-in limit needs web protection and counts POST only (SEC-WEB-89). Keys and app passwords ask no second factor (6.3.4). A password in the query string signs in (14.2.1). None of these servers takes OAuth access tokens (10.3, N/A). |
|  | [v5.0.0-6.3.4](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-7.2.2](V7-Session-Management.md) | Framework | `V7SessionManagement` |  |
|  | [v5.0.0-9.1.1](V9-Self-contained-Tokens.md) | Framework | `V9SelfContainedTokens`; `tokens_are_signed_and_expire` (`lfs.rs`); `a_request_state_is_refused_on_tamper_expiry_another_principal_or_another_request` (`mcp/tests.rs`) |  |
|  | [v5.0.0-9.2.1](V9-Self-contained-Tokens.md) | Framework | `V9SelfContainedTokens`; `tokens_are_signed_and_expire` (`lfs.rs`) |  |
|  | [v5.0.0-14.2.1](V14-Data-Protection.md) | Gap | none |  |
| API3:2023 Broken Object Property Level Authorization | [v5.0.0-8.1.2](V8-Authorization.md) | Application | none | Entity REST drops password and secret fields and answers the rest of the row (15.3.1). `PATCH /rest/s1/moqui/users/{userId}` writes fields past the recent sign-in guard (SEC-ADM-81). The PASS authority answers a field its schema does not list with 422 (mr-mantle `PassAuthority`). |
|  | [v5.0.0-8.2.3](V8-Authorization.md) | Gap | none |  |
|  | [v5.0.0-15.3.1](V15-Secure-Coding-and-Architecture.md) | Application | none |  |
|  | [v5.0.0-15.3.3](V15-Secure-Coding-and-Architecture.md) | Framework | `MultiPin` (spec) |  |
| API4:2023 Unrestricted Resource Consumption | [v5.0.0-2.4.1](V2-Validation-and-Business-Logic.md) | Configuration | `V2Validation` (deployment, opt-in) | Per-client limits are off until web protection is on. The A2A, MCP and gateway bodies (4, 4 and 16 MiB) are read before the login (SEC-AI-88); messages to a working A2A task are not counted (SEC-AI-85); one user can fill the agent pool (SEC-AI-89). |
|  | [v5.0.0-15.1.3](V15-Secure-Coding-and-Architecture.md) | Framework | document |  |
|  | [v5.0.0-15.2.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
|  | [v5.0.0-5.2.1](V5-File-Handling.md) | Gap | none |  |
|  | [v5.0.0-4.3.1](V4-API-and-Web-Service.md) | N/A | - |  |
| API5:2023 Broken Function Level Authorization | [v5.0.0-8.1.1](V8-Authorization.md) | Application | document | A `/rest/s1` path is authorized as `AT_REST_PATH` (a record is needed for its last node) and each entity operation as `AT_ENTITY`; a method-override header is authorized as the method it names (the 8.3.1 proof). Open under 8.2.1: SEC-TLS-85, SEC-AI-95. |
|  | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |
| API6:2023 Unrestricted Access to Sensitive Business Flows | [v5.0.0-2.1.3](V2-Validation-and-Business-Logic.md) | Application | none | The flows are the application's. Tools: `record#Attempt`, `ArtifactTarpit` rows, web protection. |
|  | [v5.0.0-2.3.2](V2-Validation-and-Business-Logic.md) | Application | none |  |
|  | [v5.0.0-2.4.1](V2-Validation-and-Business-Logic.md) | Configuration | `V2Validation` (deployment, opt-in) |  |
| API7:2023 Server Side Request Forgery | [v5.0.0-1.3.6](V1-Encoding-and-Sanitization.md) | Gap | none | An internal-address guard, on in production, and lists for some features, but no general allowlist (SEC-RTE-87, SEC-TLS-101, SEC-OUT-84). An A2A push URL and an AI task's webhook are the caller's: https, internal addresses refused at connect, no redirect (map); `llm-facade/a2a/push@allow-hosts` lifts the guard for its hosts. |
|  | [v5.0.0-13.2.4](V13-Configuration.md) | Configuration | `host_rejected_by_allowed_hosts_does_not_touch_the_sink` (`moqui-mail`); `an_http_location_reaches_an_internal_address_only_off_production` (`context/resource/tests.rs`) |  |
|  | [v5.0.0-13.2.5](V13-Configuration.md) | Configuration | `an_http_location_reaches_an_internal_address_only_off_production` (`context/resource/tests.rs`) |  |
|  | [v5.0.0-15.3.2](V15-Secure-Coding-and-Architecture.md) | Framework | `a_redirect_to_another_host_is_followed_only_when_asked` (`moqui-lib` `http_tests.rs`) |  |
| API8:2023 Security Misconfiguration | [v5.0.0-3.4.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` | With `@https-enabled` set, `/rest` redirects to https as the screens do (4.1.2). Some `text/plain` answers name no charset (4.1.1). `/a2a`, `/mcp` and the gateway are off by default. |
|  | [v5.0.0-4.1.1](V4-API-and-Web-Service.md) | Gap | none |  |
|  | [v5.0.0-4.1.2](V4-API-and-Web-Service.md) | Gap | none |  |
|  | [v5.0.0-4.1.3](V4-API-and-Web-Service.md) | Configuration | `V4ApiWebService` |  |
|  | [v5.0.0-4.2.1](V4-API-and-Web-Service.md) | Framework | `V4ApiWebService` |  |
|  | [v5.0.0-13.4.4](V13-Configuration.md) | Configuration | `V13Configuration` (deployment) |  |
|  | [v5.0.0-16.5.1](V16-Security-Logging-and-Error-Handling.md) | Framework | `V16Logging` |  |
| API9:2023 Improper Inventory Management | [v5.0.0-13.1.1](V13-Configuration.md) | Framework | document | The inventory is the routes map ([Attack-Surface-Routes](../Attack-Surface-Routes.md)). `/rest/v1` is still served as a deprecated alias of `/rest/e1`. REST schemas need `REST_SCHEMA`. |
|  | [v5.0.0-13.4.5](V13-Configuration.md) | Framework | `V13Configuration` |  |
|  | [v5.0.0-15.2.3](V15-Secure-Coding-and-Architecture.md) | Configuration | `V15SecureCoding` |  |
| API10:2023 Unsafe Consumption of APIs | [v5.0.0-12.3.2](V12-Secure-Communication.md) | Configuration | `the_outbound_http_client_checks_the_servers_certificate_and_name` (`tls_tests.rs`) | Remote A2A agents and MCP servers are rows an `AgentAdmin` writes; their answers reach the model wrapped as untrusted (LLM01 below). The OIDC client trusts the discovery document's own issuer (10.5.3). The mail client reads its ID token unsigned (SEC-MAIL-98). MX STARTTLS takes any certificate by default (12.3.2). |
|  | [v5.0.0-15.3.2](V15-Secure-Coding-and-Architecture.md) | Framework | `a_redirect_to_another_host_is_followed_only_when_asked` (`moqui-lib` `http_tests.rs`) |  |
|  | [v5.0.0-16.5.2](V16-Security-Logging-and-Error-Handling.md) | Framework | `a_call_to_a_service_that_stops_answering_fails_at_its_timeout` (`moqui-lib` `http_tests.rs`) |  |
|  | [v5.0.0-1.5.2](V1-Encoding-and-Sanitization.md) | Framework | `untrusted_data_is_deserialized_only_into_plain_values_and_fixed_types` (`web/tests.rs`) |  |
|  | [v5.0.0-10.5.3](V10-OAuth-and-OIDC.md) | Gap | none |  |

## OWASP Top 10 for LLM Applications (2025)

Read for the agents, Assist, A2A, MCP and the gateway: [Attack-Surface-AI](../Attack-Surface-AI.md) and the AI
entries of the [register](../../../plans/archive/Security-Review.md) (`SEC-AI-*`). ASVS 5.0.0 has no chapter for model
applications, so half of these items have no counterpart; for those the row says so. The last column names the
controls and the register entries that bear on the item. **None of it is a proof**: the controls are read from the
map, and the tests named there are regression tests of the control, not filed under a requirement.

| Item | Requirement | Status | Proof | mr-moqui's controls and register entries (not proofs) |
| --- | --- | --- | --- | --- |
| LLM01:2025 Prompt Injection | none | - | - | No ASVS counterpart. `moqui_llm::prompt::untrusted` wraps remote agents' and MCP servers' text, chat history, trigger records, attachments, learned skills, lessons and summaries. The results of the `screen`, `data`, `search` and service tools are not wrapped; for them the controls are the core prompt's rule and the approval of every write. READ tools run under the read fence. Register: SEC-AI-97, SEC-AI-93, SEC-AI-96. Regression tests: `remote_schemas_and_errors_reach_the_model_only_as_untrusted_text`, `the_summary_goes_in_as_untrusted_text_so_a_note_in_a_tool_result_cannot_close_it`. |
| LLM02:2025 Sensitive Information Disclosure | [v5.0.0-8.2.3](V8-Authorization.md) | Gap | none | The `data` tool never reads an encrypted or `ai-opt-out` field, and secret-named fields only with `secret-fields`; an agent's render shows a placeholder for an opted-out field. The content log is off by default and redacted; conversations and tool results are stored unencrypted. Register: SEC-AI-83 (Medium, Evals shows every user's prompts to an `AgentAdmin`), SEC-AI-91, SEC-AI-94, SEC-AI-98. Regression tests: `AiOptOut` (spec), `data_reads_no_text_of_ai_opt_out_columns_and_entity_fields`, `redaction_takes_secret_keys_and_token_shaped_words`. |
|  | [v5.0.0-14.1.1](V14-Data-Protection.md) | Application | none |  |
|  | [v5.0.0-16.2.5](V16-Security-Logging-and-Error-Handling.md) | Gap | none |  |
| LLM03:2025 Supply Chain | [v5.0.0-15.1.1](V15-Secure-Coding-and-Architecture.md) | Gap | none | The server loads no model; every model is a remote endpoint. Endpoints, remote A2A agents and MCP servers are rows an `AgentAdmin` writes: https unless a private network is allowed, each call held to the row's origin, no redirect. A remote MCP schema is rebuilt from an allowlist. The provider address check reads only an address written in the URL (map). No register entry. |
|  | [v5.0.0-15.1.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
|  | [v5.0.0-15.2.1](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |
| LLM04:2025 Data and Model Poisoning | none | - | - | No ASVS counterpart, and mr-moqui trains no model. What carries into later prompts: memory (one user's, a confirm card when tool output came last; SEC-AI-93), learned skills (wrapped; a site copy published only by an admin), lessons (the author's until `review#AgentLesson`), eval datasets (SEC-AI-96). Regression tests: `a_skill_saved_from_a_run_is_found_in_a_fresh_conversation_and_loads_as_untrusted`, `a_lesson_is_its_authors_until_an_admin_approves_it_and_never_in_a_prefix`. |
| LLM05:2025 Improper Output Handling | [v5.0.0-1.2.1](V1-Encoding-and-Sanitization.md) | Application | none | The drawer renders assistant Markdown with HTML escaped, then DOMPurify; images only from the site's static images (SEC-AI-03, fixed); a link carrying data past its host opens only after a confirm (SEC-AI-04, fixed); the terminal pane writes no control character. Canvases: OpenUI is validated, Vue SFC runs in a sandboxed frame. Open: SEC-AI-81 (Medium, a Vue SFC canvas navigates its frame to another site), SEC-AI-82 (Medium, dev instances only, a `screen-xml` canvas runs script in the application's origin), SEC-AI-05. |
|  | [v5.0.0-1.3.1](V1-Encoding-and-Sanitization.md) | Framework | `the_rich_text_editor_output_survives_the_safe_cleaning` (`service/validate.rs`) |  |
|  | [v5.0.0-1.3.5](V1-Encoding-and-Sanitization.md) | Gap | none |  |
|  | [v5.0.0-3.2.2](V3-Web-Frontend-Security.md) | Application | none (to write) |  |
|  | [v5.0.0-3.4.3](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.5.5](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
| LLM06:2025 Excessive Agency | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none | A service is a tool only through an `AgentToolExposure` row (`AgentAdmin`), and the user's permission is checked before it runs; a tool runs as the run's user. READ tools run under the read fence; every other class waits for an approval decided only from a browser render or the terminal (a policy approves by itself only for a background agent); DESTRUCTIVE needs `allowDestructive`. Fixed: SEC-AI-01, SEC-AI-02. Open: SEC-AI-13, SEC-AI-87, SEC-AI-90, SEC-AI-95. Regression tests: `an_ai_field_proposal_becomes_an_approval_and_never_a_silent_write`, `a_waiting_call_goes_on_only_to_the_origin_it_was_approved_for`. |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |
| LLM07:2025 System Prompt Leakage | none | - | - | No ASVS counterpart. The drawer's constitution readers leave out `instructions` and `documentText`. A model key goes in the request's auth header, not the prompt. Mantle's user facts put the user's organizations' names and ids in the prompt. No register entry. |
| LLM08:2025 Vector and Embedding Weaknesses | none | - | - | No ASVS counterpart. Semantic find is off on the default conf (no embedder). `/find/rank` needs a signed-in user, refuses an `EXTERNAL_USER`, and counts the user's quota and a tarpit (SEC-AI-07, fixed); it ranks the texts the request sends. `get#SkillRetrieval` ranks skills as any user it is given (`AgentAdmin`, SEC-AI-98). Regression test: `an_embedding_call_for_an_external_or_over_quota_user_is_refused`. |
| LLM09:2025 Misinformation | none | - | - | No ASVS counterpart. A link in a run's final text that no tool result or user message held is marked invented (`check_answer`), but only in the final text, and the marks are lost on reload (SEC-AI-92). A write is shown to the user before it runs. Regression test: `an_answer_link_no_tool_returned_is_an_invented_reference`. |
| LLM10:2025 Unbounded Consumption | [v5.0.0-2.4.1](V2-Validation-and-Business-Logic.md) | Configuration | `V2Validation` (deployment, opt-in) | A per-user `LlmQuota` (own row, else the largest group row, else the conf), reserved before a gateway call; run caps on steps, tool calls, tokens, cost and time; tarpits on new runs, MCP calls, drawer actions and embeddings; streams 256 a node and 16 a user; one agent pool of 8 workers and 64 jobs. Open: SEC-AI-85, SEC-AI-88, SEC-AI-89, SEC-AI-99. Regression test: `the_quota_is_the_users_row_else_the_largest_group_row_else_the_conf`. |
|  | [v5.0.0-15.1.3](V15-Secure-Coding-and-Architecture.md) | Framework | document |  |
|  | [v5.0.0-15.2.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |

## NIST SP 800-63B-4

The sections of SP 800-63B-4 with authentication requirements that the V6 and V7 rows meet or miss, by the
document's own section numbers, linked to the section. A few rows of other chapters are added where they are the
direct counterpart (11.4.2 for password storage, 3.3 for cookies, 3.5.1 and 12.2.1 for session binding). The rows
judge mr-moqui by ASVS; a note says where SP 800-63B-4 asks more than the ASVS row does.

| Section | Requirement | Status | Proof | Where it is thin |
| --- | --- | --- | --- | --- |
| [2.2.2](https://pages.nist.gov/800-63-4/sp800-63b.html#aal2req) Authenticator and Verifier Requirements (AAL2) | [v5.0.0-6.3.3](V6-Authentication.md) | Configuration | `V6Authentication` (deployment) | Multi-factor is a data step (`requireAuthcFactor`), off by default, and keys, app passwords and SSO ask no second factor. 2.2.2 also asks AAL2 to offer a phishing-resistant option; no WebAuthn or passkey sign-in was found in `crates/` or the maps, and ASVS puts phishing resistance at level 3 (V6.7). |
|  | [v5.0.0-6.3.4](V6-Authentication.md) | Gap | none |  |
| [2.2.3](https://pages.nist.gov/800-63-4/sp800-63b.html#aal2reauth) Reauthentication (AAL2) | [v5.0.0-7.3.1](V7-Session-Management.md) | Framework | `an_idle_session_expires_and_its_row_is_deleted` (`session_tests.rs`) | The 60-minute idle timeout fits AAL2. There is no overall timeout, which every level asks for (2.1.3, 2.2.3, 2.3.3). |
|  | [v5.0.0-7.3.2](V7-Session-Management.md) | Gap | none |  |
| [3.1.1.2](https://pages.nist.gov/800-63-4/sp800-63b.html#passwordver) Password Verifiers | [v5.0.0-6.2.1](V6-Authentication.md) | Framework | `V6Authentication`; `password_acceptable_rejects_secret_and_accepts_a_mixed_password` (`policy.rs`) | The minimum of 8 (`user-facade/password@min-length`) meets 3.1.1.2 only for a password used with a second factor; one used alone needs 15. The defaults `min-digits` and `min-others` 1 and `change-weeks` 104 are composition rules and rotation that 3.1.1.2 forbids; set them to 0. No blocklist. argon2id is not a scheme of SP 800-132, which 3.1.1.2 recommends. |
|  | [v5.0.0-6.2.4](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.2.5](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.2.6](V6-Authentication.md) | Framework | `V6Authentication` |  |
|  | [v5.0.0-6.2.7](V6-Authentication.md) | Framework | `V6Authentication` |  |
|  | [v5.0.0-6.2.8](V6-Authentication.md) | Framework | `V6Authentication` |  |
|  | [v5.0.0-6.2.9](V6-Authentication.md) | Framework | `V6Authentication` |  |
|  | [v5.0.0-6.2.10](V6-Authentication.md) | Configuration | `a_password_stays_valid_with_change_weeks_zero_and_the_shipped_default_rotates_it` (`login_tests.rs`) |  |
|  | [v5.0.0-6.2.11](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.2.12](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.1.2](V6-Authentication.md) | Application | none |  |
|  | [v5.0.0-6.4.2](V6-Authentication.md) | Framework | `V6Authentication` |  |
|  | [v5.0.0-11.4.2](V11-Cryptography.md) | Framework | `hash_password_is_an_argon2id_phc_string` (`password.rs`) |  |
|  | [v5.0.0-6.3.1](V6-Authentication.md) | Configuration | `V6Authentication` (deployment, opt-in); regression test: `SecurityAuthn` (the account lock) |  |
| [3.1.2.2](https://pages.nist.gov/800-63-4/sp800-63b.html#lookupver) Look-Up Secret Verifiers | [v5.0.0-6.5.1](V6-Authentication.md) | Framework | `a_code_is_taken_for_a_step_after_the_last_one` (`totp.rs`) | Codes are kept as SHA-256 under a fixed salt; for a secret under 112 bits 3.1.2.2 asks a salted password hash. |
|  | [v5.0.0-6.5.2](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.5.4](V6-Authentication.md) | Framework | `random_codes_have_the_width_and_strings_are_url_safe` (`factor.rs`) |  |
| [3.1.3.2](https://pages.nist.gov/800-63-4/sp800-63b.html#out-of-band-verifiers) Out-of-Band Verifiers | [v5.0.0-6.5.1](V6-Authentication.md) | Framework | `a_code_is_taken_for_a_step_after_the_last_one` (`totp.rs`) | A sent code is good for 24 hours, where 3.1.3.2 allows 10 minutes; it is not bound to its sign-in attempt; codes sent at once are not all counted (SEC-WEB-87). |
|  | [v5.0.0-6.5.3](V6-Authentication.md) | Framework | `factor_seeds_and_codes_come_from_the_operating_systems_random_source` (`login_tests.rs`) |  |
|  | [v5.0.0-6.5.4](V6-Authentication.md) | Framework | `random_codes_have_the_width_and_strings_are_url_safe` (`factor.rs`) |  |
|  | [v5.0.0-6.5.5](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.6.2](V6-Authentication.md) | Gap | none |  |
|  | [v5.0.0-6.6.3](V6-Authentication.md) | Framework | `V6Authentication` (wrong codes at once); regression test: `SecurityAuthn` (wrong codes in turn) |  |
| [3.1.3.3](https://pages.nist.gov/800-63-4/sp800-63b.html#pstnOOB) Authentication Using the Public Switched Telephone Network | [v5.0.0-6.6.1](V6-Authentication.md) | Gap | none | An unvalidated SMS factor is counted and sendable; no risk notice. |
| [3.1.4.2](https://pages.nist.gov/800-63-4/sp800-63b.html#single-factor-otp-verifiers) Single-Factor OTP Verifiers | [v5.0.0-6.5.1](V6-Authentication.md) | Framework | `a_code_is_taken_for_a_step_after_the_last_one` (`totp.rs`) | A TOTP code is taken once, for a step after the last, and is good one step either side of its 30 seconds; 6.5.5 is a Gap for the sent codes' 24 hours. |
|  | [v5.0.0-6.5.3](V6-Authentication.md) | Framework | `factor_seeds_and_codes_come_from_the_operating_systems_random_source` (`login_tests.rs`) |  |
|  | [v5.0.0-6.5.5](V6-Authentication.md) | Gap | none |  |
| [3.2.2](https://pages.nist.gov/800-63-4/sp800-63b.html#throttle) Rate Limiting (Throttling) | [v5.0.0-6.1.1](V6-Authentication.md) | Framework | document | The account lock is on by default (3 failures, 5 minutes); per-client limits need web protection. Wrong codes count as passwords do, those sent at once too (6.6.3). |
|  | [v5.0.0-6.3.1](V6-Authentication.md) | Configuration | `V6Authentication` (deployment, opt-in); regression test: `SecurityAuthn` (the account lock) |  |
|  | [v5.0.0-6.6.3](V6-Authentication.md) | Framework | `V6Authentication` (wrong codes at once); regression test: `SecurityAuthn` (wrong codes in turn) |  |
| [3.2.7](https://pages.nist.gov/800-63-4/sp800-63b.html#replay) Replay Resistance | [v5.0.0-6.5.1](V6-Authentication.md) | Framework | `a_code_is_taken_for_a_step_after_the_last_one` (`totp.rs`) |  |
| [3.2.12](https://pages.nist.gov/800-63-4/sp800-63b.html#randomness) Random Values | [v5.0.0-6.4.1](V6-Authentication.md) | Framework | `ResetPassword` (spec); `the_old_password_of_a_change_is_behind_the_login_gate` (`login_tests.rs`) |  |
|  | [v5.0.0-6.5.3](V6-Authentication.md) | Framework | `factor_seeds_and_codes_come_from_the_operating_systems_random_source` (`login_tests.rs`) |  |
|  | [v5.0.0-7.2.3](V7-Session-Management.md) | Framework | `V7SessionManagement`; `session_ids_and_tokens_are_160_bits_from_the_operating_systems_random_source` (`session_tests.rs`) |  |
| [4.1.2.1](https://pages.nist.gov/800-63-4/sp800-63b.html#bindexisting) Binding an Additional Authenticator | [v5.0.0-6.2.2](V6-Authentication.md) | Framework | `V6Authentication` | Open under 7.5.1: SEC-ADM-81, SEC-WEB-84. The notice 4.1.2.1 asks for on a new authenticator has its ASVS row at level 3 (6.3.7) and is not graded. |
|  | [v5.0.0-6.2.3](V6-Authentication.md) | Framework | `V6Authentication`; `the_old_password_of_a_change_is_behind_the_login_gate` (`login_tests.rs`) |  |
|  | [v5.0.0-7.5.1](V7-Session-Management.md) | Gap | none |  |
| [4.2](https://pages.nist.gov/800-63-4/sp800-63b.html#recovery) Account Recovery | [v5.0.0-6.4.3](V6-Authentication.md) | Gap | none | A relay administrator receives reset mail (SEC-ADM-82). No identity proofing step. |
|  | [v5.0.0-6.4.4](V6-Authentication.md) | Application | none |  |
| [4.2.1.2](https://pages.nist.gov/800-63-4/sp800-63b.html#issuedrecovery) Issued Recovery Codes | [v5.0.0-6.4.1](V6-Authentication.md) | Framework | `ResetPassword` (spec); `the_old_password_of_a_change_is_behind_the_login_gate` (`login_tests.rs`) | An emailed reset password is good for `user-facade/password@email-expire-hours`, 48 by default; 4.2.1.2 allows 24 hours for email. |
| [4.2.3](https://pages.nist.gov/800-63-4/sp800-63b.html#recoverynotification) Account Recovery Notification; [4.6](https://pages.nist.gov/800-63-4/sp800-63b.html#notification) Account Notifications | none | - | - | No ASVS row at levels 1 and 2 (6.3.7 is level 3); not graded. |
| [4.5](https://pages.nist.gov/800-63-4/sp800-63b.html#invalidation) Invalidation | [v5.0.0-6.4.4](V6-Authentication.md) | Application | none | A second-factor change offers no ending of other sessions (7.4.3); a deleted account row is not caught (7.4.2). |
|  | [v5.0.0-7.4.2](V7-Session-Management.md) | Framework | `every_session_of_a_user_ends_and_a_disabled_account_signs_out` (`session_tests.rs`) |  |
|  | [v5.0.0-7.4.3](V7-Session-Management.md) | Application | part: `PasswordChangeGate` (spec) |  |
|  | [v5.0.0-7.4.5](V7-Session-Management.md) | Framework | `AdminSessions` (spec) |  |
| [5.1](https://pages.nist.gov/800-63-4/sp800-63b.html#bindings) Session Bindings | [v5.0.0-7.2.1](V7-Session-Management.md) | Framework | `V7SessionManagement`; `a_bad_mac_is_an_unknown_session` (`session_tests.rs`) | A re-authentication keeps the session id (7.2.4). The session secret is 160 bits, over the 64 that 5.1 asks. |
|  | [v5.0.0-7.2.2](V7-Session-Management.md) | Framework | `V7SessionManagement` |  |
|  | [v5.0.0-7.2.3](V7-Session-Management.md) | Framework | `V7SessionManagement`; `session_ids_and_tokens_are_160_bits_from_the_operating_systems_random_source` (`session_tests.rs`) |  |
|  | [v5.0.0-7.2.4](V7-Session-Management.md) | Gap | none |  |
|  | [v5.0.0-7.4.1](V7-Session-Management.md) | Framework | `V7SessionManagement`; `a_logout_ends_the_session_and_its_attributes` (`session_tests.rs`) |  |
|  | [v5.0.0-7.4.4](V7-Session-Management.md) | Framework | none (to write) |  |
|  | [v5.0.0-3.5.1](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-12.2.1](V12-Secure-Communication.md) | Configuration | `V12SecureCommunication` (deployment) |  |
| [5.1.1](https://pages.nist.gov/800-63-4/sp800-63b.html#sesscookies) Browser Cookies | [v5.0.0-3.3.1](V3-Web-Frontend-Security.md) | Gap | none | The session cookie is `Secure` and `__Host-` only over https; plain http is the default. |
|  | [v5.0.0-3.3.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
|  | [v5.0.0-3.3.3](V3-Web-Frontend-Security.md) | Gap | none |  |
|  | [v5.0.0-3.3.4](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |
| [5.2](https://pages.nist.gov/800-63-4/sp800-63b.html#sessionreauthn) Reauthentication | [v5.0.0-7.1.1](V7-Session-Management.md) | Gap | none | The limits are not documented with a rationale (7.1.1), and there is no overall timeout (7.3.2). |
|  | [v5.0.0-7.3.1](V7-Session-Management.md) | Framework | `an_idle_session_expires_and_its_row_is_deleted` (`session_tests.rs`) |  |
|  | [v5.0.0-7.3.2](V7-Session-Management.md) | Gap | none |  |

V6 and V7 rows with no counterpart in SP 800-63B-4: 6.1.3, 6.3.2, 7.1.2, 7.1.3, 7.5.2 and 7.6.2. The rows 6.8.1 to
6.8.4 and 7.6.1 are federation, which ASVS takes from SP 800-63C; they are not graded here.

## CWE Top 25 (2025)

The 2025 list in rank order, each weakness to the requirements that prevent it. "In the ASVS export" gives the 5.0.0
ids the pre-release export names for that CWE ([above](#where-the-mappings-come-from)); where it differs from ours,
ours is wider or the export's row is level 3.

| CWE | Requirement | Status | Proof | In the ASVS export | Where it is thin |
| --- | --- | --- | --- | --- | --- |
| 1. [CWE-79](https://cwe.mitre.org/data/definitions/79.html) Improper Neutralization of Input During Web Page Generation ('Cross-site Scripting') | [v5.0.0-1.2.1](V1-Encoding-and-Sanitization.md) | Application | none | 3.2.3 (level 3) | Escaping is the default for `${}`, widgets and `.html.fancy`; raw output paths are the builder's. No CSP script allowlist. |
|  | [v5.0.0-3.2.2](V3-Web-Frontend-Security.md) | Application | none (to write) |  |  |
|  | [v5.0.0-1.3.1](V1-Encoding-and-Sanitization.md) | Framework | `the_rich_text_editor_output_survives_the_safe_cleaning` (`service/validate.rs`) |  |  |
|  | [v5.0.0-1.3.4](V1-Encoding-and-Sanitization.md) | Framework | `a_users_file_asked_inline_is_shown_only_when_passive_and_then_sandboxed` (`web/tests.rs`) |  |  |
|  | [v5.0.0-1.3.5](V1-Encoding-and-Sanitization.md) | Gap | none |  |  |
|  | [v5.0.0-1.1.2](V1-Encoding-and-Sanitization.md) | Gap | none |  |  |
|  | [v5.0.0-3.4.3](V3-Web-Frontend-Security.md) | Gap | none |  |  |
| 2. [CWE-89](https://cwe.mitre.org/data/definitions/89.html) Improper Neutralization of Special Elements used in an SQL Command ('SQL Injection') | [v5.0.0-1.2.4](V1-Encoding-and-Sanitization.md) | Framework | `V1Encoding`; `postgres_enumeration_insert_select_and_count` (`moqui-db` `sql/tests.rs`) | 1.2.4 | Raw SQL only through `makeConditionWhere` and the SQL tools (SEC-TLS-88, SEC-TLS-100). |
| 3. [CWE-352](https://cwe.mitre.org/data/definitions/352.html) Cross-Site Request Forgery (CSRF) | [v5.0.0-3.5.1](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` | 3.5.1 | A same-origin GET can run an acting transition (3.5.3, SEC-CLI-82); a cross-site GET of `/sso/login` can start a sign-in (7.6.2). |
|  | [v5.0.0-3.5.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |  |
|  | [v5.0.0-3.5.3](V3-Web-Frontend-Security.md) | Gap | none |  |  |
|  | [v5.0.0-3.3.2](V3-Web-Frontend-Security.md) | Framework | `V3WebFrontend` |  |  |
|  | [v5.0.0-7.6.2](V7-Session-Management.md) | Gap | none |  |  |
| 4. [CWE-862](https://cwe.mitre.org/data/definitions/862.html) Missing Authorization | [v5.0.0-8.1.1](V8-Authorization.md) | Application | document | none | Open under 8.2.1: SEC-TLS-85, SEC-AI-95. |
|  | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none |  |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |  |
| 5. [CWE-787](https://cwe.mitre.org/data/definitions/787.html) Out-of-bounds Write | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | none | Safe Rust; `unsafe` only at ten listed sites, which the proof pins. |
| 6. [CWE-22](https://cwe.mitre.org/data/definitions/22.html) Improper Limitation of a Pathname to a Restricted Directory ('Path Traversal') | [v5.0.0-5.3.2](V5-File-Handling.md) | Gap | none | none (it gives 5.3.2 CWE-73) | `print#ResourceDocument` reads any location (SEC-RTE-83). Static files refuse `..` and hidden names (the 13.4.1 proof's Rust test). |
| 7. [CWE-416](https://cwe.mitre.org/data/definitions/416.html) Use After Free | [v5.0.0-1.4.3](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | 1.4.3 |  |
| 8. [CWE-125](https://cwe.mitre.org/data/definitions/125.html) Out-of-bounds Read | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | none |  |
| 9. [CWE-78](https://cwe.mitre.org/data/definitions/78.html) Improper Neutralization of Special Elements used in an OS Command ('OS Command Injection') | [v5.0.0-1.2.5](V1-Encoding-and-Sanitization.md) | Framework | `no_script_starts_a_process` (`framework_gaps_tests.rs`) | 1.2.5 |  |
| 10. [CWE-94](https://cwe.mitre.org/data/definitions/94.html) Improper Control of Generation of Code ('Code Injection') | [v5.0.0-1.3.2](V1-Encoding-and-Sanitization.md) | Framework | `WikiPageTypes`, `DocumentExpression` (specs) | 1.3.5, 1.3.7 | Markdown and CSS (1.3.5) and templates from stored text (1.3.7) are Gaps; on a dev instance a `screen-xml` canvas runs model-written script (SEC-AI-82). |
|  | [v5.0.0-1.3.5](V1-Encoding-and-Sanitization.md) | Gap | none |  |  |
|  | [v5.0.0-1.3.7](V1-Encoding-and-Sanitization.md) | Gap | part: `urls_forms_and_assembled_expressions` (`script/theme/tests.rs`) |  |  |
|  | [v5.0.0-5.3.1](V5-File-Handling.md) | Framework | `WikiPageTypes` (spec); `a_write_at_a_location_with_a_hash_is_refused` (`resource/tests.rs`) |  |  |
| 11. [CWE-120](https://cwe.mitre.org/data/definitions/120.html) Buffer Copy without Checking Size of Input ('Classic Buffer Overflow') | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | 1.4.1 |  |
| 12. [CWE-434](https://cwe.mitre.org/data/definitions/434.html) Unrestricted Upload of File with Dangerous Type | [v5.0.0-5.2.2](V5-File-Handling.md) | Application | none | 5.2.2 | No extension-to-content check (5.2.2) and no malware scan hook (5.4.3); a stored file is served inert and never run (3.2.1, 5.3.1). |
|  | [v5.0.0-5.3.1](V5-File-Handling.md) | Framework | `WikiPageTypes` (spec); `a_write_at_a_location_with_a_hash_is_refused` (`resource/tests.rs`) |  |  |
|  | [v5.0.0-3.2.1](V3-Web-Frontend-Security.md) | Framework | `a_users_download_has_an_inert_type_and_the_sandbox` (`web/tests.rs`) |  |  |
|  | [v5.0.0-5.4.3](V5-File-Handling.md) | Gap | none |  |  |
| 13. [CWE-476](https://cwe.mitre.org/data/definitions/476.html) NULL Pointer Dereference | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | none | No ASVS row names it; 1.4.1 is the nearest. Safe Rust cannot form a null reference, and the `unsafe` sites are pinned by the 1.4.1 proof. |
| 14. [CWE-121](https://cwe.mitre.org/data/definitions/121.html) Stack-based Buffer Overflow | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | none |  |
| 15. [CWE-502](https://cwe.mitre.org/data/definitions/502.html) Deserialization of Untrusted Data | [v5.0.0-1.5.2](V1-Encoding-and-Sanitization.md) | Framework | `untrusted_data_is_deserialized_only_into_plain_values_and_fixed_types` (`web/tests.rs`) | 1.5.2 |  |
| 16. [CWE-122](https://cwe.mitre.org/data/definitions/122.html) Heap-based Buffer Overflow | [v5.0.0-1.4.1](V1-Encoding-and-Sanitization.md) | Framework | `the_server_has_unsafe_code_only_at_the_listed_reviewed_sites` (`moqui-host` `lib_tests.rs`) | none |  |
| 17. [CWE-863](https://cwe.mitre.org/data/definitions/863.html) Incorrect Authorization | [v5.0.0-8.2.2](V8-Authorization.md) | Gap | none | none | Record and field access are Gaps; tenants are the application's (8.4.1). |
|  | [v5.0.0-8.2.3](V8-Authorization.md) | Gap | none |  |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |  |
|  | [v5.0.0-8.4.1](V8-Authorization.md) | Application | none |  |  |
| 18. [CWE-20](https://cwe.mitre.org/data/definitions/20.html) Improper Input Validation | [v5.0.0-2.1.1](V2-Validation-and-Business-Logic.md) | Application | none | 2.1.1, 2.1.2, 2.2.1, 2.2.3 | The rules are the application's; the framework runs them on the server for every service call (2.2.2). |
|  | [v5.0.0-2.1.2](V2-Validation-and-Business-Logic.md) | Application | none |  |  |
|  | [v5.0.0-2.2.1](V2-Validation-and-Business-Logic.md) | Application | none |  |  |
|  | [v5.0.0-2.2.2](V2-Validation-and-Business-Logic.md) | Framework | `V2Validation` |  |  |
|  | [v5.0.0-2.2.3](V2-Validation-and-Business-Logic.md) | Application | none |  |  |
| 19. [CWE-284](https://cwe.mitre.org/data/definitions/284.html) Improper Access Control | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none | none | A class over CWE-862 and CWE-863. |
|  | [v5.0.0-8.2.2](V8-Authorization.md) | Gap | none |  |  |
|  | [v5.0.0-8.3.1](V8-Authorization.md) | Framework | `V8Authorization` |  |  |
| 20. [CWE-200](https://cwe.mitre.org/data/definitions/200.html) Exposure of Sensitive Information to an Unauthorized Actor | [v5.0.0-13.4.2](V13-Configuration.md) | Framework | part: `V16Logging` (its 16.5.1 feature) | 14.2.3, 16.4.2, 13.4.6 (level 3) | A password in the query string signs in (14.2.1); entity REST answers the whole row less secret fields (15.3.1). |
|  | [v5.0.0-13.4.5](V13-Configuration.md) | Framework | `V13Configuration` |  |  |
|  | [v5.0.0-14.2.3](V14-Data-Protection.md) | Framework | `V14DataProtection` |  |  |
|  | [v5.0.0-16.4.2](V16-Security-Logging-and-Error-Handling.md) | Configuration | `the_log_files_are_the_server_users_alone` (`logging_tests.rs`) |  |  |
|  | [v5.0.0-16.5.1](V16-Security-Logging-and-Error-Handling.md) | Framework | `V16Logging` |  |  |
|  | [v5.0.0-14.2.1](V14-Data-Protection.md) | Gap | none |  |  |
|  | [v5.0.0-15.3.1](V15-Secure-Coding-and-Architecture.md) | Application | none |  |  |
| 21. [CWE-306](https://cwe.mitre.org/data/definitions/306.html) Missing Authentication for Critical Function | [v5.0.0-6.1.3](V6-Authentication.md) | Gap | document | 6.1.3, 6.3.4, 7.5.1, 7.5.3 (level 3) | Screens and services need a user unless marked (8.2.1); the pathways are not documented together (6.1.3); the recent sign-in guard has open entries (SEC-ADM-81, SEC-WEB-84). |
|  | [v5.0.0-6.3.4](V6-Authentication.md) | Gap | none |  |  |
|  | [v5.0.0-7.5.1](V7-Session-Management.md) | Gap | none |  |  |
|  | [v5.0.0-8.2.1](V8-Authorization.md) | Gap | none |  |  |
| 22. [CWE-918](https://cwe.mitre.org/data/definitions/918.html) Server-Side Request Forgery (SSRF) | [v5.0.0-1.3.6](V1-Encoding-and-Sanitization.md) | Gap | none | 1.3.6, 13.2.5 | An internal-address guard but no allowlist (SEC-RTE-87, SEC-TLS-101, SEC-OUT-84). |
|  | [v5.0.0-13.2.4](V13-Configuration.md) | Configuration | `host_rejected_by_allowed_hosts_does_not_touch_the_sink` (`moqui-mail`); `an_http_location_reaches_an_internal_address_only_off_production` (`context/resource/tests.rs`) |  |  |
|  | [v5.0.0-13.2.5](V13-Configuration.md) | Configuration | `an_http_location_reaches_an_internal_address_only_off_production` (`context/resource/tests.rs`) |  |  |
|  | [v5.0.0-15.3.2](V15-Secure-Coding-and-Architecture.md) | Framework | `a_redirect_to_another_host_is_followed_only_when_asked` (`moqui-lib` `http_tests.rs`) |  |  |
| 23. [CWE-77](https://cwe.mitre.org/data/definitions/77.html) Improper Neutralization of Special Elements used in a Command ('Command Injection') | [v5.0.0-1.2.5](V1-Encoding-and-Sanitization.md) | Framework | `no_script_starts_a_process` (`framework_gaps_tests.rs`) | none |  |
|  | [v5.0.0-1.3.11](V1-Encoding-and-Sanitization.md) | Framework | `a_call_gives_its_subject_sender_relay_and_bounce_address` (`moqui-mail` `implementation_tests.rs`) |  |  |
| 24. [CWE-639](https://cwe.mitre.org/data/definitions/639.html) Authorization Bypass Through User-Controlled Key | [v5.0.0-8.2.2](V8-Authorization.md) | Gap | none | 8.2.2 | Rows are the caller's only by row filters and checks the application writes (8.2.2). |
|  | [v5.0.0-15.3.3](V15-Secure-Coding-and-Architecture.md) | Framework | `MultiPin` (spec) |  |  |
| 25. [CWE-770](https://cwe.mitre.org/data/definitions/770.html) Allocation of Resources Without Limits or Throttling | [v5.0.0-2.4.1](V2-Validation-and-Business-Logic.md) | Configuration | `V2Validation` (deployment, opt-in) | 2.4.1, 4.3.1, 5.2.4 (level 3) | Per-client limits need web protection; a multipart upload is held in memory (SEC-RTE-82); archives in the loader are unbounded (SEC-DAT-91). |
|  | [v5.0.0-15.2.2](V15-Secure-Coding-and-Architecture.md) | Gap | none |  |  |
|  | [v5.0.0-5.2.1](V5-File-Handling.md) | Gap | none |  |  |
|  | [v5.0.0-5.2.3](V5-File-Handling.md) | Gap | none |  |  |
|  | [v5.0.0-17.3.1](V17-WebRTC.md) | Gap | none |  |  |
|  | [v5.0.0-4.3.1](V4-API-and-Web-Service.md) | N/A | - |  |  |

## Thin spots

Across the five standards, where a reader will find mr-moqui thinnest:

- **No ASVS counterpart.** LLM01, LLM04, LLM07, LLM08 and LLM09; SP 800-63B-4's phishing-resistant option at AAL2
  (2.2.2) and its account notifications (4.2.3, 4.6), which ASVS puts at level 3; CWE-476, which ASVS does not
  name (1.4.1 is the nearest). Their notes point to controls, not proofs.
- **A counterpart, but no framework proof** (every row a Gap or Application): LLM02 and LLM03; SP 800-63B-4's
  telephone codes (3.1.3.3) and account recovery (4.2); CWE-22 (path traversal) and CWE-306 (missing
  authentication for a critical function).
- **One proof among Gaps.** Items whose rows are mostly Gaps or Application, with one row carrying the only
  proof: A03 (3.7.1 of four rows); the authorization items API1, API3, API5, CWE-862, CWE-863 and CWE-284 (8.3.1 or
  15.3.3 beside the Gaps 8.2.1 to 8.2.3); the resource items API4, API6 and CWE-770 (2.4.1, opt-in, against a
  deployment). A04 is proved for password hashing and transport, not for stored fields (V11.3) or the other hashes
  (11.4.1, 11.4.3, 11.4.4).
- **Where SP 800-63B-4 asks more than the ASVS row.** 15 characters for a password used alone (ASVS asks 8), an
  emailed recovery code good for 24 hours at most, and a phishing-resistant option at AAL2 (ASVS: level 3).

## Notices

The OWASP Top 10, the API Security Top 10, the Top 10 for LLM Applications and ASVS are © the OWASP Foundation, under
the licenses above. The CWE ids and names in the CWE table are Copyright © 2006–2026, The MITRE Corporation, used
under the [CWE terms of use](https://cwe.mitre.org/about/termsofuse.html); CWE is a trademark of The MITRE
Corporation. SP 800-63B-4 is a publication of the National Institute of Standards and Technology.
