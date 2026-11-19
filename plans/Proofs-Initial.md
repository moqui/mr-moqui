# Security proofs: the initial plan

Status 2026-11-18: analysis and plan; phase P1, the responsibility matrix for the framework and PASS at levels 1 and 2, is written ([security proofs](../docs/security/proofs/README.md), and mr-pass's `docs/ASVS-Matrix.md`); phase P2, the harness (`moqui test-client --proofs`, the profiles and the summary by requirement), is built; phase P3 moved the security client specs that prove a requirement into the framework's `test/proofs/` (42 features under 20 requirements); phase P4 gave every level 1 row that can take one a proof (56 framework features under 34 requirements, eight of PASS's).

This plan is for a public library of security proofs: executable tests that show mr-moqui meets a published security
standard's requirements. The library is organized by a standard, as the OWASP lists are. Its readers are the project,
which wants to know whether the framework meets the standard by default, and the people who build on mr-moqui and
certify what they build. A certifier should be able to read which requirements the framework meets, which their
configuration or their application must meet, and then run the proofs against their own deployment.

The library is not the security review's register ([Security-Review](Security-Review.md)) and not a list of every bug
that review found. A proof exists for a requirement of a published standard. A fixed finding gets a proof only when its
class is such a requirement; the rest stay regression tests.

Sections: [what exists](#what-exists) · [standards and licenses](#standards-and-licenses) ·
[what a proof is](#what-a-proof-is) · [the responsibility matrix](#the-responsibility-matrix) ·
[layout](#layout) · [running for a certifier](#running-for-a-certifier) · [first coverage](#first-coverage) ·
[phases](#phases) · [decisions](#decisions)

## What exists

- [Proof-Tests](../docs/security/Proof-Tests.md) is a catalog of security controls and their tests. Each control
  names an OWASP Top 10:2025 category and a client spec. It also has tables for the Tools database rules and for stored
  secrets, the latter with eight `known_gap_` Rust tests that pin weaknesses, and a "Proof strength" section saying
  what a green run does not prove. It mentions "ASVS L1 ideas", but no spec or document uses an ASVS id.
- Seven regression client specs in `runtime/components/base/framework/test/client/`: `SecurityAccess` (30 features),
  `SecurityAuthn` (27), `SecurityCors` (4), `SecurityHeaders` (13), `SecurityRest` (15), `SecurityScreens` (27) and
  `SecurityDevice` (5), about 120 features in all. Their names start with a Top 10 id (A01, A02, A05, A07, A10), except
  `SecurityDevice`'s and four in `SecurityScreens`. `SecurityTarpit` (2) is in `test/acceptance/client/`. The users
  they need are made by `test/ClientFixtures.spec.js` (`sec.view.only`, `sec.none.only`, `sec.lock.test`,
  `sec.mfa.totp` and eleven more). `Java-Port-Catalog.md` maps the Moqui Java pytest suite they port.
  `SecurityDevice` is in neither the catalog nor the client README.
- `moqui test-client` (`crates/moqui-host/src/test_client/`, [JS-Test-Client](../docs/dev/JS-Test-Client.md)) runs
  a spec against any server: `--server https://host` with no database or conf of its own. A feature that needs a
  fixture user skips and says so. It speaks HTTP with cookie jars, WebSocket, raw TCP and TLS, and SSH, and drives a
  headless Chromium. Its output is the console and JUnit (`--junit dir`); there is no JSON report of all results.
- The directory decides the suite ([Build-Test, The two suites](../docs/Build-Test.md#the-two-suites)): `test/client`
  is regression, where a spec over 15 seconds fails the run, and `test/acceptance/client` is acceptance. `ci.xml` runs
  `./moq test-framework`, client specs included, on every push.
- There is no docs site; [docs/README.md](../docs/README.md) is a hand-kept index.

## Standards and licenses

Checked 2026-11-18. mr-moqui is CC0. The repository holds ids, levels, short names and links, and states each control
in our own words. It copies no standard's requirement text, so every file stays CC0. A reader follows the link for a
standard's wording.

| Standard | Version | License | What the library takes from it |
| --- | --- | --- | --- |
| [OWASP ASVS](https://github.com/OWASP/ASVS) | 5.0.0 (May 2025): 17 chapters, levels 1 to 3, a CSV export of the requirements (`req_id`, `L`) | CC BY-SA 4.0 | The spine. Each proof is filed under one requirement, cited with its version (`v5.0.0-7.2.1`) |
| [OWASP Top 10](https://top10.owasp.org/2025) | 2025 (final) | the site states CC BY 3.0 | A category on each proof (A01 to A10) |
| [OWASP WSTG](https://github.com/OWASP/wstg) | 4.2 stable; 5.0 in progress | CC BY-SA 4.0 | The test technique a proof follows, cited with its version (`WSTG-v42-ATHN-03`), since its ids change between versions |
| [OWASP API Security Top 10](https://owasp.org/API-Security/) | 2023 | CC BY-SA 4.0 | A category on proofs of `/rest`, `/a2a` and `/mcp` |
| [OWASP Top 10 for LLM Applications](https://genai.owasp.org/) | 2025 | CC BY-SA 4.0 | A category on proofs of the agents and MCP |
| [OWASP MASVS](https://mas.owasp.org/MASVS/) | 2.x | CC BY-SA 4.0 | Later, for the mobile and desktop clients |
| [CWE](https://cwe.mitre.org/) and CAPEC | current; CWE Top 25 | royalty-free use with MITRE's copyright notice reproduced ([terms](https://cwe.mitre.org/about/termsofuse.html)) | Weakness ids on each proof |
| [NIST SP 800-63B-4](https://pages.nist.gov/800-63-4/sp800-63b.html) | July 2025 | US government work | Authentication requirements ASVS takes from it (password length, rate limiting, factors) |
| NIST SP 800-53 | Rev. 5 | US government work | An optional control cross-reference for certifiers (AC, IA, SC, AU families) |
| IETF RFCs and W3C specifications | RFC 6265bis (cookies), 6797 (HSTS), 9700 (OAuth security), 8725 (JWT); CSP Level 3, Fetch Metadata | IETF Trust and W3C document licenses | The protocol rule a proof checks |
| PCI DSS | 4.0.1 | © PCI Security Standards Council, all rights reserved | Requirement numbers only, in a cross-reference, for the payment features |
| CIS Controls | 8.1 | CC BY-NC-ND 4.0 | Ids only, if at all |
| ISO/IEC 27001 and 27002, SOC 2 | | proprietary | Not used |

ASVS is the spine because its requirements are written to be verified and come in levels, which is what a certifier
works from. The Top 10 lists are awareness lists: they tag a proof but do not file it.

## What a proof is

A proof has four parts:

1. **A claim** in our words, for example "a successful sign-in issues a new session id".
2. **The requirement** it is filed under (one ASVS id), and its tags: the Top 10 category, CWE ids, and where they add
   something, the API or LLM Top 10 category, a WSTG technique, an RFC.
3. **An executable test** that fails when the claim does not hold, with a positive control beside each refusal (a
   same-origin request allowed next to a foreign one refused), as the current specs have.
4. **What it does not prove**, in a sentence, as Proof-Tests' "Proof strength" does now: the conf it ran under, what
   it could not see from outside.

There are two kinds:

- **Black-box proofs** are client specs. They run against any server, a certifier's deployment included. They are the
  default and most of the library.
- **In-process proofs** are Rust tests or framework specs, for what cannot be seen from outside: how passwords are
  hashed, what a stored encrypted field holds, the `known_gap_` tests. A certifier reads their results from the
  project's runs; the catalog says which kind each proof is.

## The responsibility matrix

Every ASVS 5.0.0 requirement at levels 1 and 2 gets a status, once for the framework and once for PASS (mr-pass with
the Mantle services behind it, the public application the most likely to be certified):

| Status | Meaning |
| --- | --- |
| Framework | Met by default. A proof shows it. |
| Configuration | Met when the deployer sets something. The setting is named, and the proof checks the deployment. |
| Application | The builder's to meet. The framework's tool for it is named (a validation rule, an authorization record, an escaping template). |
| Gap | Not met today. Linked to the register entry or the `known_gap_` test that records it. |
| Not applicable | The requirement does not apply (for example an API style the framework does not offer), with the reason. |

For PASS, a requirement the framework already proves is marked inherited, with a link, and is not proved twice. The
matrix is the main tool for a certifier: it says what mr-moqui meets, what is theirs, and where the evidence is.

## Layout

- **Proofs** go in `runtime/components/base/framework/test/proofs/`, a third kind of test beside `test/client` and
  `test/acceptance`, with its own `suite.js`, which also names the matrix the run's summary joins. A file holds one
  ASVS chapter (`V3WebFrontend.spec.js`, `V6Authentication.spec.js`, ...; the directory, not the suffix, marks a
  proof). A feature name starts with the requirement id: `v5.0.0-7.2.4: a successful sign-in issues a new session id`.
  The cross-reference tags (the Top 10 category, CWE ids) are P6's, on the matrix row or on the feature as decided
  then; until then the requirement id is the only filing.
- **They run** in every `./moq test-framework`, each spec under 15 seconds as in the regression suite, and alone with
  `moqui test-client framework --proofs --server URL`.
- **The public documents** go in `docs/security/proofs/`: an index, the matrix (one file per ASVS chapter), a
  certifier's guide, and the license and attribution notes. Proof-Tests becomes that index or points to it. Its Tools
  and stored-secret tables become in-process proofs under V11, V13 and V14.
- **PASS's proofs** go in its own repository, `mr-pass/test/proofs/`, run with `moqui test-client mr-pass --proofs`;
  its matrix in `mr-pass/docs/`. The framework's index links to it.

## Running for a certifier

Two profiles:

- **fixtures**: the project's run. The server is started with `ClientFixtures`, and every proof runs.
- **deployment**: a certifier's run against their own server. No fixtures: the anonymous checks, and those that need
  an account use a low-privilege one the certifier passes with `--param`. A proof that needs more skips and says
  what it needs.

The output is JUnit, as now, and a new summary by requirement: the matrix with each requirement's proofs and their
pass, fail or skip, as JSON and as Markdown, which a certifier can attach as evidence.

## First coverage

Level 1, framework-owned, by ASVS chapter. The requirement ids are filled in from the 5.0.0 CSV in phase P1; this plan
names chapters only and guesses no numbers.

| Chapter | What the first proofs cover | Existing features |
| --- | --- | --- |
| V1 Encoding and Sanitization | template output escaping, bound SQL, no header injection | a few in `SecurityHeaders` |
| V3 Web Frontend Security | security headers (CSP, frame-ancestors, nosniff, referrer), cookie attributes, CORS, cross-site requests refused, redirects on the site only | `SecurityHeaders`, `SecurityCors`, much of `SecurityAccess` |
| V4 API and Web Service | method override, content types, WebSocket origin checks | `SecurityRest`, the `/notws` features |
| V5 File Handling | upload type and size, download disposition, path traversal | the multipart and `..` features |
| V6 Authentication | password rules (after NIST 800-63B), rate limiting and lockout, one error text for every refusal, second factors, password reset, no default credentials | `SecurityAuthn` |
| V7 Session Management | new session id at sign-in, logout ends the session, timeouts, re-authentication for sensitive changes | `SecurityAuthn`, the session Rust tests |
| V8 Authorization | denied by default, checked on the server, objects and organizations held to the caller | `SecurityAccess`, `SecurityRest`, `SecurityScreens` |
| V10 OAuth and OIDC | the sign-in client: state, PKCE, nonce, ID token checks | none yet |
| V11 Cryptography | password hashing; the field-encryption gaps recorded as Gap | the `crypto_tests.rs` and `password.rs` tests |
| V12 Secure Communication | the TLS listener's versions, HSTS, outbound certificate checks | none yet |
| V13 Configuration | production defaults, no debug endpoints, error pages without internals | `SecurityHeaders`, `SecurityScreens` |
| V14 Data Protection | cache control on private answers, secrets out of URLs | a few in `SecurityHeaders` |
| V15 Secure Coding and Architecture | the outbound address guard (SSRF), dependency checks | none yet as proofs |
| V16 Security Logging and Error Handling | sign-in events logged, secrets out of logs, no stack frames in answers | the A10 features |

The existing features are mapped first. Most become proofs under V3, V4, V6, V7, V8 and V13 with no new test code.
Those that check an internal rule with no standard behind it stay in `test/client` as regression tests.

## Phases

- **P0** (this plan).
- **P1, the matrix.** Read the ASVS 5.0.0 level 1 and 2 requirements, give each a status for the framework and for
  PASS, and name the evidence. This is the largest analysis step. PASS's statuses lean on the security review's PASS
  units (round 3, C1 to C9) and the Apps-PASS maps.
- **P2, the harness** (built). `test/proofs` as a suite kind, `--proofs`, the per-requirement summary in JSON and
  Markdown, the deployment profile; [AGENTS.md](../AGENTS.md), Build-Test and JS-Test-Client say what the third kind
  is. As built: `moqui test-client <component> --proofs` reads only `test/proofs/` (no pool with `test/client/`); its
  `suite.js` declares the server and names the matrix (`matrix: "..."`); `--profile fixtures|deployment`, fixtures
  with `--serve` and deployment otherwise, reaches the specs as `profile`, and `needsFixtures(what)` skips a feature
  in the deployment profile; the run writes `proofs.json` and `proofs.md` into its output directory, each matrix row
  with its proofs and a result (passed, partial, failed, skipped, none; an Inherited row takes the other components'
  proofs); a passing feature filed under no requirement fails the run; `./moq test-framework` and `./moq test-apps`
  run `--serve --proofs` beside the client specs; a `proofs` line in `test/budget` limits them. Seed proofs:
  `V6Authentication` (6.2.6, 6.2.7, 6.3.2) and `V7SessionManagement` (7.4.1).
- **P3, the move** (done). The current security specs move into proofs by requirement, renamed by id; the parts that
  are regression only stay in `test/client`. As done: of the seven specs' 121 features, 32 moved (four as the control
  of another proof), into `V1Encoding`, `V3WebFrontend`, `V4ApiWebService`, `V5FileHandling`, `V6Authentication`,
  `V8Authorization`, `V9SelfContainedTokens`, `V13Configuration`, `V14DataProtection` and `V16Logging`, beside the
  seeds; `SecurityCors` moved whole. A feature moved only under a Framework row: one that shows part of a Gap row
  (7.2.4, the CSP rows 3.4.3 to 3.4.6, deny by default 8.2.1) stays a regression test until the gap closes, and so do
  the tests of internal findings, framework-specific routes (`SecurityDevice`, `/status` from loopback) and Java-port
  details. Moving tightened three checks (`no-store`, the token's 401, a Content-Disposition present) and gave four
  refusals a control. Every proof that signs in takes the deployment's account or calls `needsFixtures`. The
  framework's `test/budget` split its client budget, its total kept: `client = 3.5`, `proofs = 2.5`. The matrix rows
  for the findings fixed since P1 (SEC-PFS-82, SEC-TLS-81, SEC-TLS-82) were corrected, PASS's V5.3.1 to Inherited.
- **P4** (done). New proofs for the level 1 requirements with none: the framework's, then PASS's. As done
  (2026-11-18): every level 1 row of either matrix that is Framework, Configuration or PASS has a proof, except the
  documentation rows (6.1.1, PASS's 8.1.1), which a test does not show.
  - The framework, black-box: 2.2.2 (`V2Validation`), 3.5.2, 6.2.2, 6.2.3, 6.2.8, 6.4.2, 7.2.1, 7.2.2 and 7.2.3, the
    password proofs on a fixture user `sec.pw.proof` whose password ClientFixtures sets again on each run.
  - The framework's Configuration rows, proved against a deployment: 13.4.1 (no `.git` served) runs everywhere; 4.4.1,
    12.2.1 and 12.2.2 need a deployment over https and skip otherwise; 6.3.1's per-client sign-in limit is opt-in
    (`--param bruteForce=yes`), as it limits the client. Specs read `server.insecure` for 12.2.2.
  - The framework, in-process: 12.1.1 (`tls_tests.rs`), 9.1.2 and 9.1.3 (`sso_tests.rs`, against a mocked provider),
    the generator half of 7.2.3 (`session_tests.rs`), 6.4.1 (`ResetPassword`, a framework spec reading the mailed
    reset password from the capture). V1.3.2 and V5.3.1 became Framework when SEC-SCR-81 was fixed, proved by the
    `WikiPageTypes` spec.
  - What the proofs found: a mailed reset password could be set as the new long-term password (6.4.1), since
    `update#PasswordInternal` did not compare the new password with `resetPassword`: fixed the same day, and
    `ResetPassword` proves the refusal; a missing required field of
    the password change is answered HTTP 500; the OIDC allowlist is the provider's metadata, not the deployer's; a
    `discoveryUri` over plain http to another host is not refused.
  - PASS (mr-pass `test/proofs`): 3.2.2, 3.5.1 (twice), 6.2.2, 6.4.1 (a registration code), 7.4.1, 6.3.1 (PASS's own
    limit by account name, and the opt-in per-client limit at its route) and 1.3.2 (`Forum/TextNotCode`, in-process).
- **P5** (done). The same for level 2. Framework part done (2026-11-18): every level 2 row of the framework that is
  Framework or Configuration has a proof, 19 black-box features (13.4.3, 14.2.3, 3.5.5, 3.7.1, 4.2.1, 6.2.9, and the
  Configuration rows 13.4.4, 3.4.5, 3.5.4, 4.1.3, 15.3.4, 2.4.1, 6.3.3, 15.2.3, against a deployment) and 23 Rust tests
  (encoding, unsafe sites, cryptography, TLS suites and clients, the cluster port, egress, logging, the SFU's DTLS and
  SRTP, OIDC signatures and audiences). 15.3.3 is Framework since SEC-RTE-81 was fixed (`MultiPin`). The proofs found
  four rows not met, now Gap: 6.6.3 (wrong codes sent at once are not all counted), 12.1.2 (the listener does not
  prefer its own suite order), 6.8.2 (the mail client's OAuth sign-in does not check the ID token's signature), and,
  noted with their rows, a TLS failure logged without its reason (16.3.4), TRACE rendering screens (13.4.4), an
  IP-allowed refusal that confirms the password, and forwarded headers read from their first value behind a proxy.
  PASS part (mr-pass `899c04e`, `b0829a1`): every PASS level 2 row that can take a proof has one (black-box in its
  `test/proofs`, 3.5.5 in Chromium; in-process in `SourceRules`, `Transport` and mr-moqui's authority client test for
  12.3.2); the documentation rows 6.1.3 and 7.1.3 are not tests; 16.2.2, 16.4.2 and 16.4.3 are Inherited. 3.4.3 stays a
  Gap (`base-uri 'self'`). The findings of P4 and P5 are in the register (SEC-WEB-87 to 93 and the others its status
  paragraph names), and the matrix rows cite them.
- **P6** (done, 2026-11-18). Cross-references (the API Top 10 for `/rest`, the LLM Top 10 for the agents, NIST
  800-63B for authentication, the CWE Top 25) and the certifier's guide. As done: `docs/security/proofs/Cross-References.md`
  maps each item of the five standards to the matrix rows with their status and proofs, ids and names only;
  `Certifier-Guide.md` says how a builder or certifier runs the proofs against a deployment (the account, the
  parameters, the opt-in proofs, reading `proofs.md`, the settings each Configuration row asks for, the in-process
  proofs from a checkout, what is not covered). The documentation rows (6.1.1; PASS's 6.1.1, 6.1.3, 7.1.3, 8.1.1)
  cite the document that meets them, and `Attack-Surface-Web.md` now names lockout abuse, which 6.1.1 asks for.
- **P7, BOSS.** The business applications (mr-boss, with the Mantle services behind them) through the same steps as
  PASS: a matrix at levels 1 and 2 in mr-boss `docs/` (statuses Inherited, BOSS, Configuration, Gap, N/A; most rows
  inherit the framework's), round 3's BOSS units and the Apps-BOSS map as its evidence, then proofs in mr-boss
  `test/proofs` for the rows BOSS meets with its own code (its staff access walks, the record files, the payroll and
  bank changes, the share links), black-box where a client can show it.
- **P8, the clients.** The desktop and mobile apps and the Tauri client (`crates/moqui-desktop`, `moqui-mobile`,
  `moqui-client-tauri`, with `moqui-client`) against OWASP MASVS 2.x (CC BY-SA 4.0, ids and links only): storage,
  cryptography, authentication, network, platform, code, resilience and privacy. A matrix of its own, since MASVS is a
  standard of its own; proofs as Rust tests and the clients' own test harnesses, as these are not served pages.

## Decisions

The owner's, 2026-11-18:

- Proofs are a third kind of test, in their own `test/proofs` directory beside regression and acceptance.
- ASVS 5.0.0 is the spine. The Top 10, CWE, the API and LLM Top 10s and NIST 800-63B are cross-references.
- The repository holds ids and links only, never a standard's text.
- The matrix covers the framework and PASS, at levels 1 and 2.

Still open, for later phases: level 3, and whether the documents are published as a site.
