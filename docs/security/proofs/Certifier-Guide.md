# Certifying a deployment

How to use mr-moqui's security proofs to check an application built on it against the OWASP Application Security
Verification Standard (ASVS) 5.0.0 at levels 1 and 2. For a builder preparing a deployment, and for a certifier or
auditor checking one. What the statuses mean is in the [index](README.md); how the proofs run is in
[JS-Test-Client](../../dev/JS-Test-Client.md#security-proofs).

## What you get

- **The matrix.** Each ASVS requirement at levels 1 and 2 with mr-moqui's status: *Framework* (met by default),
  *Configuration* (met when the deployment sets something, named in the row), *Application* (the builder's to meet,
  with the framework's tool for it), *Gap* (not met today, with its entry in the security review's
  [register](../../../plans/Security-Review.md)), *N/A*. The framework's is in this directory, one file a chapter;
  PASS's, the public application, and BOSS's, the business applications, are in their own repositories
  ([mr-pass](https://github.com/moqui/mr-pass/blob/master/docs/ASVS-Matrix.md),
  [mr-boss](https://github.com/moqui/mr-boss/blob/master/docs/ASVS-Matrix.md)). An application of your
  own starts from the framework's rows: its Application rows are yours, and its Configuration rows are your deployment's.
- **Black-box proofs.** Client specs that run against any server, your deployment included, and write their results
  by requirement. These are the ones you run.
- **In-process proofs.** Rust tests and framework specs, for what a client cannot see (how a password is hashed, what a
  TLS listener offers, what a log holds). They run in the project's builds; the matrix row names each, and you can run
  them from a checkout (below).
- **Cross-references.** The OWASP Top 10, the API and LLM Top 10s, NIST SP 800-63B and the CWE Top 25, each mapped to
  the ASVS rows: [Cross-References](Cross-References.md).

## Before you run

- **An https address.** The TLS proofs (4.4.1, 12.2.1, 12.2.2) need `--server https://<host>` and skip on plain http.
- **An account made for the run.** A low-privilege account, not a person's: the proofs sign it in and out (a sign-out
  ends the account's other sessions), and some store and blank a preference. Pass its name with `--param user=` and
  its password in the environment, `MOQUI_TEST_PASSWORD`, so that it stays out of `ps` and the shell history.
- **A checkout of the version you deployed**, for the proof files and the binary (`moqui test-client` is in every
  package). The client opens no database and writes only its output directory, so it can run from any machine.

## Running the proofs

```sh
MOQUI_TEST_PASSWORD=... moqui test-client framework --proofs --server https://erp.example.com --param user=auditor
MOQUI_TEST_PASSWORD=... moqui test-client mr-pass --proofs --server https://shop.example.com --param user=auditor --param site=SHOP
```

Against a server the run did not start, the profile is *deployment*: no test fixture user signs in, and a proof that
needs the project's fixture data skips and says what it needs. The parameters a proof reads:

| Parameter | Proofs | What it is |
| --- | --- | --- |
| `user`, `password` | most that sign in | the account for the run (`MOQUI_TEST_PASSWORD` for the password) |
| `httpUrl` | 4.4.1, 12.2.1 | the plain-http address, when it is not port 80 |
| `siteUrl` | 3.5.4 | another application's host, to check it serves none of the framework's apps |
| `flood`, `floodPath`, `floodMax` | 2.4.1, 15.3.4 | opt-in (`flood=yes`): a burst against the per-client rate limit |
| `bruteForce` | 6.3.1 (framework and PASS) | opt-in (`bruteForce=yes`): sign-in tries against the per-client login limit |
| `site`, `accountPath`, `siteHost`, `forumPath` | PASS | the PASS site, its account path, its host and its forum |
| `authorityUrl`, `clientId` | PASS | the PASS authority and a registered client (`MOQUI_TEST_CLIENT_SECRET` for its secret) |
| `mfa` | PASS 6.3.3 | `required` when the deployment requires a second factor |

The opt-in proofs change the client's standing: a burst or a run of failed sign-ins gets the client rate-limited for a
minute, and twenty failed sign-ins in ten minutes ban it for fifteen. Run each alone, once, after the rest:
`--spec V6Authentication --feature v5.0.0-6.3.1 --param bruteForce=yes`. Proof 6.3.2 tries the demo data's well-known
account once; a deployment without the demo data refuses it, which is the proof.

A deployment that requires a second factor for every account (6.3.3) refuses the run's account a password sign-in:
the proofs that sign in then fail with "needs a second factor", since the harness passes no code. Run those with an
account outside the requirement, or read them as not run.

## Reading the result

The run writes `proofs.md` and `proofs.json` into its output directory (`--out`): every row of the matrix with the
proofs of it and a result. Attach them as evidence.

| Result | Means |
| --- | --- |
| passed | every proof of the requirement passed against your deployment |
| partial | some passed and the rest skipped |
| failed | a proof failed: on a Configuration row your setting is missing; on a Framework row, report it to the project |
| skipped | none could run here; each says what it needs (https, an account, an opt-in, the project's fixtures) |
| none | the run has no proof of it: see the row's Proof cell for an in-process proof, or it is not proved yet |

## The settings a deployment makes

The Configuration rows, and what each asks of you. The row in the matrix names the exact setting.

| Requirement | What to set |
| --- | --- |
| 4.4.1, 12.2.1 | the HTTPS listener (a certificate in `runtime/cert`, or ACME) or a TLS proxy, `webapp-list/webapp[webroot]@https-enabled`, and no published plain-http port but a redirect |
| 12.2.2 | a publicly trusted certificate: ACME on System › Certificates, or the operator's own |
| 13.4.1 | deploy from `./moq archive` or the binary, not a checkout |
| 2.4.1, 6.3.1, 13.4.4 | `webapp-list/webapp[webroot]/web-protection@enabled` (or `@trusted-proxies`): per-client rate limits, bans and the methods list; add the WebDAV methods to that list if `/dav` is used (SEC-WEB-89) |
| 3.4.5 | a `Referrer-Policy` response header (`response-header` of type `screen-render` under the webroot webapp) |
| 3.5.4 | a host per application: a `root-screen` per `host` with `framework-routes="refuse"` |
| 4.1.3, 15.3.4 | behind a proxy, `@client-ip-header` with `@trusted-proxies`; a proxy that sets, not appends, the forwarding headers (SEC-WEB-91) |
| 6.2.10 | `user-facade/password@change-weeks` 0 |
| 6.3.3 | `requireAuthcFactor` Y on the groups that must use a second factor (`ALL_USERS` for everyone) |
| 12.3.1 to 12.3.3 | TLS on every link: `mail-delivery@mx-require-tls` or MTA-STS, datasource `sslmode` `verify-full`, an https search URL, the cluster's TLS port |
| 13.2.4, 13.2.5 | an egress firewall: the framework refuses internal addresses on production but keeps no allowlist of its own |
| 15.2.3 | a production build (`./moq build --enterprise` or without `devtools`) and `instance@purpose` production |
| 16.2.2, 16.4.2, 16.4.3 | a synchronized clock; log files left at their defaults (0600 in a 0700 directory); `telemetry@otlp-endpoint` to ship logs |

The [production checklist](../../Run-Deploy.md#production-checklist) covers these and the rest of a deployment.

## In-process proofs from a checkout

Each matrix row names its in-process proofs. To run them yourself on the version you deployed:

```sh
cargo test -p moqui-host --lib tls::tests          # a Rust module's proofs, by its module path
./moq test --nobuild -- framework --spec ResetPassword   # a framework spec
./moq test-framework                                  # all of them, with every other regression test
```

## Requirements a test does not show

Some requirements ask for a document, not a behavior (6.1.1, 6.1.3, 7.1.3, 8.1.1). Their rows name the document that
meets them, in [docs/security](..) and in PASS's `docs/`; read it as part of the evidence.

## The desktop and mobile apps

The apps are held to OWASP MASVS, not ASVS: [the clients' matrix](clients/README.md) gives each control a status for
the mobile app and for the desktop app (profiles L1, L2 and P; R is not covered). Their proofs are tests, not specs a
run takes against a server: `./moq test-clients` runs the client crates' tests, the shared client's TLS proofs and the
static checks of the Android app's settings among them (those two also in every `./moq test-framework`). Most rows are
Gaps today, each naming the part not met and its register entry; a certifier of an app build reads those first.

## What this does not cover

- **Level 3.** The matrix covers levels 1 and 2.
- **Your application's own code.** The Application rows are yours to meet and prove; the framework's tool for each is
  named in its row. A proof of the framework's sign-in page proves nothing of an application's own.
- **The Gaps.** A Gap row is a known shortcoming, with its register entry; check whether a later version closes it.
- **What only the server sees** is proved in-process, from the project's builds, not by your run.
