# Security proofs

The framework's standing against the OWASP Application Security Verification Standard (ASVS) 5.0.0, requirement by
requirement, and the proofs that show it. The plan is [Proofs-Initial](../../../plans/Proofs-Initial.md).

Status 2026-11-18: the responsibility matrix (phase P1) is written for levels 1 and 2, the harness that runs the
proofs (P2) is built, and the framework's security client specs that prove a requirement are moved into proofs (P3):
42 features under 20 requirements, in eleven chapter files.
A status is one reading of the code at mr-moqui `bf7034ec9`, mr-pass `d2dfa67` and mr-mantle `dff2c9fe`. A Framework
status is a claim until a proof shows it, and a Gap closes when its fix lands and a proof shows it.

## Who this is for

- **The project:** where the framework meets the standard by default, and where it does not.
- **Builders and certifiers:** which requirements mr-moqui meets for an application built on it, which the
  deployment's configuration or the application itself must meet, and tests to run against a deployment.

## Reading a chapter file

Each file is one ASVS chapter, with a table for each of its sections. A row is one requirement:

| Column | Holds |
| --- | --- |
| Requirement | The ASVS id with its version, `v5.0.0-<chapter>.<section>.<requirement>`, as the standard asks to cite it. |
| L | The requirement's level: 1 or 2. Level 3 is not covered yet. |
| Topic | What the requirement is about, in our words. The requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). |
| Status | One of the five below. |
| Evidence | For Framework, the control (a file and its function or rule); for Configuration, the setting by its conf selector; for Application, the framework's tool for it; for Gap, the [register](../../../plans/Security-Review.md) entry, the `known_gap_` test or what was seen; for N/A, why. `(map)` marks evidence taken from an [attack surface map](../Attack-Surface.md) file and not checked again in code. |
| Proof | The existing test that shows a Framework status, or `to write: black-box` (a client spec can show it from outside) or `to write: in-process` (only a Rust test or framework spec can see it). |

A requirement met only in part takes the status of the part that is not met, and its evidence says what is met.

## Statuses

| Status | Meaning |
| --- | --- |
| Framework | Met by default, with no setting changed. |
| Configuration | Met when the deployer sets something; the setting is named. |
| Application | The builder's to meet; the framework's tool for it is named. |
| Gap | Not met today. |
| N/A | Does not apply to mr-moqui, with the reason. |

## The chapters

| Chapter | Requirements | Framework | Configuration | Application | Gap | N/A |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [V1 Encoding and Sanitization](V1-Encoding-and-Sanitization.md) | 27 | 12 | 0 | 3 | 7 | 5 |
| [V2 Validation and Business Logic](V2-Validation-and-Business-Logic.md) | 11 | 2 | 1 | 8 | 0 | 0 |
| [V3 Web Frontend Security](V3-Web-Frontend-Security.md) | 19 | 9 | 2 | 1 | 7 | 0 |
| [V4 API and Web Service](V4-API-and-Web-Service.md) | 10 | 4 | 2 | 0 | 2 | 2 |
| [V5 File Handling](V5-File-Handling.md) | 9 | 2 | 0 | 2 | 5 | 0 |
| [V6 Authentication](V6-Authentication.md) | 35 | 17 | 3 | 2 | 12 | 1 |
| [V7 Session Management](V7-Session-Management.md) | 18 | 8 | 0 | 3 | 7 | 0 |
| [V8 Authorization](V8-Authorization.md) | 7 | 1 | 0 | 3 | 3 | 0 |
| [V9 Self-contained Tokens](V9-Self-contained-Tokens.md) | 7 | 6 | 0 | 0 | 1 | 0 |
| [V10 OAuth and OIDC](V10-OAuth-and-OIDC.md) | 29 | 4 | 0 | 0 | 4 | 21 |
| [V11 Cryptography](V11-Cryptography.md) | 14 | 2 | 0 | 0 | 12 | 0 |
| [V12 Secure Communication](V12-Secure-Communication.md) | 9 | 3 | 5 | 0 | 1 | 0 |
| [V13 Configuration](V13-Configuration.md) | 13 | 5 | 4 | 0 | 4 | 0 |
| [V14 Data Protection](V14-Data-Protection.md) | 9 | 2 | 0 | 2 | 5 | 0 |
| [V15 Secure Coding and Architecture](V15-Secure-Coding-and-Architecture.md) | 13 | 2 | 2 | 4 | 5 | 0 |
| [V16 Security Logging and Error Handling](V16-Security-Logging-and-Error-Handling.md) | 16 | 7 | 3 | 0 | 6 | 0 |
| [V17 WebRTC](V17-WebRTC.md) | 7 | 3 | 0 | 0 | 4 | 0 |
| **All** | 253 | 89 | 22 | 28 | 85 | 29 |

## Running the proofs

The framework's proofs are in
[`runtime/components/base/framework/test/proofs/`](../../../runtime/components/base/framework/test/proofs), a spec
file for each ASVS chapter, each feature filed under the requirement its name starts with. They run in every `./moq
test-framework`, and alone:

```sh
moqui test-client framework --proofs --serve          # the project's run: a server with the test fixtures
MOQUI_TEST_PASSWORD=... moqui test-client framework --proofs --server https://erp.example.com --param user=auditor
```

Against a server the run did not start, the profile is *deployment*: no test fixture user signs in. A proof that needs
an account takes the one `--param user` names, with its password in `MOQUI_TEST_PASSWORD`; one that needs the
project's fixture data skips and says what it needs. Use a low-privilege account made for the run: a proof signs it in
and out, and the 6.3.2 proof tries one sign-in of the demo data's well-known account. The run writes `proofs.md` and
`proofs.json` into its output directory (`--out`): every row of this matrix with the run's proofs of it and their
result, to attach as evidence. The harness is described in [JS-Test-Client](../../dev/JS-Test-Client.md#security-proofs).

PASS, the public application, has its own matrix in its repository:
[mr-pass ASVS matrix](https://github.com/moqui/mr-pass/blob/master/docs/ASVS-Matrix.md). A PASS requirement the
framework meets is marked inherited there and links here.

## The standard and its license

ASVS is © The OWASP Foundation and published under the [Creative Commons Attribution-ShareAlike 4.0
International](https://creativecommons.org/licenses/by-sa/4.0/) license. These files cite its requirement ids and
levels and link to it; they do not copy its text. Everything here is CC0, as the rest of mr-moqui.
