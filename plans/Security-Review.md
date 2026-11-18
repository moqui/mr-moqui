# Security review

Status 2026-11-15: first pass done, in two rounds. The attack surface map was split by area and every slice of the
code has been read at least in part by a reviewer; the suspected weaknesses are listed here. Most are as their
reviewer reported them and have not been read a second time; the Status column says which. The twelve High findings
(ten fixes: two pairs are one weakness) are fixed, each with its regression test; its section's **Fix** says how. On
2026-11-16 the Medium findings were fixed in the same way, with follow-ups the same day: all 77 are fixed (SEC-RTE-02
and SEC-OUT-08 finished later the same day), each entry's **Fix** in [Security-Review-Medium](Security-Review-Medium.md)
saying how. The Low findings that were confirmed in code were then fixed in batches the same day, each entry's **Fix**
in [Security-Review-Low](Security-Review-Low.md) saying how, and two batches of the owner's decisions on the ones they
left were carried out after them, and on 2026-11-17 the ones fixed in part were finished: of the 215 Low and Info
findings 158 are fixed and 5 deferred, none refuted, and 52 (12 Low, 40 Info) are as reported. What is left is under
[What is left](#what-is-left).

A third round began 2026-11-17: it reads the code the first two did not reach (the "not reached" of the
[coverage](#coverage) table and the map files' open questions), in about 60 units, and records what it finds without
fixing it. Its findings have ids from 81 up in each area. The counts above and the [counts](#counts) table are of the
first two rounds until the third is recorded whole; [method](#method) step 7 says how it is run.

This is the findings register of the security review that the pre-0.1.0 notice in the [README](../README.md) names:
until 0.1.0, findings are recorded openly in this repository. The map of what can be reached is
[Attack-Surface](../docs/security/Attack-Surface.md) and its area files; this document holds what looks wrong with
it. The two secrets plans, [Sealed-Secrets](Sealed-Secrets.md) and [Vault-Instances](Vault-Instances.md), were
reviewed in the same pass and changed in place.

Sections: [method](#method) · [the findings that matter most](#the-findings-that-matter-most) ·
[coverage](#coverage) · [found sound](#found-sound) · [what is left](#what-is-left) · [counts](#counts) ·
[findings by area](#findings-by-area) · [Critical and High findings](#critical-and-high-findings). The Medium
findings in full are in [Security-Review-Medium](Security-Review-Medium.md), and the Low and Info ones in short form
in [Security-Review-Low](Security-Review-Low.md).

## Method

1. Three inventories (framework listeners, routes, outbound clients and child processes; the base and dev components;
   mr-boss and mr-pass) compared what the code has with what the map said.
2. `docs/security/Attack-Surface.md` was split into an index and one file for each area, text unchanged.
3. Reviewers each read one slice of the code and the map text for it, and wrote a report with four lists: what the
   map lacks, what in it is stale, suspected weaknesses, and what was not settled. No reviewer ran anything: every
   item is from reading code, with the lines quoted.
4. For two map files (Web and Data) a second reader checked the report against the code, wrote what held into the map
   and gave each weakness a verdict. The lead read the code for the findings marked "(lead)".
5. In a second round, on a smaller budget, one reader for each remaining slice read the code and wrote what is
   reachable straight into the slice's map file, with the suspected weaknesses in a short report. Those map files and
   findings have had no second reading.
6. The map files whose first-round reports had not been applied (Routes, Admin, Tools, AI, Git, Comms,
   Cluster-Edge, and the PASS frontend and authority) were then written from those reports by one writer each, who trusted an item whose quoted code was
   specific and read the code only where it was not.
7. In a third round (2026-11-17) the code no reviewer had reached was cut into about 60 units, by exposure: what a
   network peer or an anonymous caller reaches in the framework (19 units), what a signed-in user and the clients
   reach (17), PASS (9), and BOSS with the Mantle services behind it (15). One reviewer read each unit and its map
   text, ran nothing, and wrote a report in the form of step 3, with the controls found sound and the map's open
   questions answered from the code. A unit over about 20,000 lines was read from what untrusted input reaches
   inward, and its report says what was read whole, sampled or not reached. The lead read the cited lines of the High
   and most Medium candidates before they were recorded: an entry's **Re-check** says which lines and what was not
   read, and its status says `(lead)`; a Medium with no Re-check, and every Low or Info one, is as its reviewer
   reported it. One writer for each area then wrote the entries and brought the
   area's map file up to date from the reports. Nothing was run and nothing found in this round is fixed. An entry's
   lines, names and settings are those of the commit its unit read: mr-moqui `f6a7de2f9` for units A1 to A19 and B1 to
   B11, before the settings became conf attributes (`plans/archive/Conf-Paths.md` §6 maps each old name to its
   selector); the units after them read the tree of the merge `40c0b234e` or later, and their reports name it. The
   map files use the selectors throughout.

Status values:

- **reported**: as the reviewer wrote it, with the reviewer's own confidence in brackets (confirmed in code, likely,
  to check). Not read a second time. Treat it as a lead to check, not as established.
- **confirmed**, **likely**, **not confirmed (second check)**: a second reader's verdict after trying to refute it.
- **confirmed**, **likely (lead)**: the lead read the lines named in the finding's Re-check.
- **refuted**: read again and found not to hold; listed at the end.

Severity: Critical, an unauthenticated caller gets code execution, another user's session, or bulk data. High, an
anonymous write or read of private data, or a low-privilege user reaching another organization's data or a privileged
action. Medium, needs a signed-in victim's click, an administrator's mistake, or leaks limited data. Low, hardening.
Info, worth writing down. A reviewer's severity is kept unless a second check changed it.

The reports, the briefs the reviewers and writers worked from, the prompts each slice was given, and the script
that builds this document are in `plans/security-review-evidence/`, which is not in the repository's history
(`.gitignore`): a working record on the machine that ran the review.

## The findings that matter most

No finding is rated Critical. These are the ones to look at first; each High one has its section below, each Medium one in
[Security-Review-Medium](Security-Review-Medium.md). Every High one below is fixed (2026-11-15), and every Medium one
(2026-11-16).

- **A disabled account comes back** (SEC-WEB-01, SEC-ADM-01; High; confirmed by the lead). An account disabled for
  good becomes a five-minute lock after four failed sign-in attempts, and a password change writes `disabled=N`. An
  offboarded user who still knows the password signs in again.
- **A link runs script as a Tools administrator** (SEC-TLS-01, and SEC-SCR-01 from the scripts slice; High; confirmed by the lead). `Tools/Data/Export`'s
  `EntityExport` is a `read-only` transition, so it runs on a GET with URL parameters and no session token, and it
  evaluates its `filterMap` parameter as script with the full `ec`, which can start a process on the host.
- **State changes on GET** (SEC-WEB-04, SEC-TLS-04, SEC-RTE-04, SEC-RTE-05, and the same rule behind several others;
  Medium; the rule confirmed by the lead; fixed 2026-11-16: such a GET another site started is refused). The session token is checked on POST, PUT, PATCH and DELETE only. A
  transition with actions and the default method runs on a GET with no query string, and a `read-only` transition
  takes URL parameters, on a `SameSite=Lax` cookie. Reported as reachable this way: clearing caches, reloading the
  conf, logout, mailing a rendered report to a given address, the `/resources` file commands, and 57 PASS transitions.
- **Hosted-instance rows start host processes** (SEC-TLS-02; High; reported). System › Instance, which the map did
  not have, runs a stored command as a host process and expands row values as script.
- **Git import reads another repository or a server directory** (SEC-GIT-01; High; reported), and **a pipeline step
  inherits its runner's token** (SEC-GIT-02; High; reported).
- **`GET /email/<id>` marks any message viewed**, anonymous and by a sequenced id (SEC-RTE-01, High, and SEC-MAIL-03;
  reported): a message waiting in the Outbox then leaves the send queries.
- **Any Comms user can host a mail account at any unclaimed address** and receive its mail once inbound SMTP is on
  (SEC-MAIL-01; High; reported).
- **A request's file name is expanded as script when a screen is mailed** (SEC-MAIL-61; High; reported, not run),
  and **mr-marble's party mail screen returns any stored message's body by its id** as inline HTML (SEC-MAIL-63;
  High; reported).
- **Text from records is expanded as script** in more places than the two above (SEC-SCR-05; High; reported): a
  notification's title is expanded, and Mantle builds some titles from record values. `System.getProperty` gave any
  server script every property, the secret ones included (SEC-SCR-03; fixed 2026-11-16: it answers null for a secret).
- **Stored credentials are readable** by an administrator through REST and exports, where the map said write-only
  (SEC-DAT-03, confirmed by the lead; SEC-TLS-05), and a find by request parameters takes conditions on password-hash
  and encrypted fields (SEC-DAT-01). Fixed 2026-11-16: no REST answer or export without `ADMIN_BACKUP_DOWNLOAD` holds
  an encrypted field's value (`<field>IsSet` in its place), and request input takes no condition or order on a password
  field and no order or pattern on an encrypted one.
- **PASS sign-in had no rate limit on the authority** (SEC-PSA-01; Medium; reported; fixed 2026-11-16: 20 sign-ins a
  requester and 10 a user name in ten minutes).
- **`allow-remote` is read by nothing.** The map cited `allow-remote="false"` on about 37 lines as if it kept a
  service unreachable. No Rust code reads the attribute (the service XSD says so). A service is reached from outside
  only through a screen transition, a REST resource, a web script, a token route, the PASS authority's route table or
  an agent tool exposure. The map's Web file says this, and the other files' sentences are corrected or carry a
  note.

## Coverage

| Slice | Code | State | Map file written from it |
| --- | --- | --- | --- |
| 1 Web authentication and controls | `moqui-host/src/http*`, `guard*`, `moqui-core/src/user`, `web` | Read; not reached: `web/servlet.rs`, `screen/gate.rs`, most of `acme/`, SSO | Web: written |
| 2 Routes, screens, REST | `moqui-core/src/screen`, REST, `components/base/webroot` | Read; not reached: `screen/frame.rs`, `web/servlet.rs`, the services behind `/rest/s1/moqui` | Routes: written from the report, no second check |
| 3 Administration, identity, sign-in | `org.moqui.admin.*`, SSO, factors, keys | Read; the bodies of the Configuration, Operation, DataTransfer, Backup, Readiness and Audit scripts only scanned for permission checks | Admin: written from the report, no second check |
| 4 Tools, System, Monitor, Dashboards | `components/base/tools`, `moqui-tools`, `moqui-report`, telemetry | Partial: `moqui-calc`, `moqui-collab`, most of `moqui-report` and `moqui-tools`, backups and most Dashboards screens not reached | Tools: written from the report, no second check |
| 5 LLM, A2A, MCP, Assist | `moqui-llm`, `moqui-host/src/llm` | Partial: the run loop, several tools, the MCP server tool path, gateway quotas and evals not read | AI: written from the report, no second check |
| 6 Git hosting and pipelines | `moqui-git`, `git_http`, `ci_run` | Read; the gix transports and several service files not reached | Git: written from the report, no second check |
| 7 Mail | `moqui-mail`, `moqui-host/src/smtp*` | Read in two rounds; not reached: the SMTP queue beyond its header, mail DNS, the DMARC and ARC decision, IMAP and POP size limits, the OAuth exchange, account owner transfer | Mail: written by its reviewer |
| 8 Files, DAV, SSH, terminal | `moqui-dav`, `moqui-host/src/ssh`, `term` | Read in two rounds; not reached: the shell's interactive prompt, the host session environment, parts of `ssh/server.rs`, the SFTP drives backend and the SFTP client, DAV locks and free-busy, the `pim` parser bodies | Files, SSH: written by their reviewer |
| 9 Calls, chat, SIP, XMPP | `components/base/basic-comms`, `moqui-rtc`, `moqui-sip`, `moqui-xmpp`, `moqui-channels` | Partial: chat bridges and SMS, Web Push and FCM, recording and captions only sampled | Comms: written from the report, no second check |
| 10 Cluster and edge node | `moqui-host/src/cluster*`, `edge*`, `grpc_listen` | Read; pools, takeover, the front node and the bodies of the delegated call and SIP services not checked | Cluster-Edge: written from the report, no second check |
| 11 Clients | `moqui-desktop`, `moqui-mobile`, `moqui-client*`, `moqui-browser`, the browser shell | Read in the second round, on a small budget; not reached: `moqui-browser`, `moqui-embedded`, the terminal client, parts of `moqui-client`, the mobile Kotlin code, the updater's signature check, the rich text editor | Clients: written by its reviewer |
| 12 Data layer | `moqui-entity`, `moqui-db`, `moqui-drivers` | Read; `moqui-core/src/search`, the cache and the load parsers not read | Data: written |
| 13 Script engine, templates, parsers | `moqui-core/src/script`, `resource` | Read in the second round, on a small budget; not reached: the locked template runtime, report and formula expressions, CSV and XLSX output, multipart and JSON limits; parsers and the second half of rendering are the thinnest | Scripts: written by its reviewer |
| 13a Outbound requests, processes, packaging (report `slice-21.md`) | `moqui-lib/src/net.rs`, `http.rs`, `podman/`, `systemd/` | Read in the second round, on a small budget; not reached: the IMAP, SIP, ACME, cluster, edge and PASS clients, SFTP, external databases, log contents, the `moq` downloads in full | Outbound, Process: written by their reviewer |
| 13b Development tools (report `slice-22.md`) | `components/dev`, `moqui-devkit` | Read in the second round, on a small budget; not reached: the LSP, the test client, lint and typegen, the agent tool bodies, how `/devstatic` maps a path, the example services | Dev: written by its reviewer |
| 14 PASS frontend | `mr-pass` | Read; the HTML-injection sweep of the apps other than Smithy not finished | Apps-PASS: written from the report, no second check |
| 15 PASS authority and public accounts | `mr-mantle/script/Pass*`, `org/moqui/public` | Read; 47 operations sampled, first caller check only | Apps-PASS: written from the report, no second check |
| 16 PASS application services | `mr-mantle/service/mantle` portal services | Read in two rounds; the first round's helper readers' items were read again only where they went into the map; not reached: several relay and result pages, the gateway's setup services, parts of the theme code | Apps-PASS-Commerce: written by its reviewer |
| 17 Forum and Smithy | `mr-mantle/service/mantle/forum`, `smithy` | Read in two rounds; not reached: the delivery worker, the Forum import and integrations, repository transfer, branch rules, releases, the templates (how cleaned HTML reaches a page), the git authorization hook | Apps-PASS-Forum-Smithy: written by its reviewer |
| 18 BOSS access model; Admin, Commons, People, Accounting | `mr-boss` | Read in the second round, on a small budget; not reached: the report-data grants, most of the authorization-disabled reads, the share, calendar and intake services, most of Commons, People and Accounting beyond sampled gates, the skills beyond their front matter | Apps-BOSS: written by its reviewer (access model, row isolation, external and device users, token-route screens, Admin, Commons, People, Accounting, skills) |
| 19 BOSS business apps; the Mantle REST root | `mr-boss`, `mr-mantle` | Read in the second round, on a small budget; not reached: Customer Service, Marketing sends and imports, Warehouse, Manufacturing, Products, Maintenance, Insights, the 360 screens; the price, discount, credit and refund services; the existing Sales and Marketing paragraphs were not checked | Apps-BOSS: written by its reviewer (the Mantle REST root, integrations and stored credentials, Commerce, order approval, Projects, shared record panels) |
| 20 The two secrets plans | `plans/Sealed-Secrets.md`, `plans/Vault-Instances.md` | Read whole against the code | the plans: edited in place |

A reviewer's report says under "Read" what it read whole, what it sampled and what it did not reach. The partial
ones above are partial for the reasons their reports give.

## Found sound

Checked by a reviewer and reported as holding (none read a second time unless a finding's Re-check says so):

- Static files under the root: no path escapes their directories.
- Entity REST find parameters do not reach SQL unescaped; the page size is 100 by default and at most 1000.
- A bad token on a token route is a 404 and nothing of the screen runs. A screen's own `condition` is not used as
  access control.
- `returnTo` on the sign-in and SSO transitions is same-origin checked and is never taken from a provider's callback.
- Secret and token comparisons in the web sign-in code are constant time or hash lookups, with one exception
  (a single-use code compared in script).
- The SSO account link and group sync keep the bounds the map states (`link_allowed`, `sso_may_give`).
- A browser cannot set a header that a member behind an edge node trusts: the edge node strips and overwrites them.
- Edge pairing codes have 60 bits, a default life of 60 minutes and 10 tries per source in 10 minutes. Only a
  trunk's `authJson` is replicated to an edge node as a credential, and the edge node has no write-back path.
- Chat link previews apply the internal-address guard to the URL, to each redirect and to DNS answers.
- Chat, call and recording services check membership or ownership in the service, attachment downloads included.
  Uploaded HTML and SVG are not served inline. A call guest cannot start recording or captions.
- HTML mail never reaches the application's document: the client cleans it and shows it in a sandboxed frame with no
  script and its own CSP, and remote images are fetched by the browser only. The mailbox services that were read
  check the account in the service. The mail OAuth callback's state is single use, bound to the session and user.
- Smithy and the Forum, in what was read: no read of a private repository or a confidential issue without a view
  check; the opt-out and held-mail tokens are hashed and bound; claiming an imported GitHub identity needs GitHub's own
  endpoints and the numeric id, and moves authorship only.
- The Assist services check the user, `AgentUse` and ownership of every id (the map's open question on this is
  closed). `ConstitutionServices`, granted to every user, checks a sealed permission in each write.
- The PASS operation allow-list: the three copies of the bearer-write list are equal; every routed operation is in a
  schema; a login key reaches only the 18 app-key operations; a delegation is looked up by token digest, client and
  site; a download's location comes from the Mantle service's answer, never from the request.
- The five `llm_*_api_key` properties are inside XML comments in the default conf, and
  `component_deploy_attestation_key` is treated as a secret by its name.
- The audit log holds ciphertext of an encrypted field; cluster cache invalidation carries keys only; the Tools grid
  masks encrypted fields.
- The router and the servlet match use the raw request path with no dot-segment or `//` handling on either side, and a servlet and its filters are chosen from the same string, so no path was found that reaches a host route's handler past its gate or a script servlet without its filter. A rule with `host` never exempts a connection or a ban, a shared ban never bans a trusted proxy, and `/health`, `/metrics`, `X-Forwarded-Proto` and `X-Moqui-Test-Peer` are believed only as the map states (SEC-WEB-11 and SEC-RTE-10 hold). The parent frame (`screen/frame.rs`) and `reauthenticate#User` hold as the map states.
- SSO sign-in: the callback needs the session's pending state and removes it first; `state` is compared in constant time, PKCE S256 is sent unless the provider lists only `plain`, and the ID token's issuer, audience, expiry, `sub` and nonce are required (huskarl 0.11.10; its JWS verifier and JWKS fetch were not read). An account is linked only by a verified email, never a privileged one, and the app sign-in code is single use, hashed and good a minute. The fixes of SEC-ADM-09 and SEC-ADM-11 hold.
- The edge link's acceptance holds as the map states it: a client certificate's signature is verified before its fingerprint is used, an up connection is taken only for an active key, every `Link` call but Probe, Pair and Hello needs an admitted member of the current generation, a member's own key is proved over the TLS exporter value and pinned by a compare and set in one committed transaction, a linked id named with another key is refused, the down connection needs the generation and its 16-byte token compared without an early exit, a front node refuses the edge set's calls, and a forwarded request's host is a strict DNS name with the client's forwarding headers stripped.
- The cluster port serves nothing before the pinned client certificate and keeps the limits the map gives; a delegated call runs on a local node only when its own conf lists it, as `_edge_`, with `auth*`, `_*` and `edgeCaller` dropped from what the edge node sent; and 18 of the 20 delegated call and SIP services have a guard for `_edge_` (the edge node's own instance id, a call whose media node is edge-linked, a fresh digest answer, or the box a call opened by PIN).
- Over SSH, in what was read: the fixes of SEC-FIL-02 (an app password gets the drives over SFTP only) and SEC-FIL-62 (each shell line is audited) hold; "none" is not an offered method; a dialog connection gets one `apps` on a pty and nothing else, and a cluster connection `cluster <topic>` channels only; a key must be the named account's own and active; a disabled, terminated or display account, or one that must change its password, is refused at a key login and at every later command; russh refuses forwarding, agent and unknown channel types; a host program starts from a cleared environment with a close-on-exec pty master; the TUI copies to the clipboard only on a key or mouse action.
- Over DAV and SFTP, in what was read: a path under another user's name is 404 before any handler, and a `.`, `..` or `/` segment is refused in the request path and in a `Destination`; file content is opened only at the location `get#ResourceInfo` answered, and every DAV write goes through a service that checks `canWrite` or `canUpload` itself; names and hrefs are escaped in XML and the HTML listing, and the request XML refuses a DOCTYPE and nesting past 256; a CalDAV PUT matches a UID only inside the calendar named and a client's ATTENDEE, ORGANIZER and VALARM are dropped; a free/busy-only share is no CalDAV collection; SFTP paths cannot leave the root and writes are bounded per user; the SFTP client refuses a changed or mismatched host key; `create#Drive` has no caller that passes a `localPath`.
- Mail delivery and authentication, in what was read: an MX host's internal addresses are dropped before any connection and the session connects to the address that was tested, not to the name again; with an MTA-STS `enforce` policy or `mail_mx_require_tls` the certificate is checked and nothing goes in clear text; the listener's own `Authentication-Results` cannot be extended by what the client wrote, and `rewrite_head` and the delivery's trust test read the authserv-id with the same function; the edge node answers `RCPT TO` from its copies and never calls the local node, and the local node resolves the recipients again from its own accounts and finds its own `mailOut` item by id and direction, never by what the edge node names.
- The mail client's connections and OAuth, in what was read: every IMAP, POP3 and STARTTLS connection goes through `connect_tcp` (one lookup, the checked addresses connected to, never link-local) with the host name checked in TLS, and a plain sign-in is refused without STARTTLS or STLS unless `storeAllowPlain` or loopback; the OAuth sign-in uses PKCE S256, a redirect URI that is never a request's, an ID token whose audience, issuer, expiry and nonce are checked, and logs or answers no token; `transfer#EmailAccountOwner` asks the owner or `EMAIL_ACCOUNT_OWNER_CHANGE`, and `add#EmailAccountUser` level 3 on the account.
- Git LFS: an upload must be the content of its oid, an oid is 64 hex digits and never a path, an object is read only through a `GitLfsObject` row of the request's repository, the `MoquiLfs` token is bound to its repository and operation and authorized again as its user, the upload body is read only after the write check and the size limit, and a job token gets nothing from LFS. On the wire: a want by id must be a commit the readable refs reach and a `have` is acknowledged only for one, a pkt-line and a push's commands are bounded, a gzip body is held to the request limit as inflated, the outbound smart HTTP client follows no redirect and denies internal addresses, and an SSH mirror connects to the checked address with a pinned key.
- Pipelines and git services: a hidden run is read only by the user who triggered it or internally, a cache is never served by the artifact reads, an untrusted run and any merge request's run get no secret, the run check lowers trust and never raises it, a secret's value and an environment's token are never answered, a job token reaches only its own job run, a step's environment holds no runner credential, `ci.xml` and JUnit parsing take no DOCTYPE and at most 256 nested elements, merges read no `.gitattributes` and run no driver or filter command, `merge#MergeRequest` merges the commit it counted approvals for by id, and no route lets a user who is not ADMIN name a git service to call. The fixes of SEC-GIT-10, SEC-GIT-14 and SEC-GIT-16 hold where they were read.
- A2A, MCP and gateway requests, and every agent pool job, run in a context made for the job with no user (`request_pool.rs:476-480`), so no login carries over to the next job on a thread. Every A2A task read, cancel, subscribe, message and push config is by the caller's user id and the `a2a` surface; a push config's wire object carries no token or credentials. An MCP `requestState` is checked by MAC first, in constant time, then for its version, expiry, user, tool and digest.
- In the run loop and the tools that were read: a step logs in as the run's own user, read from the run's row; every READ tool runs under the read fence; an approved write runs once, for the same user, tool call and arguments; a write left running by a stopped node is never run again; a remote's answer, error and tool descriptions reach the model wrapped as untrusted. The fixes of SEC-AI-02, SEC-AI-03, SEC-AI-08 and SEC-AI-12 hold. Every drawer read is keyed by the caller's user id, memory and personal skills are written only by their owner, and a screen session is found and used only by its owner with every write held by the gate.
- SIP, call media, recording and captions (`round3/A9.md`): the fixes of SEC-COM-25 and SEC-COM-28 hold (the nonce MAC covers the source, a foreign nonce is not a failure, the digest `uri` must be the Request-URI, a trunk's address is never counted or blocked). The source of a request is the connection's peer or the Via as rsipstack rewrote it from the datagram's source. Bindings are made only for an authenticated device, within `max_bindings` and the expiry limits. SRTP checks the replay window and the tag before any state change and is never mixed with or downgraded to plain RTP. The in-house G.711 and G.722 decoders index only with masked values. A recording is started only by a host or the link's owner, read only by its owner and the call's hosts, deleted only by its owner; `store#RecordingFile` takes a spool file only under `runtime/tmp/call-recording` after canonicalizing; an end-to-end encrypted call is neither recorded nor captioned; a guest acts only on its own leg.
- Bridges, texts, pushes and XMPP (`round3/A10.md`, `round3/A11.md`): on a full node a webhook for an unknown, adapterless or disabled bridge is 401 before any parse, and the Telnyx (Ed25519), Twilio (HMAC-SHA1 over the configured public URL, never the `Host` header) and Sinch (HMAC-SHA256) signatures are verified before the replay cache and the parse. The media token is an HMAC over bridge, attachment and expiry, and the file must belong to a message delivered to that bridge. Bridge administration checks `ChatBridgeAdmin` in each service body, and `find#Bridges` returns no `configJson`. Sends are checked and counted before the provider, under the day's row lock. No transition of the unit's screens passes the whole context, so a posted `_isMulti` row replaces no pinned id. Every chat push is encrypted to its device, and its recipients come from member rows, never from a caller; a push endpoint must be https and is posted with internal addresses refused and no redirects. In XMPP nothing but STARTTLS is taken on the plain port before TLS, PLAIN is taken only over TLS with an app password scoped to XMPP, SCRAM compares its proof and binding in constant time and refuses a downgrade, a stanza's `from` is overwritten with the session's own JID, a detached session is resumed only by the same user, and rxml refuses DTDs, entity declarations, processing instructions and comments.
- The `moqui` REST root (`round3/B1.md`): below `/moqui` only the last path node needs an `AT_REST_PATH` record, so a signed-in user with no special group reaches only `git` and `hosting`; a path id replaces a request parameter of the same name, and a REST service call is never a multi call. `components/deploy` checks the HMAC before it parses, counts a wrong signature as a failed sign-in and claims each component once, newer only. `keyRevoke` and the other key services touch only the caller's own rows. `get#EntitySyncData` and `put#EntitySyncData` refuse restricted entities (SEC-DAT-08 holds), a stored `filterMap` or `sendUrl` is checked when written, `/rest/sm` compares signatures and URL tokens by `same_text` and spends a replay key once, and `print#ScreenDocument` reads its parameters as JSON only (SEC-SCR-02 holds).
- Request bodies (`round3/A4.md`): a body is bounded before the read and while it arrives, a declared length over the limit is 413 before any read, an upload with no user is held to the body limit outside the open upload paths (a `..` or `%2e` closes one), and a spool file is `create_new` with mode 0600, named by the process and a counter, and removed when the upload is refused or cut short.
- Parsers and spreadsheet output (`round3/A4.md`, `round3/B8.md`): `moqui_lib::node` refuses a DOCTYPE, an `<!ENTITY` and every other declaration, defines no entity, decodes only the five named ones and character references of at most 16 bytes, stops nesting at 256 and interned names at 16,384 a thread, and the DAV reader is that parser. JSON bodies keep serde_json's depth limit of 128 and become decimals with no float. A multipart spool file is `create_new`, mode 0600, never named from the request, and is removed when an upload is refused; a file part's name loses its directories. Images are refused over 30,000 a side in and 16,383 a side or 50,000,000 pixels out, a rich-text image over 8 MiB before its bytes are read. Git archives check names, depth, entries, bytes, disk and concurrency before anything is written, and keep a link only inside. EDI text gives errors, not panics; Code 128 values stay inside the pattern table and barcode SVG text is escaped. The xlsx render mode and the workbook writer write text as a shared string, never a formula; the screen csv mode guards a single widget's value and the headers, the report CSV every text cell, and the BOSS report exports every cell but a plain number in a number column. The IronCalc engine has no file, network, process or service call, bounds evaluation depth, and its xlsx reader expands no XML entity.
- Templates and themes (`round3/B9.md`): the locked sandbox has no `ec`, host object or module loader, only the clock, a discarding `write`, the deadline and three host functions; its global graph and the compiled function are frozen before any template runs, and a caller can only lower its deadline and memory. A theme's limits only tighten the operator's; a package path fits a fixed layout with no `..`, and names are looked up in the package, never as a location; a template's source and every render's output are refused for script, frames, forms, event attributes, `javascript:` and non-image `data:` URLs and unapproved origins; a fragment placeholder needs the page's nonce; rich text goes through `safe_html::clean` and SVG through an allow-list. `runAsSystem` is refused outside a service's own code, `print#ScreenDocument` reads `screenParametersStr` as JSON only (SEC-SCR-02's fix holds), a script's `RegExp` polls the interrupt every 10,000 steps, and a `.fancy` or `.fancy-locked` file under a screen is never sent as a static file.
- Configuration, sign-in providers, connections, readiness and audit (`round3/B2.md`): a read of a connection or a provider returns no secret, and a connection's URL is answered without credentials; the endpoint-change rule clears every stored secret not entered again, and a test sends nothing until it is; a holder of `ADMIN_CONNECTION_MANAGE` alone cannot re-enable a provider it repointed, and enabling one needs `ADMIN_ACCESS_MANAGE`, a passing test of the configuration as it is, the group checks and `HELD_BEYOND`; a mapped or default group is a registered bundle the actor may assign, never privileged or sealed; an existing account is linked by the provider's `email` claim, never a mapped field; no service name comes from a request (an adapter, a message handler, a readiness provider, a refresh or an audit source is taken from its row); a secret setting answers no value and a setting's own `managePermission` is asked; the audit read answers only registered projections and never a value of an encrypted or secret-named field; `call#ChangeSource` and the readiness services are granted without inheritance.
- Operations, transfers and backups (`round3/B3.md`): SEC-ADM-10's fix holds (`call#OwnerService` calls only a service a registration names, its ALLOW is not inheritable, and `is_permitted` takes the artifact's own ALLOW before an inherited one); no recovery kind, action, service or handler name comes from a request; `store#Job` has no parameter for the service, the run-as user, the topic or the dates and writes only registered controls and parameters; a queued call is never listed or read; a message's payload is answered only with the descriptor's `dataPermission` and the owner's word; an uncertain message or mail is never sent again before a person settles it, and sign-in mail is never resent; an import is bounded text and a secret-named column is refused whole; an export file's name and content type come from a fixed table; `BackupServices` take no path, mode or destination, and `run#Backup` is held to one run at a time under a locked ledger row.

## What is left

1. **A second check of the reported findings**, High and Medium first: someone reads the cited lines and looks for
   the guard the reviewer may have missed, then a fix or a refutation. Put off by the owner's decision of 2026-11-15
   until it is needed.
2. **What each slice did not reach.** The coverage table names it, each report's "Read" section has the detail, and
   each map file ends with its open questions. The thinnest: the BOSS business apps other than Commerce and order
   approval, parsers of untrusted input, the mobile app's native code, and the helper-read parts of the PASS services.
3. **Fixes.** The High findings are fixed (2026-11-15, what they left on 2026-11-16), and the Medium ones (2026-11-16).
   Of the Medium findings none is open, none deferred, none refuted and none fixed only in part. Of the 169 Low and 46
   Info findings, 158 are fixed (152 Low, 6 Info) and 5 Low deferred; none is refuted whole (SEC-GIT-19 and SEC-PSS-28
   are in part); 12 Low and 40 Info are as reported. The first batch of the owner's decisions (2026-11-16) finished 14
   of them: SEC-WEB-05, SEC-WEB-06, SEC-WEB-10, SEC-RTE-17, SEC-ADM-10, SEC-GIT-18, SEC-GIT-19, SEC-COM-20, SEC-DAT-11,
   SEC-PSS-26, SEC-PSS-27, SEC-PSS-28, SEC-PSS-44 and SEC-OUT-13. The second (2026-11-17) finished 7: SEC-MAIL-06,
   SEC-MAIL-07, SEC-CLU-02, SEC-CLU-04, SEC-PSF-04, SEC-PSS-05 and SEC-PSS-23. A third (2026-11-17) finished the five
   fixed in part, SEC-TLS-08, SEC-TLS-17, SEC-COM-21, SEC-DAT-09 and SEC-BAP-06, and the follow-ups of SEC-DAT-11,
   SEC-MAIL-07 and SEC-CLU-02 below. A fourth (2026-11-17), the open ones of most impact: SEC-PSS-66, SEC-SCR-08
   (confirmed: mr-marble's TaskSummary), SEC-ADM-11, SEC-FIL-65 and SEC-TLS-18 (the session id, then the query values).
   A fifth (2026-11-17): SEC-BOS-08, SEC-GIT-13, SEC-FIL-05, SEC-SCR-10, SEC-MAIL-05 and SEC-OUT-14.
   A sixth (2026-11-17): SEC-CLU-06, SEC-PFS-64, SEC-BAP-05, SEC-SCR-09, SEC-RTE-16, SEC-FIL-06 and SEC-PFS-02, and
   four found already fixed by an earlier fix when checked: SEC-DAT-07 (SEC-OUT-06), SEC-RTE-10 (SEC-WEB-11),
   SEC-DEV-03 (SEC-WEB-04) and SEC-MAIL-67 (SEC-MAIL-01). A seventh (2026-11-17): SEC-TLS-14, SEC-TLS-15, SEC-DEV-02,
   SEC-RTE-15, SEC-FIL-63, SEC-PSS-62, SEC-GIT-12, SEC-PSS-45 and SEC-CLI-02.

   Deferred, each for an owner's decision or a design: SEC-COM-19 and SEC-COM-26 (each a design of its own), and
   SEC-CLI-03, SEC-CLI-04 and SEC-CLI-05 (the desktop and Tauri clients, waiting for the desktop build machine).

   Found while fixing and not fixed: each `RCPT` and delivery sums a mail account's message sizes, with no kept counter
   (SEC-MAIL-07; the owner's call, later work if it is slow).

   Found while fixing and fixed 2026-11-17: on a view with two `Party` members each member is held to the row filter on
   its own (SEC-DAT-11); an edge node answers `452` for an account its local node marked full, the account's users are
   told, and a mail administrator sets `quotaBytes` (SEC-MAIL-07); the content files an edge pairing receives are
   capped (`queue-max-content-files`), and a local node past its own cap tells its administrators (SEC-CLU-02);
   mr-marble's label download streamed a stored URL with no check (SEC-BAP-06); and an import from a caller's http(s)
   source was checked for public addresses and then fetched by a lookup of its own, so a name could change its address
   between them (the Git map, Import and fetch), now fetched under `moqui_git::deny_internal` unless
   `webhook-private-network`.

   Found while fixing and fixed the same day: an account-owned email server written or polled through `PATCH email/servers/{id}`, `/rest/e1` and `servers/{id}/poll` (`update#RelayServer`,
   `sensitive::account_owned_email_server`); IronStone's people search and mention ids outside the chat-target rule
   (`ChatterServices` `search#Users`, `get#UserIdsParties`); an SSH upload-pack's limits held to the whole conversation
   instead of each round; the TURN server's map of pending ALLOCATEs grown by forged requests; and a dependency's TURN
   nonce handling, held at moqui's fork (docs/Build-Test.md, Forks) and tracked privately until disclosed upstream. The other Low and Info findings are
   not fixed. Later work beside them: DANE and TLS-RPT for MX delivery (SEC-MAIL-66 is fixed by `mail_mx_require_tls`
   for the domains it names; the default stays opportunistic).

## Counts

| Area | Critical | High | Medium | Low | Info | Refuted | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Web authentication and controls (`WEB`) | 0 | 1 | 4 | 9 | 0 | 0 | 14 |
| Routes, screens and REST (`RTE`) | 0 | 1 | 6 | 10 | 1 | 0 | 18 |
| Administration, identity and sign-in (`ADM`) | 0 | 1 | 5 | 6 | 1 | 0 | 13 |
| Tools, System, Monitor and Dashboards (`TLS`) | 0 | 2 | 7 | 8 | 2 | 0 | 19 |
| LLM, A2A, MCP and Assist (`AI`) | 0 | 0 | 2 | 10 | 1 | 0 | 13 |
| Git hosting and pipelines (`GIT`) | 0 | 2 | 5 | 12 | 2 | 0 | 21 |
| Mail (`MAIL`) | 0 | 3 | 5 | 8 | 2 | 0 | 18 |
| Files, DAV, SSH and the terminal (`FIL`) | 0 | 0 | 6 | 6 | 6 | 0 | 18 |
| Calls, chat and messaging (`COM`) | 0 | 0 | 7 | 17 | 6 | 0 | 30 |
| Cluster and edge node (`CLU`) | 0 | 0 | 0 | 6 | 1 | 0 | 7 |
| Data layer and stored secrets (`DAT`) | 0 | 0 | 4 | 7 | 2 | 0 | 13 |
| PASS frontend (mr-pass) (`PSF`) | 0 | 0 | 1 | 5 | 3 | 0 | 9 |
| PASS authority and public accounts (`PSA`) | 0 | 0 | 1 | 10 | 2 | 0 | 13 |
| PASS application services (Mantle) (`PSS`) | 0 | 0 | 5 | 20 | 4 | 0 | 29 |
| Forum and Smithy (`PFS`) | 0 | 0 | 1 | 5 | 2 | 0 | 8 |
| BOSS access model; Admin, Commons, People, Accounting (`BOS`) | 0 | 0 | 3 | 5 | 1 | 0 | 9 |
| BOSS business apps and the Mantle REST root (`BAP`) | 0 | 0 | 2 | 5 | 2 | 0 | 9 |
| Clients (`CLI`) | 0 | 0 | 1 | 4 | 4 | 0 | 9 |
| Script engine, templates and parsers (`SCR`) | 0 | 2 | 4 | 5 | 1 | 0 | 12 |
| Outbound requests, processes and packaging (`OUT`) | 0 | 0 | 5 | 8 | 1 | 0 | 14 |
| Development tools (`DEV`) | 0 | 0 | 3 | 3 | 2 | 0 | 8 |
| **All** | 0 | 12 | 77 | 169 | 46 | 0 | 304 |

## Findings by area

### Web authentication and controls

Map file: [Attack-Surface-Web.md](../docs/security/Attack-Surface-Web.md). Reviewer report: `security-review-evidence/slice-1.md` (and `slice-1b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-WEB-01 | `crates/moqui-core/src/user/login.rs:1303-1310`, `:1511-1520`, `:1736-1746` | An account disabled for good comes back by itself: four failed logins and a wait, or a password change | High | fixed |
| SEC-WEB-02 | `runtime/components/base/framework/service/org/moqui/impl/UserServices.xml:166-283` | The password change form checks `oldPassword` with no lockout | Medium | fixed |
| SEC-WEB-03 | `crates/moqui-core/src/user/login.rs:1303-1310`, `:449-470`, `:1736-1746` | A locked account stays locked while anything keeps trying, its own API key included | Medium | fixed |
| SEC-WEB-04 | `crates/moqui-core/src/screen/render.rs:2667-2672`, `:2070-2098` | Transitions with the default method run their actions on a GET with no token | Medium | fixed |
| SEC-WEB-08 | `Login.xml:116-120` | The first caller creates the administrator | Medium | fixed |
| SEC-WEB-05 | `crates/moqui-core/src/user/login.rs:54-61`, `:403-416`, `:514-516` | Several sign-ins keep the session id; the forced password change flow never changes it | Low | fixed |
| SEC-WEB-06 | `crates/moqui-core/src/user/login.rs:1042-1071`, `:1227-1245` | Logout keeps the session and its attributes; a password change ends nothing | Low | fixed |
| SEC-WEB-07 | `crates/moqui-host/src/guard.rs:295-300`, `:1010-1035` | The `login` rate limit covers part of the password checks and is passed by a dot segment | Low | fixed |
| SEC-WEB-09 | `UserServices.xml:404-462` | `reset#Password` rewrites `requirePasswordChange` for any account | Low | fixed |
| SEC-WEB-10 | `runtime/conf/MoquiDevConf.xml:9` | Dev conf: every origin is allowed with credentials | Low | fixed |
| SEC-WEB-11 | `crates/moqui-host/src/http.rs:957-963`, `:1679-1684` | `/health` and `/metrics` trust a loopback TCP peer | Low | fixed |
| SEC-WEB-12 | `crates/moqui-core/src/user/totp.rs:89-106` | Second factor details: a TOTP code works more than once; code sends are not limited in the login flow | Low | fixed |
| SEC-WEB-13 | `crates/moqui-core/src/user/session.rs:3057-3065` | A `session_cookie_key` set by the operator is not checked | Low | fixed |
| SEC-WEB-14 | `crates/moqui-core/src/user/authz.rs:703-704`, `:753-758` | An anonymous tarpit lock is everyone's | Low | fixed |
| SEC-WEB-81 | `crates/moqui-host/src/guard.rs:338-344`, `:1015-1019`, `:1034-1035`; `crates/moqui-core/src/screen/url.rs:272-275`, `:301-307`, `:548`, `:567-585`; `crates/moqui-core/src/screen/mod.rs:585-598`, `:739` | A path-scoped rate limit or path rule is passed by another path to the same transition or screen (SEC-WEB-07's fix does not cover these) | Low | reported (confirmed in code) |
| SEC-WEB-82 | `crates/moqui-host/src/guard.rs:162-173`; `crates/moqui-core/src/web/mod.rs:2532-2537` | A per-host web protection rule is passed by a `Host` header with a port that is not a number | Low | reported (likely) |
| SEC-WEB-83 | `crates/moqui-host/src/guard.rs:863-872`, `:935-944`, `:1054-1056`, `:1336-1345` | Once 100,000 clients are tracked, a new client is not rate limited, banned or connection-capped | Low | reported (confirmed in code) |
| SEC-WEB-84 | `runtime/components/base/webroot/screen/webroot/sso.xml:19-27`; `crates/moqui-core/src/service/sso.rs:309-318`, `:522-530` | With an OAuth 2 provider the recent sign-in check is met by a cross-site `GET /sso/login?...&reauth=true` | Low | reported (confirmed in code) |
| SEC-WEB-85 | `crates/moqui-core/src/user/login.rs:158-163`, `:307-336`, `:359-364`, `:486-498`; `Security.eecas.xml:154-155` | The new-device notice is not sent for a sign-in that passes a second factor, or one made within a minute of another login attempt on the account | Low | reported (confirmed in code) |
| SEC-WEB-86 | `crates/moqui-host/src/sso.rs:557-565` | User info claims are merged into the profile without comparing the user info `sub` with the ID token's | Low | reported (confirmed in code) |

### Routes, screens and REST

Map file: [Attack-Surface-Routes.md](../docs/security/Attack-Surface-Routes.md). Reviewer report: `security-review-evidence/slice-2.md` (and `slice-2b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-RTE-01 | `crates/moqui-core/src/web/mod.rs:1306` (`mark_email_message_viewed`) | `GET /email/{id}` lets anyone move mail to Viewed | High | fixed |
| SEC-RTE-02 | `crates/moqui-core/src/screen/statics.rs:767` (`screen_package`), `:545` (`unwrap_context`) | `GET <screen>/actions` answers the screen's whole context | Medium | fixed |
| SEC-RTE-03 | `crates/moqui-entity/src/entity/query_rest.rs:214` | Any signed-in user can write `EntityAuditLog` rows through entity REST | Medium | fixed |
| SEC-RTE-04 | `runtime/components/base/webroot/template/screen/BasicTransitions.xml:20` | `emailScreenAsync` mails a full report to a request-given address by `GET` | Medium | fixed |
| SEC-RTE-05 | `runtime/components/base/webroot/screen/webroot.xml:257` | `/resources` runs elFinder write commands on a `GET` | Medium | fixed |
| SEC-RTE-06 | `crates/moqui-entity/src/entity/query_rest.rs:208`, `:962` (`related_rows`) | Entity REST dependents read entities the restricted rule refuses | Medium | fixed |
| SEC-RTE-07 | `crates/moqui-host/src/http.rs:1109`-`1124`, `:1203` | A multipart body is spooled to disk before any check | Medium | fixed |
| SEC-RTE-08 | `crates/moqui-core/src/screen/render.rs:2667` (`csrf_rejected`), `:2070` (`insecure_transition`) | State changes on `GET` with no token | Low | fixed |
| SEC-RTE-09 | `crates/moqui-core/src/screen/url.rs:373` | `lastStandalone` takes the parents out of the user check | Low | fixed |
| SEC-RTE-10 | `crates/moqui-host/src/http.rs:957`, `:977`, `:1679` | `/health` and `/metrics` trust the TCP peer | Low | fixed |
| SEC-RTE-11 | `crates/moqui-core/src/screen/public_site.rs:96`-`102` | A restricted host still serves some framework paths | Low | reported (likely) |
| SEC-RTE-12 | `runtime/components/base/webroot/screen/webroot.xml:83`, `:104` | Anonymous reads through `/listViewColumns` and `/statusBar` | Low | fixed |
| SEC-RTE-13 | `crates/moqui-core/src/screen/url.rs:499` | Answers that tell an anonymous caller what exists | Low | fixed |
| SEC-RTE-14 | `crates/moqui-core/src/service/rest.rs:731` | Service REST entity lists have no upper bound | Low | fixed |
| SEC-RTE-15 | `crates/moqui-core/src/script/host/user.rs:120` | `getPreferences` builds a JavaScript regular expression from the request | Low | fixed |
| SEC-RTE-16 | `crates/moqui-core/src/screen/token_route.rs:300`-`365` | A refused token-route token is not counted | Low | fixed |
| SEC-RTE-17 | `runtime/components/base/webroot/screen/webroot/Login.xml:191`-`193`, `:249`-`256` | The Login page verifies a password on every render and offers a one-click admin sign-in | Low | fixed |
| SEC-RTE-18 | `runtime/components/base/webroot/screen/webroot/PublicShell.xml:24` | `/PublicShell` takes `tokenRoute` from a JSON body | Info | reported (to check) |
| SEC-RTE-81 | `crates/moqui-core/src/screen/definition.rs:396-406`; `runtime/components/base/framework/template/XmlActions.js:306-307`; `crates/moqui-core/src/service/call.rs:255-280`; mr-marble `screen/MyAccount/User/Account.xml:19-25` | A posted `_isMulti=true` with `<name>_0` replaces an id a screen's always-actions pinned; on mr-marble's My Account one user changes another account's email address | High | likely (lead) |
| SEC-RTE-82 | `crates/moqui-core/src/web/multipart.rs:650-665`, `:765-774`, `:536-545` | A multipart body can hold the whole upload limit in memory, outside the spool total (SEC-RTE-07's fix counts spool files only) | Medium | likely (lead) |
| SEC-RTE-83 | `crates/moqui-core/src/service/tools.rs:158-180`; `runtime/components/base/tools/service/moqui.rest.xml:137-141` | `print#ResourceDocument` sends any file the server can read to a printer the caller defines | Medium | likely (lead) |
| SEC-RTE-84 | `crates/moqui-core/src/web/mod.rs:1975-1976`; `crates/moqui-host/src/http.rs:1130`, `:1432` | An anonymous request to `/search/*` or `/dashboard/*` may send a 100 MiB body that is held in memory before any check | Low | reported (confirmed in code) |
| SEC-RTE-85 | `crates/moqui-core/src/user/login.rs:1235-1240`; `crates/moqui-core/src/service/user_keys.rs:762-778` | A git access token mints a key for the LLM gateway at `POST /rest/s1/moqui/hosting/gatewayKey` (SEC-PFS-61's scope does not stop it) | Low | reported (confirmed in code) |
| SEC-RTE-86 | `crates/moqui-core/src/service/system_sync/message.rs:257-264`, `:397-408` | `/rest/sm/{type}/{remote}` takes any message type on any remote's check | Low | reported (confirmed in code) |
| SEC-RTE-87 | `crates/moqui-core/src/service/tools.rs:518-562`; `moqui.rest.xml:175-176` | `get#ServerPrinters` connects to any host and port with the internal-address guard off, on a `GET` | Low | reported (confirmed in code) |
| SEC-RTE-88 | `crates/moqui-core/src/screen/url.rs:567-577`; `crates/moqui-core/src/screen/cache.rs:308-311`, `:607-618` | The `//` screen path search keeps every miss in a map with no limit | Low | reported (to check) |

### Administration, identity and sign-in

Map file: [Attack-Surface-Admin.md](../docs/security/Attack-Surface-Admin.md). Reviewer report: `security-review-evidence/slice-3.md` (and `slice-3b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-ADM-01 | `UserServices.xml:166-282` (`update#Password`), `:284-385` (`update#PasswordInternal`) | A disabled account re-enables itself with a password change | High | fixed |
| SEC-ADM-02 | `UserServices.xml:233-271` | `update#Password` tests passwords with no lockout and tells right from wrong | Medium | fixed |
| SEC-ADM-03 | `UserServices.xml:210-229` with `:704-738` (`create#SingleUseAuthcCodes`) | A signed-in session changes the password without the old one | Medium | fixed |
| SEC-ADM-04 | `UserServices.xml:532, 637, 704, 741, 815, 932, 963, 990, 1008, 1052` | Factor services act on any `userId` | Medium | fixed |
| SEC-ADM-05 | `UserServices.xml:183-202` | `ADMIN` reaches the sealed permissions of `ADMIN_ADV` through another account | Medium | fixed |
| SEC-ADM-06 | `admin/Access.js:1093-1120` (`applyGrantProposal`) | `ADMIN_IDENTITY_MANAGE` assigns bundles though it is not sealed | Medium | fixed |
| SEC-ADM-07 | `crates/moqui-core/src/service/user_keys.rs:216-229, 382-401, 137-176` | Long-lived credentials are minted from a session with no recent sign-in | Low | fixed |
| SEC-ADM-08 | `base/tools/screen/System/Security/UserAccount/UserAccountDetail.xml:31-43` | Login-as leaves no evidence and has no bound | Low | fixed |
| SEC-ADM-09 | `base/webroot/screen/webroot/sso.xml:40-45` | `/sso/logout` signs the browser out on a GET | Low | fixed |
| SEC-ADM-10 | `admin/OperationLib.js:59-72` | Owner services run below `OperationServices` with inherited authorization | Low | fixed |
| SEC-ADM-11 | `crates/moqui-core/src/service/sso.rs:601-650, 762-777` | The SSO sync writes an unverified `email` claim to `UserAccount.emailAddress` | Low | fixed |
| SEC-ADM-12 | `UserServices.xml:404-462` | An anonymous reset can force a password change | Low | fixed |
| SEC-ADM-13 | `UserServices.xml:464-505` | `create#InitialAdminAccount` on a fresh instance | Info | fixed |
| SEC-ADM-81 | `tools/service/moqui.rest.xml:241`; `crates/moqui-core/src/service/rest.rs:676-693` | An `ADMIN` member changes any account's email address through `PATCH /rest/s1/moqui/users/{userId}`, past the guard of `update#UserAccount` (SEC-ADM-05's fix does not hold on this path) | Medium | confirmed (lead) |
| SEC-ADM-82 | `admin/ConfigurationConnections.js:836-936`, `:363-403`, `:571-581`; `UserServices.xml:453-523` | A holder of `ADMIN_CONNECTION_MANAGE` repoints the `SYSTEM` mail relay and receives any account's reset password | Medium | likely (lead) |
| SEC-ADM-83 | `admin/ConfigurationConnections.js:883-889`; `crates/moqui-mail/src/implementation.rs:101-103`, `:602-603` | A relay's link base URL is changed with no test and points the links of system mail at another host | Medium | likely (lead) |
| SEC-ADM-84 | `admin/ConfigurationProviders.js:481-484`, `:588-611` | A sign-in provider an instance administrator allowed on the private network keeps that when a delegated administrator repoints it | Low | reported (confirmed in code) |
| SEC-ADM-85 | `admin/ConfigurationProviders.js:405-420`, `:529-544`; `crates/moqui-core/src/service/sso.rs:723-730` | A field map writes any claim to the account's email address when the provider verified another one (SEC-ADM-11's fix does not ask which claim was mapped) | Low | reported (likely) |
| SEC-ADM-86 | `admin/ConfigurationLocalization.js:389-432`; `crates/moqui-core/src/service/validate.rs:629-647` | A translation with markup is saved through the import, though the single save refuses it | Low | reported (likely) |
| SEC-ADM-87 | `admin/ConfigurationProviders.js:611-628`; `admin/ConfigurationConnections.js:288` | No configuration change asks for a recent sign-in | Low | reported (confirmed in code) |
| SEC-ADM-88 | `admin/ConfigurationConnections.js:1388-1389`, `:292-300` | A connection key that names no record starts a new connection of a kind marked not creatable | Low | reported (to check) |

### Tools, System, Monitor and Dashboards

Map file: [Attack-Surface-Tools.md](../docs/security/Attack-Surface-Tools.md). Reviewer report: `security-review-evidence/slice-4.md` (and `slice-4b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-TLS-01 | `runtime/components/base/tools/screen/Tools/Data/Export.xml:18` and `:54-61` | Export's `filterMap` runs a caller's script, by GET, with no script permission | High | fixed |
| SEC-TLS-02 | `runtime/components/base/framework/service/org/moqui/dev/InstanceServices.xml:444-447`, `:195`, `:206`, `:295` | Hosted-instance rows give host command execution and script execution to a Tools and System holder | High | fixed |
| SEC-TLS-03 | `crates/moqui-core/src/script/host/tools.rs:721`, `:1272`, `:2602-2621` | `runTokenCommand` is on `ec.resource` for every script | Medium | fixed |
| SEC-TLS-04 | `crates/moqui-core/src/screen/definition.rs:387`, `render.rs:2076-2099`, `:2668-2673` | State-changing transitions that run on a cross-site GET | Medium | fixed |
| SEC-TLS-05 | `runtime/components/base/tools/screen/Tools/Data/Export.xml:26-45` | Export and Snapshots read the entities the other generic tools refuse | Medium | fixed |
| SEC-TLS-06 | `crates/moqui-report/src/schedule.rs:75-76`, `:1370-1392` | A scheduled report writes to any server path | Medium | fixed |
| SEC-TLS-11 | `crates/moqui-report/src/recovery/mod.rs:485-492`, `crates/moqui-report/src/recovery/execute.rs:526` and `:176-199`, `crates/moqui-entity/src/entity/sensitive.rs:66-76` | (telemetry reader) Monitor's permissions are checked only in its services; a generic write to a `moqui.monitor` row goes around them | Medium | fixed |
| SEC-TLS-12 | `crates/moqui-report/src/recovery/execute.rs:44-52`, `:446-451` | (telemetry reader) A recovery webhook adapter sends any process property to a URL its author chooses | Medium | fixed |
| SEC-TLS-13 | `crates/moqui-core/src/telemetry/scrape.rs:548-558`, `:705-727` | (telemetry reader) Synthetic checks and scrape targets reach internal addresses and answer what they found | Medium | fixed |
| SEC-TLS-07 | `runtime/components/base/framework/service/org/moqui/impl/EntityServices.xml:276-284` | `export#EntityDataSnapshot` joins `baseFilename` into a path | Low | fixed |
| SEC-TLS-08 | `crates/moqui-devkit/src/deploy/git.rs:210-234` | The production internal-address check of a deploy URL is bypassed by user info, and fails open | Low | fixed |
| SEC-TLS-09 | `runtime/components/base/tools/screen/Tools/Service/ServiceRun.xml:26-29` | Secrets written to the server log by Tools and Instance | Low | fixed |
| SEC-TLS-10 | `runtime/components/base/framework/service/org/moqui/dev/InstanceServices.xml:816`, `:842` | `create#DatabaseMySQL` quotes the password for standard SQL only | Low | fixed |
| SEC-TLS-14 | `crates/moqui-core/src/telemetry/ingest.rs:878-891`, `crates/moqui-report/src/regression/mod.rs:144-157`, `:276-281`, `crates/moqui-report/src/annotations.rs:60-75` | (telemetry reader) An ingest key's sender can pose as this server's own service and move the regression detector and recovery | Low | fixed |
| SEC-TLS-15 | `crates/moqui-report/src/annotations.rs:19-31`, `crates/moqui-host/src/monitor/receiver.rs:224-243` | (telemetry reader) An ingest key owned by an administrator writes business annotations for any organization | Low | fixed |
| SEC-TLS-16 | `runtime/components/base/tools/screen/System/Monitor/Settings.xml:17-20` | (telemetry reader) `Settings/revertAll` runs on a GET with no token | Low | fixed |
| SEC-TLS-17 | `crates/moqui-host/src/monitor/receiver.rs:53-78`, `crates/moqui-host/src/monitor/grpc.rs:95-109` | (telemetry reader) The ingest routes do not count failed keys, and gRPC decodes before it authenticates | Low | fixed |
| SEC-TLS-18 | `crates/moqui-host/src/http.rs:1398`, `:1456` | (telemetry reader) Request spans keep the raw session id and unredacted query values for every `SYSTEM_APP` reader | Info | fixed |
| SEC-TLS-19 | `crates/moqui-report/src/recovery/execute.rs:598-612` | (telemetry reader) The recovery callback compares token hashes with `!=` and tells an unknown action from a wrong token | Info | reported (confirmed in code) |

### LLM, A2A, MCP and Assist

Map file: [Attack-Surface-AI.md](../docs/security/Attack-Surface-AI.md). Reviewer report: `security-review-evidence/slice-5.md` (and `slice-5b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-AI-01 | `crates/moqui-llm/src/approval/mod.rs:1129-1135` | A room member approves a write that runs as another user | Medium | fixed |
| SEC-AI-02 | `crates/moqui-llm/src/assist/canvas.rs:316-371` | An OpenUI Button runs a model-written write on one click, showing only the transition's name | Medium | fixed |
| SEC-AI-03 | `js/WebrootAssist.js:300-305`, `:326-329`, `:335` | Assistant Markdown keeps same-site image paths, which the browser fetches with the session | Low | fixed |
| SEC-AI-04 | `js/WebrootAssist.js:371-376`, `:351-353` | An external link in an answer carries whatever the model put in its URL | Low | fixed |
| SEC-AI-05 | `crates/moqui-host/src/llm/sandbox.rs:43-45` | With "other sites" on, a Vue SFC canvas can send data out in an image URL | Low | reported (likely) |
| SEC-AI-06 | `crates/moqui-host/src/llm/sse.rs:47`, `:55-63`, `:169` | One user can hold every Assist event stream | Low | fixed |
| SEC-AI-07 | `runtime/components/base/webroot/screen/webroot/find.xml:9-20` | `/find/rank` sends caller text to the embedding model with no quota or external-user check | Low | fixed |
| SEC-AI-08 | `System/Llm/Calls/CallList.xml:27-30`, `Conversations/ConversationList.xml:26-28`, `Runs/RunList.xml:25-28`, `Approvals/ApprovalList.xml:25-28` | `search-form-inputs` on the audit lists lets an `AgentAdmin` test content without `AgentAudit` | Low | fixed |
| SEC-AI-09 | `crates/moqui-llm/src/services.rs:761-762`, `:802-810`, `:1285-1345` | Three agent services check nothing in their bodies | Low | fixed |
| SEC-AI-10 | `crates/moqui-llm/src/mcp/server.rs:596-615` | The MCP public documents are cacheable without `Vary: Host` though their URLs come from `Host` | Low | fixed |
| SEC-AI-11 | `crates/moqui-llm/src/a2a/mod.rs:106-108` | Loopback counts as a secure transport for credentials | Low | fixed |
| SEC-AI-12 | `crates/moqui-core/src/service/external_agent.rs:46-51` | The external agent adapter inherits the server's environment | Low | fixed |
| SEC-AI-13 | `crates/moqui-llm/src/tools/catalog.rs:157-160` | Exposing a service that needs no authentication gives it to every agent user | Info | reported (confirmed in code) |
| SEC-AI-81 | `crates/moqui-llm/src/ui/vue_sfc.rs:1295-1301`, `:560`; `js/WebrootAssistSandbox.js:24-25`, `:219-225`; `js/WebrootAssistCanvas.js:1160-1164`, `:1318-1320` | A Vue SFC canvas can send what it read to another site by navigating its own frame | Medium | likely (lead) |
| SEC-AI-82 | `crates/moqui-llm/src/ui/screen_xml.rs:48-66`, `:102-153`, `:169-208`; `js/WebrootAssistCanvas.js:1361-1373`, `:1389-1390` | On a dev instance a `screen-xml` canvas runs model-written script in the application's origin | Medium | confirmed (lead) |
| SEC-AI-83 | `System/Llm/Evals/Runs.xml:16-44`; `AgentEval.js:26-31`, `:61-64`, `:34-40` | Evals › Runs shows every user's prompts and replies to an `AgentAdmin` without `AgentAudit`, and logs nothing | Medium | confirmed (lead) |
| SEC-AI-84 | `crates/moqui-llm/src/a2a/ops.rs:639-661`; `crates/moqui-llm/src/run/mod.rs:525-530` | An A2A context made earlier still starts tasks of an agent whose `exposeA2a` was turned off | Low | reported (confirmed in code) |
| SEC-AI-85 | `crates/moqui-llm/src/a2a/types.rs:300-349`; `crates/moqui-llm/src/a2a/ops.rs:732-795`, `:381-390` | An A2A message is up to 2 MiB of text, and messages to a working task are not counted | Low | reported (likely) |
| SEC-AI-86 | `crates/moqui-llm/src/a2a/mod.rs:206-258`; `crates/moqui-llm/src/mcp/server.rs:422-455`; `crates/moqui-llm/src/gateway.rs:1610-1743` | An `LlmApiClient` limits nothing on the A2A and MCP servers | Low | reported (confirmed in code) |
| SEC-AI-87 | `crates/moqui-llm/src/mcp/tools.rs:104-110`; `crates/moqui-llm/src/tools/mod.rs:587-592` | When the MCP agent's tool policy cannot be read, the server offers every tool but the destructive ones | Low | reported (confirmed in code) |
| SEC-AI-88 | `crates/moqui-host/src/llm/gateway.rs:43`, `:163`, `:192`; `crates/moqui-host/src/llm/a2a.rs:187`; `crates/moqui-host/src/llm/mcp.rs:67` | The gateway buffers 16 MiB of a request before it looks at credentials | Low | reported (likely) |
| SEC-AI-89 | `crates/moqui-llm/src/run/mod.rs:498-670`, `:738-924`; `crates/moqui-llm/src/run/pool.rs:36-52`, `:70-84` | One user can fill the node's agent pool, so other users' runs wait | Low | reported (likely) |
| SEC-AI-90 | `crates/moqui-llm/src/run/step.rs:852-868`, `:1013-1017`; `crates/moqui-llm/src/run/mod.rs:1170-1219` | A run keeps working after its user loses `AgentUse` or its agent is disabled | Low | reported (confirmed in code) |
| SEC-AI-91 | `crates/moqui-llm/src/run/step.rs:749-767`; `crates/moqui-llm/src/run/mod.rs:1510-1513`, `:1531-1540`; `crates/moqui-llm/src/chat.rs:1050-1060` | The text of a step's internal error is shown to the user and posted in the chat room | Low | reported (likely) |
| SEC-AI-92 | `crates/moqui-llm/src/run/step.rs:1695-1718`; `js/WebrootAssist.js:748-751`, `:488-534`, `:928`, `:300-303` | The server's check of invented links covers only a run's last text, and its marks are lost on reload | Low | reported (likely) |
| SEC-AI-93 | `crates/moqui-llm/src/memory.rs:147-184`; `crates/moqui-llm/src/tools/remember.rs:122-124`; `crates/moqui-llm/src/skills/learn.rs:741` | `remember` and `save_skill` ask for a confirm only when tool output follows the last user message | Low | reported (likely) |
| SEC-AI-94 | `crates/moqui-llm/src/tools/data.rs:664-671`, `:684-686`, `:742-761` | `data` finds and sorts by an `ai-opt-out` field when the find is written in the path's query | Low | reported (likely) |
| SEC-AI-95 | `AgentEval.js:7`, `:44`, `:101-104`; `System/Llm/Evals/Experiments.xml:5-8`, `:53`; `Scorers.xml:4-11` | An eval experiment or scorer calls any service by name with authorization off | Low | reported (confirmed in code) |
| SEC-AI-96 | `AgentEval.js:30-31`, `:72-83`; `crates/moqui-llm/src/ai_task.rs:476-499`; `crates/moqui-llm/src/approval/mod.rs:457-461` | A prompt saved from another user's run is later run as the administrator who starts the experiment | Low | reported (confirmed in code) |
| SEC-AI-97 | `runtime/components/dev/dev-aide/src/agent/hosting.rs:177-190`; `src/hosting/client.rs:306-318`, `:325-332` | The `hosting` tool gives the model a remote server's error text outside the untrusted wrapper | Low | reported (likely) |
| SEC-AI-98 | `crates/moqui-llm/src/skills/services.rs:398-447`, `:296-318` | An `AgentAdmin` reads any user's personal skill | Low | reported (confirmed in code) |
| SEC-AI-99 | `crates/moqui-llm/src/mcp/tools.rs:433-436`, `:482`; `crates/moqui-core/src/user/authz.rs:730` | The MCP tarpit counts by tool name, and the name is the caller's | Info | reported (confirmed in code) |
| SEC-AI-100 | `crates/moqui-llm/src/gateway.rs:160-204`, `:512-543` | The gateway passes URLs in a prompt to the model server, which may fetch them from where it runs | Info | reported (to check) |
| SEC-AI-101 | `crates/moqui-llm/src/skills/learn.rs:745-759`; `crates/moqui-tui/src/assist/draw.rs:264-282` | The `save_skill` confirm card shows the skill's name and description, not its text | Info | reported (to check) |

### Git hosting and pipelines

Map file: [Attack-Surface-Git.md](../docs/security/Attack-Surface-Git.md). Reviewer report: `security-review-evidence/slice-6.md` (and `slice-6b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-GIT-01 | `crates/moqui-git/src/lib.rs:1470-1530` | `import#GitRepository` reads another repository or a server directory with no check of the source | High | fixed |
| SEC-GIT-02 | `crates/moqui-host/src/ci_run.rs:134`, `:151` | A registered runner gives its runner token to every step | High | fixed |
| SEC-GIT-03 | `crates/moqui-git/src/server/mod.rs:771-777` | A gzip request body is inflated without limit, and SSH has no request limit | Medium | fixed |
| SEC-GIT-04 | `crates/moqui-git/src/service/mod.rs:605` | A viewer reads a whole blob or LFS object into server memory through REST | Medium | fixed |
| SEC-GIT-05 | `crates/moqui-git/src/server/pack_in.rs:158-171`, `:249`, `:327-344` | A small pack of large or deeply chained objects costs far more than its bytes | Medium | fixed |
| SEC-GIT-06 | `crates/moqui-git/src/mirror.rs:396-411` | An SSH push mirror's server is read without size or total-time limits | Medium | fixed |
| SEC-GIT-07 | `crates/moqui-git/src/service/mod.rs:168-181` (`fetch#GitRepository`), `:185-198` (`push#GitMirror`), `:230-250` (`init#GitRepository`), `:296-316` (`export#GitRepository`) | Git services that trust their caller | Medium | fixed |
| SEC-GIT-08 | `crates/moqui-git/src/ci/log.rs:98-121`, `:217` | Secret masking misses what is not one log batch | Low | fixed |
| SEC-GIT-09 | `crates/moqui-host/src/git_http.rs:293-298` | A loopback socket peer counts as a secure transport | Low | fixed |
| SEC-GIT-10 | `crates/moqui-git/src/service/review.rs:81-97` | `fork#GitRepository` uses the new `repositoryId` as a directory name without checking it | Low | fixed |
| SEC-GIT-11 | `crates/moqui-git/src/service/hosting.rs:631-665` | `delete#GitRef` deletes any ref, a merge request's head included | Low | fixed |
| SEC-GIT-12 | `crates/moqui-git/src/browse.rs:140-150`, `:102-106` | A revision that is a full ref name reads any ref of the store but a hidden merge request's head | Low | fixed |
| SEC-GIT-13 | `GitReviewServices.xml:508`, `:520-535` | Approvals stay when a merge request's target changes | Low | fixed |
| SEC-GIT-14 | `crates/moqui-core/src/user/authz.rs:502-504` | The services' repository checks pass when `AT_GIT_REPOSITORY` authorization is off | Low | fixed |
| SEC-GIT-15 | `crates/moqui-git/src/storage.rs:160-171` | `delete#GitRepository` leaves the repository's push mirror rows | Low | fixed |
| SEC-GIT-16 | `GitServices.xml:1123` | A webhook URL may be plain http and carry a user and password; `remoteUrl` is shown to every viewer | Low | fixed |
| SEC-GIT-17 | `crates/moqui-git/src/ci/driver/docker.rs:732`, `:769-792`, `:273-279` | The run services put their parameters into the Docker API path and remove a container by any id | Low | fixed |
| SEC-GIT-18 | `crates/moqui-host/src/git_http.rs:251`, `:345`, `:432` | Unauthenticated requests take a git worker, and one call can walk every repository | Low | fixed |
| SEC-GIT-19 | `crates/moqui-git/src/ci/token.rs:150-160` | Runner tokens never expire, and a pool of registered runners may take untrusted runs | Low | fixed |
| SEC-GIT-20 | `crates/moqui-git/src/server/pack_in.rs:269-277` | Pushed trees are not checked for unsafe names, and `write_commit` has no limits | Info | reported (confirmed in code) |
| SEC-GIT-21 | `crates/moqui-git/src/service/rest.rs:546-565` | `get#AuthorSigned` answers for any user id or e-mail address | Info | reported (to check) |
| SEC-GIT-81 | `crates/moqui-git/src/ci/runner/mod.rs:282-322`, `:712-728` | A registered runner reads a job's report or artifact path through a symbolic link, as root with `--step-user`, and the server serves the file to the run's readers | High | confirmed (lead) |
| SEC-GIT-82 | `crates/moqui-git/src/server/plan.rs:128-165` | A fetch's `shallow` lines make the server send the ancestors of a commit the reader may not read, and say whether the store holds it | Medium | confirmed (lead) |
| SEC-GIT-83 | `crates/moqui-git/src/server/upload.rs:246-282`, `:567-576` | One v0 upload-pack request can keep a git worker walking history for hours | Medium | reported (likely) |
| SEC-GIT-84 | `crates/moqui-git/src/server/lfs.rs:406`, `:485-502`, `:701-761` | LFS uploads and locks are taken for an archived repository, and for one that is being deleted | Low | reported (confirmed in code) |
| SEC-GIT-85 | `crates/moqui-git/src/server/lfs.rs:803-831`, `:862-899` | A user with a push right creates LFS locks without limit, and each locks request reads them all | Low | reported (confirmed in code) |
| SEC-GIT-86 | `crates/moqui-git/src/lib.rs:1579-1587`; `crates/moqui-git/src/fs.rs:438-456` | An import from a caller's `source` offers the repository's stored token to that host | Low | reported (likely) |
| SEC-GIT-87 | `crates/moqui-git/src/fs.rs:427-433`, `:461` | gitoxide's text of a failed fetch is stored and shown as it is, and can name the URL with its password | Low | reported (to check) |
| SEC-GIT-88 | `crates/moqui-git/src/ci/trust.rs:59-60`; `ci/queue.rs:644-651` | A scheduled run is trusted whoever pushed the branch it runs on | Low | reported (confirmed in code) |
| SEC-GIT-89 | `crates/moqui-git/src/ci/queue.rs:127-133`; `browse.rs:319` | The pipeline file is read whole before its size is checked, on each event and for any viewer | Low | reported (likely) |
| SEC-GIT-90 | `crates/moqui-git/src/ci/artifact.rs:207-216`, `:219-247` | A job's uploads have no storage cap by default, and the count of a job's uploads is not held | Low | reported (confirmed in code) |
| SEC-GIT-91 | `crates/moqui-git/src/ci/service/secrets.rs:184-190`, `:358-369`, `:402-404` | An environment's `allowedRefs` and `approvalRequired` change without its secrets being given again | Low | reported (confirmed in code) |
| SEC-GIT-92 | `crates/moqui-git/src/gc/local.rs:116-121`, `:245`; `server/mod.rs:342`, `:1071`, `:1103` | Quarantine and scratch directories a killed process left are never removed | Low | reported (confirmed in code) |
| SEC-GIT-93 | `crates/moqui-git/src/ci/driver/docker.rs:560-563`, `:613-627` | A pool's `egressRule` refuses only `host` and `container:`, and an image's `VOLUME` is outside the disk limit | Low | reported (confirmed in code) |
| SEC-GIT-94 | `crates/moqui-git/src/events.rs:638-666` | `process#GitUpdate` runs the rest of the caller's request as the pusher if the way back fails | Low | reported (to check) |
| SEC-GIT-95 | `crates/moqui-git/src/ci/runner/archive.rs:59-76` | The CI runner unpacks a cache with no limit on expanded size or entries | Low | reported (confirmed in code) |
| SEC-GIT-96 | `crates/moqui-core/src/context/identity.rs:4-26` | The `value` of a CI secret is not a secret name to the request log's rule | Info | reported (confirmed in code) |
| SEC-GIT-97 | `crates/moqui-git/src/merge.rs:420-428` | A commit made by `commit#GitFiles` keeps a `Moqui-User-Id` trailer the caller wrote | Info | reported (confirmed in code) |

### Mail

Map file: [Attack-Surface-Mail.md](../docs/security/Attack-Surface-Mail.md). Reviewer report: `security-review-evidence/slice-7.md` (and `slice-7b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-MAIL-01 | `runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml` `create#EmailAccount` (207-306), `update#EmailAccount` (308-452) | Any mail user can take the inbound mail of an address nobody has claimed | High | fixed |
| SEC-MAIL-61 | runtime/components/base/webroot/template/screen/BasicTransitions.xml:20-22 | A request's fileName runs as a server-side expression through emailScreenAsync | High | fixed |
| SEC-MAIL-63 | runtime/components/addon/mr-marble/screen/SimpleScreens/Party/PartyEmails.xml:23-26 | mr-marble PartyEmails serves any EmailMessage body as inline text/html | High | fixed |
| SEC-MAIL-02 | `crates/moqui-mail/src/inbound.rs:1018-1031` (`connect_tcp`), `crates/moqui-mail/src/sync.rs:462-530` (`open_imap`), `crates/moqui-mail/src/implementation.rs` `smtp_transport` (1888-1934), `require_host` (979-998) | A mail account's server settings make the server connect to internal addresses | Medium | fixed |
| SEC-MAIL-03 | `crates/moqui-core/src/web/mod.rs:1289-1340` (`view_email_message`, `mark_email_message_viewed`) | An anonymous GET changes the status of any mailbox or Outbox message | Medium | fixed |
| SEC-MAIL-04 | `runtime/components/base/tools/service/moqui.rest.xml:122-131` | The tools REST resource returns mailbox passwords and OAuth refresh tokens | Medium | fixed |
| SEC-MAIL-62 | runtime/components/base/webroot/template/screen/BasicTransitions.xml:20-28 (and 29-37) | A GET mails a screen's full export to an address the request names | Medium | fixed |
| SEC-MAIL-64 | runtime/components/addon/mr-marble/screen/SimpleScreens/Party/PartyEmails.xml:20 | mr-marble sendEmailMessage transitions send any stored message again | Medium | fixed |
| SEC-MAIL-05 | `EmailClientServices.xml` `send#OutOfOfficeReplies` (5197-5412), `run#EmailRules` `forward` (4532-4571) | Out-of-office replies and rule forwards have no total limit and trust the From header | Low | fixed |
| SEC-MAIL-06 | `crates/moqui-mail/src/delivery.rs:601-725` | The "delivered" notice is backscatter with the sender's headers | Low | fixed |
| SEC-MAIL-07 | `crates/moqui-host/src/smtp.rs` session loop (1169-1206), `rcpt` (1354-1450) | Internet mode has no cap on messages a session, refused recipients, or stored bytes | Low | fixed |
| SEC-MAIL-08 | `crates/moqui-mail/src/delivery.rs` `deliver_one` (753-772) | A message is dropped when its Message-ID is already in the account | Low | fixed |
| SEC-MAIL-09 | `crates/moqui-core/src/service/mail_capture.rs` (`decide`, `enabled`) | The mail capture can be on in production | Low | fixed |
| SEC-MAIL-65 | runtime/components/base/framework/service/org/moqui/impl/UserServices.xml:441-459 | Reset passwords and verification tokens are kept in clear in EmailMessage.body | Low | fixed |
| SEC-MAIL-66 | crates/moqui-mail/src/mx.rs:977-1010 (`hosts`), 1053-1075, 1138-1160 (`session`), 1313-1326 (`sts::fetch`) | Edge MX delivery connects to whatever address a recipient domain's DNS names, and falls back to clear text | Low | fixed |
| SEC-MAIL-67 | crates/moqui-mail/src/auth.rs:405-418 (`signing_domain`), 493-512 (`sign_for`), 763-790 (`verify_internal`) | A mail user's hosted domain is one the node signs for and then verifies as its own | Low | fixed |
| SEC-MAIL-10 | `runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.qvue:1880` | Mail forged as from the reader loads remote images without asking | Info | reported (to check) |
| SEC-MAIL-68 | runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml:5002-5034 (`create#EmailIdentity`), 3357-3359 (`save#Draft`) | An identity's address is any text with an @ | Info | reported (confirmed in code) |
| SEC-MAIL-81 | `crates/moqui-mail/src/auth.rs:643-665` (`verify_with`), `crates/moqui-host/src/smtp.rs:1946-1956`, `crates/moqui-mail/src/store.rs:128-137` | A message whose From names two domains, or none, is not held to DMARC | Medium | likely (lead) |
| SEC-MAIL-82 | `crates/moqui-mail/src/implementation.rs:1663-1677` (`lettre_send`), `crates/moqui-mail/src/auth.rs:408-417` (`signing_domain`), `crates/moqui-mail/src/edge_mail.rs:629-634`, `EmailClientServices.xml:5107-5122` (`create#EmailIdentity`) | A mail user gets the node's DKIM signature on mail From any address of a hosted domain | Medium | likely (lead) |
| SEC-MAIL-83 | `crates/moqui-mail/src/mx.rs:1161-1288` (`session`), 1149-1155 (`Target::step`), 442-491 | A recipient domain's mail server holds an MX delivery session without end and makes the node buffer an unbounded line | Medium | likely (lead) |
| SEC-MAIL-84 | `runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml:467-471` (`update#EmailAccount`), `crates/moqui-mail/src/sync.rs:530-548` (`sign_in`), `crates/moqui-mail/src/oauth.rs:1736-1746` | A manager of a mail account points its server at another host and receives the stored password or OAuth token | Medium | confirmed (lead) |
| SEC-MAIL-85 | `crates/moqui-mail/src/inbound.rs:1122-1137` (`read_line_bytes`), 900-915 (`pop_read_dot`), `crates/moqui-mail/src/sync.rs:444-460` (`raw_command`), 2462 | A mail server a user names can exhaust the server's memory: no read from IMAP or POP3 is bounded | Medium | likely (lead) |
| SEC-MAIL-86 | `crates/moqui-core/src/edge/queue.rs:273-397` (`enqueue`), 1234-1266 (`fail_over_cap`), `crates/moqui-host/src/smtp_queue.rs:418-439` | Mail the edge node answered 250 for is failed, oldest first, when a pairing's queue passes its size cap | Low | reported (confirmed in code) |
| SEC-MAIL-87 | `crates/moqui-mail/src/edge_mail.rs:284-308` (`inbound_item_trusting`), `crates/moqui-core/src/edge/queue.rs:308-329` | On an edge node a second message with a known Message-ID and the same recipients is answered 250 and dropped (SEC-MAIL-08's fix does not cover the queue) | Low | reported (confirmed in code) |
| SEC-MAIL-88 | `crates/moqui-mail/src/auth.rs:666-677` | A trusted ARC sealer overrides DMARC whatever that sealer recorded about the message | Low | reported (confirmed in code) |
| SEC-MAIL-89 | `crates/moqui-mail/src/auth.rs:385-403` (`from_address`), `crates/moqui-mail/src/edge_mail.rs:810-835` (`refused_sender`), 908-960 (`deliver_own`), `crates/moqui-mail/src/store.rs:128-130` | On an all-in-one node the sender rule and the mailbox read different From fields | Low | reported (likely) |
| SEC-MAIL-90 | `crates/moqui-mail/src/auth.rs:408-417`, `crates/moqui-mail/src/edge_mail.rs:618-634` | With `mail_dkim_domains` set, an edge node signs one pairing's mail for another pairing's domain | Low | reported (confirmed in code) |
| SEC-MAIL-91 | `crates/moqui-mail/src/edge_mail.rs:92-106`, 121-180, 1035-1072, 1111-1142; `crates/moqui-host/src/smtp_queue.rs:177-194`, 394-417 | Anyone who has seen a relayed message's Return-Path can report it bounced | Low | reported (likely) |
| SEC-MAIL-92 | `crates/moqui-mail/src/auth.rs:599-608`, 660-665 | A DMARC check that ends in a DNS error accepts the message | Low | reported (confirmed in code) |
| SEC-MAIL-93 | `crates/moqui-mail/src/sync.rs:3292-3324` (`sync_due_accounts`), 1045-1119 (`run_sync`), 2793-2844 (`store_new`), `crates/moqui-mail/src/inbound.rs:1070-1071` | A slow mail server holds the one sync job, and a sync stores without any size or quota limit | Low | reported (likely) |
| SEC-MAIL-94 | `crates/moqui-mail/src/sync.rs:486-492`, 444-457, 905-906, 1155-1176; `EmailClientServices.xml:133` | A sync shows the remote server's own text, which the test was changed to hide (SEC-MAIL-02's fix does not cover the sync) | Low | reported (confirmed in code) |
| SEC-MAIL-95 | `crates/moqui-mail/src/edge_mail.rs:967-1010` (`queue_back`) | Mail queued back to the local node keeps any Authentication-Results that claims the edge node's name | Info | reported (to check) |
| SEC-MAIL-96 | `crates/moqui-host/src/smtp_queue.rs:490-496`, 508-519, 108-131 | Every RCPT and every message of an edge node's listener waits on one lock held across database work | Info | reported (confirmed in code) |
| SEC-MAIL-97 | `crates/moqui-mail/src/oauth.rs:1360`, 1799-1803, 1817; `EmailClientServices.xml:263-267` | The OAuth callback makes an account with authorization disabled, so a shared account needs no mail administrator when the provider row has no IMAP host | Info | reported (to check) |

### Files, DAV, SSH and the terminal

Map file: [Attack-Surface-Files.md](../docs/security/Attack-Surface-Files.md). Reviewer report: `security-review-evidence/slice-8.md` (and `slice-8b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-FIL-01 | `runtime/components/base/webroot/screen/webroot.xml:257` | `/resources` runs elFinder write commands on a GET with no session token | Medium | fixed |
| SEC-FIL-02 | `crates/moqui-host/src/ssh/server.rs:905-925`, `:746-763` | An app password opens the admin shell and the SFTP root over SSH, past the second factor | Medium | fixed |
| SEC-FIL-03 | `crates/moqui-dav/src/dav/files.rs:415-490` | DAV sends a user's HTML or SVG file inline from the application's origin | Medium | fixed |
| SEC-FIL-04 | `crates/moqui-host/src/dav_http.rs:47-59,79` | DAV buffers up to 256 MiB per request before it checks credentials | Medium | fixed |
| SEC-FIL-08 | `webroot.xml:257-272` | `/resources` lets `ADMIN` read any file the server's user can read, in production too | Medium | fixed |
| SEC-FIL-61 | crates/moqui-host/src/ssh/server.rs:1010 | SFTP holds up to `sftp-max-file` in memory per channel, and the handler does not count channels | Medium | fixed |
| SEC-FIL-05 | `runtime/components/base/framework/service/org/moqui/resource/FileServices.xml:2669-2702` | No attempt limit on a public link's password | Low | fixed |
| SEC-FIL-06 | `FileServices.xml:2834-2870`, `:1250` | Anonymous file drop has no size, count or type limit of its own | Low | fixed |
| SEC-FIL-07 | `FileServices.xml:307,341` | Three `FileServices` parameters name a server path and are not checked in the service | Low | fixed |
| SEC-FIL-62 | crates/moqui-host/src/command/shell.rs:87 | The admin shell's `service`, `entity`, `cache` and `js` commands leave no audit record | Low | fixed |
| SEC-FIL-63 | crates/moqui-host/src/command/shell.rs:185 | Admin shell error text and C1 controls reach the administrator's terminal unescaped | Low | fixed |
| SEC-FIL-67 | crates/moqui-dav/src/pim/rrule.rs:35 | Calendar and contact objects have no parser limit of their own; the bound is the 256 MiB DAV body | Low | fixed |
| SEC-FIL-09 | `runtime/components/addon/mr-boss/screen/PublicShare.xml:28,56` | `PublicShare.xml` `open` and `upload` name no method | Info | reported (confirmed in code) |
| SEC-FIL-10 | `FileServices.xml:2649-2655` | A link or share stays valid after its maker loses access | Info | reported (confirmed in code) |
| SEC-FIL-11 | `crates/moqui-host/src/ssh/server.rs:746-763`, `:1073`, `:1196` | SSH permissions are fixed for the life of a connection | Info | reported (confirmed in code) |
| SEC-FIL-64 | crates/moqui-host/src/term/registry.rs:96 | A terminal ticket is reusable for 300 seconds and travels in the `/termws` query string | Info | reported (confirmed in code) |
| SEC-FIL-65 | crates/moqui-host/src/term/access.rs:247 | `terminal` `command-line` and `host-sessions` are on in production for any value but `dev` or `false` | Info | fixed |
| SEC-FIL-66 | crates/moqui-host/src/term/hold.rs:139 | The `term-hold` socket checks file modes only; a connection starts any program | Info | reported (confirmed in code) |
| SEC-FIL-81 | `crates/moqui-host/src/ssh/server.rs:765-772`; `crates/moqui-host/src/term/apps.rs:343-344` | An SSH client sets a terminal size of up to 10 million cells, and each `apps` session draws frames that large | Medium | likely (lead) |
| SEC-FIL-82 | `crates/moqui-dav/src/pim/ics.rs:31-36`, `:54-98` | A calendar file of nested `BEGIN` lines overflows the stack when its parse tree is dropped, which ends the server process | Medium | likely (lead) |
| SEC-FIL-83 | `crates/moqui-dav/src/dav/files.rs:319-337`; `crates/moqui-dav/src/dav/locks.rs:99-158` | Any user a drive is listed for locks any path of it over WebDAV, the whole drive included, and holds every other writer | Medium | likely (lead) |
| SEC-FIL-84 | `crates/moqui-host/src/ssh/sftp_fs.rs:351-407` | An SFTP read handle holds a window as large as the client asks, past the per-user bound of SEC-FIL-61 | Medium | confirmed (lead) |
| SEC-FIL-85 | `crates/moqui-dav/src/pim/rrule.rs:353-379`, `:450-475` | One period of a recurrence rule is built whole before any limit applies: a yearly rule makes about 31.5 million date-times | Medium | confirmed (lead) |
| SEC-FIL-86 | `crates/moqui-dav/src/dav/mod.rs:646-711` | A PROPFIND that asks for many properties is answered once for each child, whole in memory | Medium | reported (likely) |
| SEC-FIL-87 | `crates/moqui-host/src/ssh/server.rs:63-68`, `:1057-1081` | Connections with no user from one address hold every SSH session | Low | reported (confirmed in code) |
| SEC-FIL-88 | `crates/moqui-host/src/ssh/server.rs:1108-1128`, `:1321-1341` | A connection's per-channel maps are never cleared, so repeated channels grow the server's memory | Low | reported (confirmed in code) |
| SEC-FIL-89 | `crates/moqui-host/src/ssh/server.rs:1035-1045`; `crates/moqui-core/src/user/login.rs:527-529` | An SSH key login does not check `ipAllowed` and writes no login history | Low | reported (confirmed in code) |
| SEC-FIL-90 | `crates/moqui-host/src/command/line.rs:142-147`; `crates/moqui-host/src/command/mod.rs:143-178` | The prompt's history stores whole command lines, with service parameters and `js` code, as a user preference | Low | reported (confirmed in code) |
| SEC-FIL-91 | `crates/moqui-host/src/ssh/agent.rs:79-84`, `:194-204` | `agent-apps <id>` waits on a blocking lock on the async runtime while another command runs in that session | Low | reported (likely) |
| SEC-FIL-92 | `crates/moqui-host/src/ssh/server.rs:979-983`, `:1049` | The SSH login lines write the client's username to the log as sent | Low | reported (likely) |
| SEC-FIL-93 | `crates/moqui-dav/src/pim/rrule.rs:227`, `:268-270`; `crates/moqui-dav/src/pim/ics.rs:296-297` | The calendar parsers panic on three malformed values | Low | reported (confirmed in code) |
| SEC-FIL-94 | `crates/moqui-dav/src/pim/ics.rs:893-896`, `:928-930`; `crates/moqui-dav/src/pim/line.rs:160-165` | The iCalendar writer writes RRULE and URL values as stored, so a stored line break adds content lines | Low | reported (likely) |
| SEC-FIL-95 | `crates/moqui-dav/src/dav/xml.rs:73-84`; `crates/moqui-dav/src/pim/vcard.rs:134-143` | Two parses of a 1 MiB DAV body are quadratic | Low | reported (likely) |
| SEC-FIL-96 | `crates/moqui-host/src/ssh/client.rs:508-511`; `crates/moqui-core/src/service/system_sync/sftp.rs:358-359` | The SFTP client reads a remote file whole, with no size bound | Low | reported (confirmed in code) |
| SEC-FIL-97 | `crates/moqui-host/src/term/access.rs:179-189`; `crates/moqui-host/src/command/host.rs:51` | Over SSH a loopback peer is the server's own machine, and a recent sign-in is never asked | Info | reported (confirmed in code) |
| SEC-FIL-98 | `runtime/components/base/framework/service/org/moqui/resource/FileServices.xml:1545-1551`, `:1476-1481` | A drive's `.trash` is guarded in four services only | Info | reported (confirmed in code) |

### Calls, chat and messaging

Map file: [Attack-Surface-Comms.md](../docs/security/Attack-Surface-Comms.md). Reviewer report: `security-review-evidence/slice-9.md` (and `slice-9b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-COM-01 | `runtime/components/base/basic-comms/service/org/moqui/comms/CommsServices.xml:17-45` | Any Comms user lists every account and can message or add it; only calls ask the apps who may be reached | Medium | fixed |
| SEC-COM-02 | `crates/moqui-lib/src/safe_html.rs:30-36` | A chat message may carry an external image, so a sender learns each reader's address and when they read | Medium | fixed |
| SEC-COM-11 | `crates/moqui-host/src/xmpp/mod.rs:596-603` | `ipAllowed` is not checked for XMPP sign-in or its service calls | Medium | fixed |
| SEC-COM-12 | `crates/moqui-core/src/user/login.rs:1303-1310`, `1736-1746` | A connected XMPP app keeps a locked account locked | Medium | fixed |
| SEC-COM-13 | `crates/moqui-rtc/src/turn/mod.rs:473-492` | TURN: usernames from unauthenticated requests are kept without bound | Medium | fixed |
| SEC-COM-14 | `ChatServices.xml:2807-2811`, `3235` | A file uploaded over XMPP outlives the deletion and retention of its message | Medium | fixed |
| SEC-COM-25 | `crates/moqui-sip/src/ua.rs:1871-1888` (failure counting), `ua.rs:469` (the block), `crates/moqui-lib/src/source_guard.rs:121-146` | A forged-source digest failure blocks a trunk's or a phone's address | Medium | fixed |
| SEC-COM-03 | `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:3143-3192` | Chat files have no size limit or quota of their own | Low | fixed |
| SEC-COM-04 | `crates/moqui-host/src/notws/rtc.rs:670-688,691-713,737` | A few anonymous connections keep every guest out of `/callws` | Low | fixed |
| SEC-COM-05 | `runtime/components/base/framework/service/org/moqui/call/Calls.js:1514-1561` (`guestJoin`), `:1414-1438` (`revokeGuest`) | Fifty guest joins fill a link's meeting until it ends, and on the default conf nothing slows them | Low | fixed |
| SEC-COM-06 | `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:4086-4099` | An invisible user's last activity and call state are still answered | Low | fixed |
| SEC-COM-07 | `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:1596,1623,1668,1673` (`add#Member`), `:1774,1799` (`set#MemberRole`), `:4023-4033` (`deliver#AgentMessage`) | A room member's `agentRef` is any service name a moderator gives, and each message then calls it | Low | fixed |
| SEC-COM-08 | `crates/moqui-host/src/rtc/driver.rs:435-449` | Relayed call frames have no rate limit and reach every leg, a guest's included | Low | fixed |
| SEC-COM-15 | `XmppServices.xml:82-90` (`check#Credential`) | An XMPP session outlives the account's `terminateDate` and keeps receiving room messages | Low | fixed |
| SEC-COM-16 | `crates/moqui-xmpp/src/session/chat.rs:303-310` | An upload URL with its token is written to the log | Low | fixed |
| SEC-COM-17 | `crates/moqui-rtc/src/turn/mod.rs:495-503` | TURN allocation quotas are per username, and a leg or user can get many usernames | Low | fixed |
| SEC-COM-18 | `crates/moqui-rtc/src/turn/mod.rs:340-358` | An XMPP user's TURN credential can relay to the host's own public addresses | Low | reported (likely) |
| SEC-COM-19 | `crates/moqui-xmpp/src/registry.rs:182-190` | One user can close another user's XMPP sessions with routed stanzas | Low | deferred |
| SEC-COM-20 | `XmppServices.xml:507-514` (PEP), `592-598` (block list), `611-626` (push), `660-667` and `738-750` (uploads) | Per-user storage over XMPP has no total bound | Low | fixed |
| SEC-COM-21 | `runtime/components/base/basic-comms/screen/BasicComms/Settings/ChatApps.xml:22-27` | Chat apps makes a long-lived, unscoped credential with no recent sign-in | Low | fixed |
| SEC-COM-22 | `crates/moqui-xmpp/src/session/auth.rs:411-421` | SCRAM and FAST sign-ins leave no login history and no last-used date | Low | fixed |
| SEC-COM-26 | `crates/moqui-sip/src/ua.rs:492-499`, `crates/moqui-lib/src/source_guard.rs:214-215` | Unauthenticated connections can hold every SIP TCP and TLS slot | Low | deferred |
| SEC-COM-27 | `crates/moqui-sip/src/media.rs:149-150,264-266`, `crates/moqui-host/src/sip/leg.rs:334` | RTP is sent to whatever address an SDP names, with no address check | Low | fixed |
| SEC-COM-28 | `crates/moqui-sip/src/digest.rs:472-486` | The digest answer's `uri` is not compared with the request | Low | fixed |
| SEC-COM-09 | `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:192,2225,2231,3125,4164`, `:1367` | "Authorization disabled" doubles as "trusted caller" in the chat and call services | Info | reported (confirmed in code) |
| SEC-COM-10 | `crates/moqui-channels/src/telnyx.rs:97-100,122-124,150-161,242-244,388-391` | An SMS bridge's API base may be `http://`, and one setting lifts the address guard | Info | fixed |
| SEC-COM-23 | `crates/moqui-rtc/src/turn/mod.rs:84-122` | `is_public` leaves out some non-public IPv6 ranges | Info | fixed |
| SEC-COM-24 | `crates/moqui-xmpp/src/session/auth.rs:242-248` | The SCRAM decoy differs from a real user by one service call | Info | reported (to check) |
| SEC-COM-29 | `call/Voicemail.js:625-637`, `crates/moqui-host/src/sip/voicemail.rs:371-374` | The phone-access answers tell an existing box from a number with none | Info | reported (likely) |
| SEC-COM-30 | `call/Voice.js:302-309` | The emergency call email has no subject | Info | reported (confirmed in code) |
| SEC-COM-81 | `crates/moqui-core/src/service/web_push.rs:190-274`, `chat_push.rs:323-325`, `:1087-1111` | One signed-in user can stall every user's chat pushes, and make the server post to a host of their choice without limit | Medium | likely (lead) |
| SEC-COM-82 | `crates/moqui-xmpp/src/framing.rs:90-104` | An XMPP client can send a stanza far over the 256 KiB limit, before it signs in, and the server keeps it in memory | Medium | likely (lead) |
| SEC-COM-83 | `crates/moqui-sip/src/ua.rs:447-458`, `:472-490` | A SIP request with a made-up To tag passes the request rate limit and the transaction cap | Low | reported (confirmed in code) |
| SEC-COM-84 | `crates/moqui-sip/src/ua_phones.rs:574-588`, `crates/moqui-sip/src/ua.rs:884`, `:920-925` | A UDP phone's or PBX's Contact decides where the server sends its ACK, BYE and re-INVITE, a new TCP connection included | Low | reported (likely) |
| SEC-COM-85 | `runtime/components/base/framework/service/org/moqui/call/CallCaptionServices.xml:116-118`, `CallRecordings.js:738` | Stored captions are kept for ever and nothing reads them | Low | reported (confirmed in code) |
| SEC-COM-86 | `crates/moqui-sip/src/dtmf.rs:176-186`, `crates/moqui-host/src/sip/leg.rs:305-312`, `crates/moqui-host/src/sip/mod.rs:1575-1587` | Each DTMF packet of a phone leg becomes a task and a service call, with no rate | Low | reported (confirmed in code) |
| SEC-COM-87 | `crates/moqui-rtc/src/audio/opus.rs:82-85`, `jitter.rs:153-154`, `crates/moqui-host/src/rtc/captions.rs:719-721`, `crates/moqui-host/src/sip/leg.rs:218-220` | Hostile Opus packets are decoded by a library that indexes without bounds checks, on threads that do not catch a panic | Low | reported (to check) |
| SEC-COM-88 | `crates/moqui-core/src/service/chat_bridge.rs:1159-1164`, `:1324`, `crates/moqui-channels/src/sms.rs:849-859`, `:1067-1070` | An SMS bridge whose `start` fails still sends, so an `http://` API address still gets the provider key (the fix of SEC-COM-10 does not hold on the send path) | Low | reported (confirmed in code) |
| SEC-COM-89 | `crates/moqui-core/src/service/chat_bridge_edge.rs:384-442`, `:531-583`, `:705`, `:787-822` | On an edge node an anonymous caller spends a Twilio or Sinch bridge's webhook allowance, learns which bridge ids exist and fills its kept refusals | Low | reported (confirmed in code) |
| SEC-COM-90 | `runtime/components/base/framework/service/org/moqui/call/Sms.js:553-561`, `:644-646`, `crates/moqui-core/src/service/chat_bridge.rs:1080-1087` | A STOP or START is remembered for 30 days only, so a signed Twilio text sent again later changes the opt-out again | Low | reported (confirmed in code) |
| SEC-COM-91 | `runtime/components/base/framework/service/org/moqui/call/Sms.js:616-633`, `:657-658`, `crates/moqui-core/src/service/chat_bridge_web.rs:627-645` | Inbound texts and MMS files have no cap: any outside sender makes rooms for every user of the number and stores files without a quota | Low | reported (confirmed in code) |
| SEC-COM-92 | `crates/moqui-core/src/service/chat_push.rs:1166-1216`, `:877`, `:1024-1066` | A chat push that waits is still sent after its message is deleted, the user leaves the room, or the account is disabled | Low | reported (confirmed in code) |
| SEC-COM-93 | `crates/moqui-xmpp/src/session/sm.rs:30`, `:57-66` | Stream management keeps up to 1,000 unacknowledged stanzas a session with no byte bound | Low | reported (likely) |
| SEC-COM-94 | `crates/moqui-sip/src/ua.rs:1744-1749`, `:1634-1651`, `:1666-1686`, `crates/moqui-host/src/sip/mod.rs:1309-1315` | Whoever names a dialog's Call-ID and tags ends the call, moves its media or asks a transfer, from any address | Info | reported (to check) |
| SEC-COM-95 | `crates/moqui-core/src/context/identity.rs:4-22`, `runtime/components/base/basic-comms/screen/BasicComms/Settings/Phone.xml:51` | `pin` is not a secret parameter name, and `setPin` takes it from a query | Info | reported (confirmed in code) |
| SEC-COM-96 | `crates/moqui-channels/src/sms.rs:1245-1253`, `providers/twilio.rs:686-691`, `providers/twilio/group.rs:546-550` | The Twilio API credentials go to whatever host a signed webhook's `MediaUrl` names, and a next-page address is checked by prefix only | Info | reported (confirmed in code) |
| SEC-COM-97 | `crates/moqui-core/src/service/chat_bridge_edge.rs:1276-1305` | `store#EdgeMedia` does not check that the edge queue calls it, and reads any `contentLocation` | Info | reported (confirmed in code) |
| SEC-COM-98 | `crates/moqui-channels/src/sms.rs:771-779`, `ChatBridgeServices.xml:715-717` | A failed delivery's error shows a room's readers the provider's API address and account id | Info | reported (likely) |
| SEC-COM-99 | `runtime/components/base/framework/service/org/moqui/call/Sms.js:300-332` | `open#PairRoom` has no limit: a user with one text number makes a room, with every user of the number in it, for each number typed | Info | reported (confirmed in code) |
| SEC-COM-100 | `crates/moqui-xmpp/src/jids.rs` (`escape_node`), `XmppServices.xml:43-48` | Two accounts whose usernames differ only by case, or by spaces at the ends, have one JID | Info | reported (to check) |
| SEC-COM-101 | `crates/moqui-xmpp/src/session/auth.rs:356-392`, `XmppServices.xml:104-108`, `:146-153` | A FAST token is taken by either mechanism, and the count that stops a replay is not covered by the proof | Info | reported (confirmed in code) |

### Cluster and edge node

Map file: [Attack-Surface-Cluster-Edge.md](../docs/security/Attack-Surface-Cluster-Edge.md). Reviewer report: `security-review-evidence/slice-10.md` (and `slice-10b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-CLU-01 | `crates/moqui-host/src/edge_link/down.rs:223` | The address an edge node names passes the member's loopback rules | Low | fixed |
| SEC-CLU-02 | `crates/moqui-host/src/edge_link/accept.rs:2910`-`2923`, `:2893` | A paired node can fill the other end's database through Replicate and PutContent | Low | fixed |
| SEC-CLU-03 | `crates/moqui-host/src/ssh/server.rs:939`-`952`, `:1099`-`1112` | The cluster login is accepted on the SSH port whatever the transport | Low | fixed |
| SEC-CLU-04 | `crates/moqui-core/src/cluster/mod.rs:1327`-`1350`, `:1476`-`1483` | A `ClusterMember` row decides where cluster messages go and, for `ssh://`, which host key is trusted | Low | fixed |
| SEC-CLU-05 | `crates/moqui-host/src/pass_authority/mod.rs:131`-`154`, `:189`-`213` | The PASS authority's wrong-secret limit is keyed by the exact TCP peer | Low | fixed |
| SEC-CLU-06 | `runtime/components/base/webroot/screen/webroot/js/MoquiLib.js:736` | A notification's `link` is followed with no scheme check | Low | fixed |
| SEC-CLU-07 | `crates/moqui-devkit/src/deploy/git.rs:1023`-`1066`, `:798`-`801` | A cluster message makes a member load a commit the sender names | Info | reported (confirmed in code) |
| SEC-CLU-81 | `crates/moqui-core/src/edge/pool.rs:209`-`217`, `:313`-`316`; `crates/moqui-host/src/edge_link/accept.rs:2477`-`2485` | A node with only the pairing's shared key links as a listed member that has no key pinned, with no notice | Low | reported (confirmed in code) |
| SEC-CLU-82 | `crates/moqui-host/src/edge_link/store.rs:156`-`193`; `crates/moqui-host/src/edge_link/accept.rs:2705`-`2721` | A valid pairing code pairs any revoked pairing id with a new key, which then has what was kept for that id | Low | reported (confirmed in code) |
| SEC-CLU-83 | `crates/moqui-host/src/edge_link/queue.rs:208`-`256`; `crates/moqui-host/src/edge_link/accept.rs:1623`-`1627` | `PutContent` holds up to 64 MiB a call in memory before the pairing's cap is checked, on a connection with no stream limit of its own | Low | reported (confirmed in code) |
| SEC-CLU-84 | `crates/moqui-host/src/edge_link/accept.rs:1567`-`1584`; `crates/moqui-host/src/edge_link/store.rs:79`-`94` | An up connection with any certificate costs a database transaction and an info log line, with no rate limit | Low | reported (confirmed in code) |
| SEC-CLU-85 | `crates/moqui-core/src/edge/setup.rs:476`-`482`, `:511`-`527` | The PASS client id of the edge node's offer is written unseen at Apply, over an existing client of that id | Low | reported (confirmed in code) |
| SEC-CLU-86 | `runtime/components/base/framework/service/org/moqui/call/Voicemail.js:343`-`366`; `runtime/components/base/framework/service/org/moqui/call/CallRecordings.js:615`-`635` | The voicemail and recording queue handlers do not refuse the wrong role | Low | reported (likely) |
| SEC-CLU-87 | `crates/moqui-host/src/tui.rs:667`-`673`; `crates/moqui-tui/src/session/shell.rs:810`-`814`; `crates/moqui-tui/src/ui/paint.rs:195`-`213` | A notification's title is drawn on a TUI user's terminal with its control characters | Low | reported (confirmed in code) |
| SEC-CLU-88 | `crates/moqui-host/src/grpc_listen.rs:230`-`233`, `:264`-`267` | The gRPC listener has no idle, keepalive or request timeout | Low | reported (confirmed in code) |
| SEC-CLU-89 | `crates/moqui-entity/src/entity/sensitive.rs:47`-`119`; `crates/moqui-core/src/cluster/mod.rs:938`-`943`, `:1591`-`1593` | A `ClusterMember` row is not among the entities the generic tools refuse, and one with a future heartbeat is live for ever | Low | reported (likely) |
| SEC-CLU-90 | `crates/moqui-core/src/edge/media.rs:357`-`362`; `crates/moqui-core/src/service/call_media.rs:262`-`314` | On a dedicated edge node every pairing's greetings and prompts are kept at one location, with no cap and no removal | Low | reported (confirmed in code) |
| SEC-CLU-91 | `runtime/components/base/framework/service/org/moqui/call/Voice.js:924`-`937`, `:944`-`976` | As the edge node's delegate user, `record#AuthFailure` blocks any source address and `note#TrunkStatus` marks any trunk, each with an alert | Low | reported (confirmed in code) |
| SEC-CLU-92 | `crates/moqui-core/src/edge/caps.rs:119`-`150`; `crates/moqui-core/src/edge/replica.rs:707`-`722` | A pairing's replicated records are capped by count, not by size, though SEC-CLU-02's fix names bytes | Low | reported (likely) |
| SEC-CLU-93 | `crates/moqui-core/src/service/edge.rs:813`-`826`; `crates/moqui-core/src/edge/pool.rs:231`-`236` | An approval of a held node names its instance id, not its key | Info | reported (confirmed in code) |
| SEC-CLU-94 | `runtime/components/base/tools/screen/System/EdgeNode/Pair/EdgeUrl.xml:35`, `Connect.xml:33`; `crates/moqui-core/src/user/session.rs:2729`-`2730` | The pairing code is kept in clear in the session row while the wizard is open | Info | reported (likely) |
| SEC-CLU-95 | `runtime/components/base/framework/service/org/moqui/call/CallTransfers.js:428`-`443`; `runtime/components/base/framework/service/org/moqui/call/Calls.js:2299`-`2330` | The edge node acts as a phone's user with no fresh digest answer through `refer#Call` and a device leg's `answered` | Info | reported (confirmed in code) |

### Data layer and stored secrets

Map file: [Attack-Surface-Data.md](../docs/security/Attack-Surface-Data.md). Reviewer report: `security-review-evidence/slice-12.md` (and `slice-12b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-DAT-01 | `crates/moqui-entity/src/entity/search_form.rs:189-195`, `:86-92` | A find by request parameters takes conditions and order on password-hash and encrypted fields | Medium | fixed |
| SEC-DAT-02 | `runtime/components/addon/mr-mantle/data/MantleDocumentData.xml:55` | An encrypted field in a data document is indexed in plaintext | Medium | fixed |
| SEC-DAT-03 | `runtime/components/base/tools/service/moqui.rest.xml:123-126`, `:179-183` | REST entity reads return stored credentials in plaintext | Medium | fixed |
| SEC-DAT-04 | `crates/moqui-db/src/sql/view.rs:1195-1209` | A view alias function from a database row may be any SQL function name | Medium | fixed |
| SEC-DAT-05 | `runtime/components/base/framework/service/org/moqui/impl/EntityServices.xml:252`, `:276-284` | `export#EntityDataSnapshot` writes to a path built from an unchecked `baseFilename` | Low | fixed |
| SEC-DAT-06 | `postgresql_embedded-0.22.0/src/postgresql.rs:256` (reached from `crates/moqui-drivers/src/embedded.rs:769`) | Files with secrets are created with the process umask | Low | fixed |
| SEC-DAT-07 | `crates/moqui-drivers/src/embedded.rs:590-603`, `:906-927` | The fallback socket directory under the temp directory is accepted when it already exists | Low | fixed |
| SEC-DAT-08 | `crates/moqui-core/src/service/system_sync/entity_sync.rs:246-299`, `:220-240` | `get#EntitySyncData` reads any entity, and `put#EntitySyncData` takes `UserAccount` and job rows | Low | fixed |
| SEC-DAT-09 | `crates/moqui-entity/src/entity/search_form.rs:98-113` | No upper bound on `pageSize`, and `pageNoLimit=true` is honored, for screens and Service REST | Low | fixed |
| SEC-DAT-10 | `crates/moqui-entity/src/entity/field_rw.rs:187-198` | Stored text that is not ciphertext is read as a valid value | Low | fixed |
| SEC-DAT-11 | `crates/moqui-core/src/user/filter.rs:155`, `:376-378` | Row filters fail open when the set has `allowMissingAlias="Y"` or its `applyCond` is false | Low | fixed |
| SEC-DAT-12 | `runtime/components/addon/mr-mantle/entity/AccountingAccountEntities.xml:1144-1145` | Regulated identifiers stored in clear | Info | confirmed (second check) |
| SEC-DAT-13 | `deny.toml:6-8`, `:10-18` | The advisory check covers one target and the default features | Info | not confirmed (second check) |

### PASS frontend (mr-pass)

Map file: [Attack-Surface-Apps-PASS.md](../docs/security/Attack-Surface-Apps-PASS.md). Reviewer report: `security-review-evidence/slice-14.md` (and `slice-14b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-PSF-01 | `service/pass/Facade.js:25-32` | In local mode a login key in a header drives every PASS page, without the session token | Medium | fixed |
| SEC-PSF-02 | 57 transitions (list in A8), for example `screen/Forum/HeldMail.xml:13`, `screen/Shop/Checkout.xml:7,37,43`, `screen/Forum/Topics/TopicDetail.xml:37` | Write transitions without `method="post"` run on a GET that carries no query string, with no session token | Low | fixed |
| SEC-PSF-03 | `service/pass/RouteServices.xml:46-49` | Bearer values in PASS URLs that the log and Visit redaction do not cover | Low | fixed |
| SEC-PSF-04 | `screen/Shared/Headers.js:8-16`. | The shell's CSP has no `default-src` and allows `'unsafe-eval'` and every same-origin script | Low | fixed |
| SEC-PSF-05 | `service/pass/Transport.js:264-287`. | The REST transport sends the client secret and visitors' delegations to whatever scheme `pass_backend_url` names | Low | fixed |
| SEC-PSF-08 | `screen/Smithy/SmithySite.js:392-398` | A Smithy release file is served from the site's origin with a script content type, so `script-src 'self'` stops no injected script | Low | fixed |
| SEC-PSF-06 | `screen/Shop/ShopPreview.js:35-57` | The resolver's preview-host check fails open | Info | reported (confirmed in code) |
| SEC-PSF-07 | `service/pass/Account.js:85, 224` | Bearer values sit in the frontend's session rows as written | Info | reported (to check) |
| SEC-PSF-09 | `screen/Smithy/Files.html.fancy:22` (`view.readme.html`), `screen/Smithy/Blob.html.fancy:3` (`view.html`), `screen/Smithy/GroupOverview.html.fancy:3` (`overviewView.landingHtml`), `screen/Smithy/SmithySite.js:152` (a Docs page), `:182, 314` (release notes). | Authority HTML is written without the frontend's own cleaning (five Smithy fields, forum posts) | Info | reported (confirmed in code) |

### PASS authority and public accounts

Map file: [Attack-Surface-Apps-PASS.md](../docs/security/Attack-Surface-Apps-PASS.md). Reviewer report: `security-review-evidence/slice-15.md` (and `slice-15b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-PSA-01 | `FW/service/org/moqui/public/Account.js:536-537` | PASS sign-in has no rate limit on the authority | Medium | fixed |
| SEC-PSA-02 | `FW/service/org/moqui/public/Account.js:339-344` (also `:473`, `:511`) | Failed logins by someone else end a PASS visitor's delegation | Low | fixed |
| SEC-PSA-03 | `MM/script/PassApi.js:215-219` | A wrong client secret is not counted on the REST servlet | Low | fixed |
| SEC-PSA-04 | `crates/moqui-host/src/pass_authority/mod.rs:135-147, 190-196` | The gRPC wrong-secret limit is per connection address and stops right secrets too | Low | fixed |
| SEC-PSA-05 | `MM/script/PassAuthority.js:387-433` | A registered client reads other sites as an anonymous caller | Low | fixed |
| SEC-PSA-06 | `FW/service/org/moqui/public/Account.js:128-198, 539-549` | Registration and recovery tell whether an account exists | Low | reported (likely) |
| SEC-PSA-07 | `MM/MoquiConf.xml:51-53` | A multipart POST to the servlet may be 2 GiB | Low | fixed |
| SEC-PSA-08 | `MM/script/PassApi.js:153-182` | A REST download is read whole into the script | Low | reported (to check) |
| SEC-PSA-09 | `crates/moqui-core/src/context/identity.rs:3-41` | `returnKey` is not a secret name for the request log | Low | fixed |
| SEC-PSA-10 | `crates/moqui-core/src/pass_authority.rs:556-569, 618-639` | A gRPC session handle may stay signed in after an account action sent with a delegation | Low | reported (to check) |
| SEC-PSA-11 | `FW/service/org/moqui/public/Account.js:149-159, 255-264` | Public accounts are ordinary accounts on the business host | Low | reported (to check) |
| SEC-PSA-12 | `MM/service/mantle/pass/PassAuthorityServices.xml:8-33` | `handle#Call` trusts `clientVerified` from its caller | Info | reported (confirmed in code) |
| SEC-PSA-13 | `MM/script/PassApi.js:318-327` | Account action names are looked up in plain objects | Info | reported (confirmed in code) |

### PASS application services (Mantle)

Map file: [Attack-Surface-Apps-PASS-Commerce.md](../docs/security/Attack-Surface-Apps-PASS-Commerce.md). Reviewer report: `security-review-evidence/slice-16.md` (and `slice-16b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-PSS-01 | `M/product/ShopFulfillment.js:118`, `:525`, `:612` | A download product's file location is not restricted | Medium | fixed |
| SEC-PSS-21 | `request/SupportPortal.js:758-765`, `:775`, `:833` | `createCustomerCase` leaves the staged upload on disk when the operation is a repeat | Medium | fixed |
| SEC-PSS-41 | `service/mantle/marketing/MarketingPortal.js` reserveRegistration (starts 1058), registrationView (starts 961), subscriberOf (198-231) | An anonymous event reservation answers the directory name of the party that owns a typed email address | Medium | fixed |
| SEC-PSS-42 | `service/mantle/forum/Forum.js` publishSitePage (starts 3275), getSitePage (starts 1976) | A site owner can publish any wiki page of an unrestricted space as the site's public page | Medium | fixed |
| SEC-PSS-61 | runtime/components/addon/mr-mantle/service/mantle/marketing/MarketingPortal.js:198 (subscriberOf), :1019, :1058 | An anonymous event reservation answers the stored name of the party that owns a typed email address | Medium | fixed |
| SEC-PSS-02 | `M/order/ShopCheckout.js:1212` | The checkout return key and the cart recovery token are query parameters the request log keeps | Low | fixed |
| SEC-PSS-03 | `M/product/ShopFulfillment.js:490-506`, `:573-588` | A ten-minute download capability is accepted as an order link and can issue its successor | Low | fixed |
| SEC-PSS-04 | `M/party/ShopCustomer.js:114-160` | A guest order with a typed email is merged into the account that verifies that email | Low | reported (to check) |
| SEC-PSS-05 | `M/order/ShopCart.js:155-190` | Anonymous cart creation is limited per requester only | Low | fixed |
| SEC-PSS-06 | `M/order/ShopCheckout.js:677`, `:1174` | A zero-total guest checkout mails any typed address | Low | reported (to check) |
| SEC-PSS-08 | `M/product/ShopCatalog.js:560-600` | Shop search and catalog reads have no limit at the authority | Low | reported (to check) |
| SEC-PSS-22 | `request/SupportPortal.js:751-761` | `createCustomerCase` stages and scans the upload before it checks who asks | Low | fixed |
| SEC-PSS-23 | `request/SupportPortal.js:149-166` (`askers`), `order/CustomerOrders.js:133-162`, `order/CustomerReturns.js:287-341`, `request/SupportPortal.js:806-809` | A guest's order link (`OrderView`, a read scope) authorizes cancel, return and case writes | Low | fixed |
| SEC-PSS-24 | `account/Portal.js:426-438`, `:467-475` | The BillUpload page shows the purchase order's current total to a superseded or not-yet-sent revision's link | Low | fixed |
| SEC-PSS-25 | `request/SupportPortal.js:466`, `:472-503` | `rateArticle`'s "nonce" is any text the client sends | Low | fixed |
| SEC-PSS-26 | `account/Portal.js:309-360` (`startInvoicePayment`), `:485-554` (`submitSupplierBill`), `:195-206` (`getInvoice`) | Billing's and the bill upload's writes have no post limit; a signed-in contact has none at all | Low | fixed |
| SEC-PSS-27 | `account/Portal.js:336-340`, `:364-382` | The invoice payment's return key rides in URL parameters the request log does not redact | Low | fixed |
| SEC-PSS-28 | `request/SupportPortal.js:767-804`, `request/Channels.js:720-763`, `request/RequestServices.xml:887-921` | The public case form takes the typed email as the sender | Low | fixed |
| SEC-PSS-43 | `service/mantle/forum/Forum.js` redeemInvitation (starts 2323) | Redeeming an invitation reactivates a member the site's staff suspended or removed | Low | fixed |
| SEC-PSS-44 | `service/mantle/party/ConsentServices.xml` create#UnsubscribeToken (starts 1289), resolve#UnsubscribeToken (1428), apply#UnsubscribePreferences (1506) | A list link's token does not expire by default, is not single use, has no lookup limit and also subscribes | Low | fixed |
| SEC-PSS-45 | `service/mantle/public/AccountHub.js:360-368` | PASS's one-click unsubscribe is not a confirmed unsubscribe on a list that asks confirmation | Low | fixed |
| SEC-PSS-62 | crates/moqui-core/src/script/theme/markup.rs:124 (DENIED_ELEMENTS), :175 (DENIED_ATTRIBUTES), :628 (check_url, navigation) | A theme may hold a `<meta>` element and links to any host | Low | fixed |
| SEC-PSS-65 | runtime/components/addon/mr-mantle/service/mantle/party/PortalMedia.js:20 | `PortalMedia.sniff` reads the whole staged upload into memory to compare eight bytes | Low | fixed |
| SEC-PSS-66 | runtime/components/addon/mr-mantle/service/mantle/party/ExternalAccess.js:386 | `check()` skips the organization comparison when its caller names none, and the intake form's check names none | Low | fixed |
| SEC-PSS-67 | runtime/components/addon/mr-mantle/service/mantle/party/ShopCustomer.js:587, :751, :779 | Shop's address and card setup operations have no per-requester limit | Low | fixed |
| SEC-PSS-07 | `M/order/ShopCheckoutServices.xml:167` (`run#Step`), `:181`, `:201`, `:216` | Internal step and job services are `anonymous-all` | Info | reported (confirmed in code) |
| SEC-PSS-46 | `service/mantle/marketing/MarketingPortal.js` requestSubscription (starts 419), reserveRegistration (starts 1058) | The neutral limits of public subscription and registration can be used to shut out a site or an address | Info | reported (confirmed in code; a helper reader's item, not checked by the slice's reviewer) |
| SEC-PSS-63 | crates/moqui-core/src/script/theme/expr.rs:140 | The theme expression check does not see a property name built at run time | Info | reported (confirmed in code) |
| SEC-PSS-64 | runtime/components/addon/mr-pass/screen/Shop/ShopCheckout.js:891 | The checkout relay writes the answered form address without the host check the card relay makes | Info | reported (confirmed in code) |

### Forum and Smithy

Map file: [Attack-Surface-Apps-PASS-Forum-Smithy.md](../docs/security/Attack-Surface-Apps-PASS-Forum-Smithy.md). Reviewer report: `security-review-evidence/slice-17.md` (and `slice-17b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-PFS-61 | `runtime/components/addon/mr-mantle/service/mantle/smithy/SmithyGit.js:385-415` | A Smithy access token made for git is the account's whole login key, with no scope | Medium | fixed |
| SEC-PFS-01 | `mr-mantle/service/mantle/work/GitServices.xml:1119, 1128, 1189` | The framework's review-thread REST gives party ids and party names of a Smithy repository's commenters | Low | fixed |
| SEC-PFS-02 | `mr-mantle/service/mantle/forum/Forum.js:2494-2599` | Staged forum uploads have no count or total size limit per member | Low | fixed |
| SEC-PFS-62 | `runtime/components/addon/mr-mantle/service/mantle/smithy/SmithyGit.js:225` | The webhook secret is inside the stored hash of `storeWebhook`'s receipt | Low | fixed |
| SEC-PFS-63 | `runtime/components/addon/mr-mantle/service/mantle/smithy/SmithyGit.js:290-306` | For an account with no password, "sign in again" is any sign-in of the account in the last 15 minutes | Low | fixed |
| SEC-PFS-64 | `runtime/components/addon/mr-mantle/service/mantle/forum/Forum.js:5113-5143` | Mail is taken as authenticated on a DKIM pass for any parent of the From domain | Low | fixed |
| SEC-PFS-03 | `mr-mantle/service/mantle/smithy/SmithyGitServices.xml:9, 39, 53, 72, 89, 102, 117` | Hook and worker services are `anonymous-all` with the user named in a parameter | Info | reported (to check) |
| SEC-PFS-65 | `runtime/components/addon/mr-mantle/service/mantle/smithy/SmithyGit.js:185-200, 216` | `findWebhooks` answers a webhook's whole URL, which may hold a credential | Info | reported (confirmed in code) |

### BOSS access model; Admin, Commons, People, Accounting

Map file: [Attack-Surface-Apps-BOSS.md](../docs/security/Attack-Surface-Apps-BOSS.md). Reviewer report: `security-review-evidence/slice-18.md` (and `slice-18b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-BOS-01 | crates/moqui-core/src/user/filter.rs:144 and runtime/components/addon/mr-boss/data/BossAccountingSetupData.xml:209 | Organization row filters do not apply below a sub-area ALWAYS grant | Medium | fixed |
| SEC-BOS-02 | runtime/components/addon/mr-mantle/service/mantle/ledger/AccountingPolicy.js:83 and runtime/components/addon/mr-mantle/service/mantle/party/PartyServices.xml:626 | App administrators and ACCOUNTING_ADMIN are not bound to an organization | Medium | fixed |
| SEC-BOS-03 | runtime/components/addon/mr-boss/screen/AcceptInvitation.xml:35 | An invitation is accepted by a bare GET for a signed-in user | Medium | fixed |
| SEC-BOS-04 | runtime/components/addon/mr-boss/screen/Unsubscribe.xml:53 | Unsubscribe and public-share changes run on a bare GET | Low | fixed |
| SEC-BOS-05 | runtime/components/addon/mr-boss/screen/PublishedCalendar.xml:27 and PublishedCalendar.qvue:120 | A published calendar's password travels in the query string | Low | fixed |
| SEC-BOS-06 | runtime/components/addon/mr-mantle/service/mantle/account/PaymentRuns.js:354 and InvoiceWorkflow.js:142 | Accounting separation of duties is a preference, and two bundles hold both sides | Low | reported (to check) |
| SEC-BOS-07 | runtime/components/addon/mr-boss/skill/reset-factor.md:8 | The reset-factor skill is declared reversible | Low | fixed |
| SEC-BOS-08 | runtime/components/addon/mr-boss/data/BossSetupData.xml:5274 and runtime/components/addon/mr-boss/service/ironstone/admin/Provisioning.js:85 | request#Provisioning is authorized for ALL_USERS and checks no permission itself | Low | fixed |
| SEC-BOS-09 | runtime/components/addon/mr-boss/data/BossUsersDemoData.xml:47 | Demo accounts share one password hash and 47 demo loaders are anonymous-all | Info | reported (to check) |

### BOSS business apps and the Mantle REST root

Map file: [Attack-Surface-Apps-BOSS.md](../docs/security/Attack-Surface-Apps-BOSS.md). Reviewer report: `security-review-evidence/slice-19.md` (and `slice-19b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-BAP-01 | runtime/components/addon/mr-mantle/service/mantle/work/CalendarServices.xml:785-822 and 898-925 | Calendar subscription makes the server fetch any URL, private addresses included | Medium | fixed |
| SEC-BAP-02 | runtime/components/addon/mr-mantle/service/mantle/account/PaymentMethodServices.xml:509 and :555 | A bank account number is copied whole into the payment method description | Medium | fixed |
| SEC-BAP-03 | runtime/components/addon/mr-mantle/service/mantle/account/GatewayConnections.js:112-124 | A gateway's address is checked against its provider's hosts only on a production instance, and private addresses are refused only when `allowPrivateNetwork` is `N` | Low | reported (likely) |
| SEC-BAP-04 | runtime/components/addon/mr-mantle/service/mantle/account/PaymentMethodServices.xml:17-70 | `get#PartyPaymentMethodInfo` returns the whole card and bank account records and ignores its `partyId` | Low | fixed |
| SEC-BAP-05 | runtime/components/addon/mr-mantle/service/mantle/account/PaymentServices.xml:1891-1895 | A card security code stays stored until an authorization succeeds | Low | fixed |
| SEC-BAP-06 | runtime/components/addon/mr-mantle/service/mantle/shipment/ShipmentServices.xml:1981 | Label fetches GET a stored URL with no internal-address guard | Low | fixed |
| SEC-BAP-08 | runtime/components/addon/mr-mantle/service/mantle/ext/account/NachaServices.xml:19, OfxServices.xml:16-53, BaiServices.xml:16-53 | The bank file services hold no permission or organization check | Low | fixed |
| SEC-BAP-07 | runtime/components/addon/mr-mantle/data/MantleSetupData.xml:17-20 | Under `/rest/s1/mantle` an administrator's calls run with no row filter set and past every BOSS sub-area DENY | Info | reported (confirmed in code) |
| SEC-BAP-09 | runtime/components/addon/mr-boss/screen/Commerce.xml:32 | The Commerce store limit is one entity filter set by one screen | Info | reported (to check) |

### Clients

Map file: [Attack-Surface-Clients.md](../docs/security/Attack-Surface-Clients.md). Reviewer report: `security-review-evidence/slice-11.md` (and `slice-11b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-CLI-01 | crates/moqui-desktop/src/node/mod.rs:271, crates/moqui-core/src/web/origin.rs:127, runtime/components/base/webroot/screen/webroot/Login.xml:116 | The sidecar's loopback web listener is open to every local user and has no Host check seen | Medium | fixed |
| SEC-CLI-02 | runtime/components/base/webroot/screen/webroot/sw.js:28 | The service worker keeps any same-origin GET that has a `v` parameter, cache first, with no expiry | Low | fixed |
| SEC-CLI-03 | crates/moqui-desktop/src/lib.rs:234 | The main window's navigation check in node modes passes every scheme but five | Low | deferred |
| SEC-CLI-04 | crates/moqui-client-tauri/src/scheme.rs:319, crates/moqui-desktop/src/node/scheme.rs:195, crates/moqui-core/src/context/factory.rs:93 | In the apps the server's default CSP is dropped whole, `form-action` and `frame-ancestors` with it | Low | deferred |
| SEC-CLI-05 | crates/moqui-desktop/src/bridge.rs:288, crates/moqui-desktop/src/os/mod.rs:214 | A node page can set where OS mail and call links open, and it is kept across runs | Low | deferred |
| SEC-CLI-06 | crates/moqui-mobile/src/scheme.rs:173 | On mobile `scheme_fetch` trusts the webview's URL and the caller's own headers | Info | reported (confirmed in code) |
| SEC-CLI-07 | crates/moqui-client-tauri/src/lib.rs:96, crates/moqui-desktop/src/client/windows.rs:95 | The shell's commands in the desktop client are not bound to an account | Info | reported (confirmed in code) |
| SEC-CLI-08 | crates/moqui-client/src/store/mod.rs:21, crates/moqui-client-tauri/src/commands.rs:575 | A personal account's mail and chat stay on disk in plain SQLite after sign-out | Info | reported (confirmed in code) |
| SEC-CLI-09 | crates/moqui-desktop/src/os/instance.rs:137 | The single-instance socket is restricted only after it is bound | Info | reported (confirmed in code) |

### Script engine, templates and parsers

Map file: [Attack-Surface-Scripts.md](../docs/security/Attack-Surface-Scripts.md). Reviewer report: `security-review-evidence/slice-13.md` (and `slice-13b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-SCR-01 | runtime/components/base/tools/screen/Tools/Data/Export.xml:18 and :56 | Data Export `filterMap` runs any server script, on a GET, without the script permission | High | fixed |
| SEC-SCR-05 | crates/moqui-core/src/notify/message.rs:322 and :899-911 | Notification titles are expanded as script, and three callers build them from record values | High | fixed |
| SEC-SCR-02 | crates/moqui-core/src/service/tools.rs:204-208 | `print#ScreenDocument` evaluates its `screenParametersStr` parameter as a script | Medium | fixed |
| SEC-SCR-03 | crates/moqui-core/src/script/host/globals.rs:562-565 | `System.getProperty` returns secret properties and the whole environment to any script | Medium | fixed |
| SEC-SCR-04 | crates/moqui-core/src/script/host/tools.rs:721 and :2603-2620 | `ec.resource.runTokenCommand` starts a host program from any server script | Medium | fixed |
| SEC-SCR-06 | crates/moqui-core/src/script/oxide/resource.rs:227-237 (localized text expanded) | Records whose text is evaluated as script are writable without the script permission | Medium | fixed |
| SEC-SCR-07 | crates/moqui-core/src/script/host/tools.rs:2720-2726 and the `RestClient` shim near :903 | `ec.service.rest()` reaches internal addresses unless the script opts out | Low | fixed |
| SEC-SCR-08 | runtime/components/base/framework/template/screen/ScreenHtml.js:849 | The `editable` widget writes its text unescaped by default | Low | fixed |
| SEC-SCR-09 | crates/moqui-core/src/script/host/files.rs:455 | A thumbnail decodes an image with no limits of its own | Low | fixed |
| SEC-SCR-10 | crates/moqui-host/src/chromium.rs:312-327 | Chromium's DevTools port has no authentication on loopback | Low | fixed |
| SEC-SCR-12 | `crates/moqui-core/src/screen/definition.rs:1694` | A subscreen row's menu title is expanded as script, in menus and in the breadcrumb | Low | fixed |
| SEC-SCR-11 | crates/moqui-core/src/screen/definition.rs:910 | A screen definition loads from any resource scheme, `http` and `dbresource` included | Info | reported (to check) |
| SEC-SCR-81 | `runtime/components/base/framework/service/org/moqui/resource/WikiServices.xml:205`, `:592`, `:716-721`; `runtime/components/addon/mr-marble/screen/SimpleScreens/Wiki/wiki.xml:228-230`, `:267-268`; `crates/moqui-core/src/script/template.rs:105-113`, `:345-355`, `:452-461` | A wiki page whose type is `fancy`, `html.fancy`, `md.fancy` or `markdown.fancy` runs as server script for every viewer | High | confirmed (lead) |
| SEC-SCR-82 | `crates/moqui-core/src/script/template.rs:827-836`, `:980-982` | A template with nested template literals overflows the native stack in the tokenizer, which ends the server process | Medium | likely (lead) |
| SEC-SCR-83 | `crates/moqui-core/src/script/oxide/templates.rs:284-306`, `:345-366`; `crates/moqui-core/src/script/theme/expr.rs:33-69` | A locked template leaves promise jobs in its thread's sandbox, never run or dropped, until every locked render on that thread fails for memory | Medium | likely (lead) |
| SEC-SCR-84 | `runtime/components/base/framework/service/org/moqui/resource/WikiServices.xml:205`, `:592`; `crates/moqui-core/src/context/resource.rs:542-546`, `:1093` | `wikiType` is joined into a new page's location unchecked, so on file storage a page editor writes a file outside the wiki space | Medium | likely (lead) |
| SEC-SCR-85 | `crates/moqui-core/src/service/images.rs:177-180`, `:279`, `:446`, `:487`; `crates/moqui-core/src/script/host/files.rs:457` | `upload#RichTextImage` and `write#Image` decode a whole image outside the concurrency slot | Low | reported (confirmed in code) |
| SEC-SCR-86 | `crates/moqui-lib/src/xml_edit/outline.rs:70`, `:110`; `crates/moqui-lib/src/xml_edit/mod.rs:431`, `:507` | The XML editing layer recurses with no depth limit, so a deeply nested text ends the process | Low | reported (likely) |
| SEC-SCR-87 | `crates/moqui-lib/src/node/parse.rs:554` | One XML start tag costs time quadratic in its attributes | Low | reported (confirmed in code) |
| SEC-SCR-88 | `runtime/components/base/framework/template/screen/ScreenCsv.js:35-39`, `:92-93`; `ScreenSupport.js:243` | The CSV render mode guards and quotes each widget's text on its own, so a field with two widgets can put a formula in a cell of its own | Low | reported (likely) |
| SEC-SCR-89 | `crates/moqui-tools/src/grid.rs:1329-1334`, `:1606-1607`; `crates/moqui-entity/src/entity/query_rest.rs:2113-2121`; `crates/moqui-tools/src/etl/transform.rs:95-110` | The data grid's CSV export and the entity CSV writers have no formula guard | Low | reported (confirmed in code) |
| SEC-SCR-90 | `crates/moqui-core/src/script/theme/markup.rs:706-779`, `:784-805` | A CSS comment that starts and ends inside strings hides a `url()` or an `@import` from the theme's CSS check | Low | reported (confirmed in code) |
| SEC-SCR-91 | `crates/moqui-core/src/script/theme/markup.rs:124-144`, `:175-187` | A theme's markup may set the page's referrer policy, and the `attributionsrc` attribute is not denied | Low | reported (confirmed in code) |
| SEC-SCR-92 | `crates/moqui-core/src/screen/saved_finds.rs:1308-1312`; `runtime/components/base/framework/entity/Security.eecas.xml:142-146` | Record fields that name a template or screen location are not held by SEC-SCR-06's rule | Low | reported (likely) |
| SEC-SCR-93 | `crates/moqui-core/src/screen/html_text.rs:238` | The tui link list refuses only a lower-case `javascript:` link | Info | reported (to check) |

### Outbound requests, processes and packaging

Map file: [Attack-Surface-Outbound.md](../docs/security/Attack-Surface-Outbound.md). Reviewer report: `security-review-evidence/slice-21.md` (and `slice-21b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-OUT-01 | crates/moqui-mail/src/implementation.rs:1916 | SMTP credentials go out without TLS when the server offers no STARTTLS, or when neither TLS flag is set | Medium | fixed |
| SEC-OUT-02 | crates/moqui-core/src/service/sso.rs:237 | The address guard fails open on stored rows: it applies only when `allowPrivateNetwork` is exactly `N` | Medium | fixed |
| SEC-OUT-03 | crates/moqui-core/src/telemetry/scrape.rs:557 | Scrape targets, synthetic checks, EntitySync, `remote-rest` and resource locations never use the guard | Medium | fixed |
| SEC-OUT-07 | podman/moqui-postgres-compose.yml:32 | Compose files default the database password to `moqui` with TLS off and the port published on the host | Medium | fixed |
| SEC-OUT-08 | runtime/components/base/framework/service/org/moqui/dev/InstanceServices.xml:447 | A stored field is run as a program: `runTokenCommand(instanceImage.authTokenCmd)`; the Docker API defaults to plain HTTP on 2375 | Medium | fixed |
| SEC-OUT-04 | crates/moqui-mail/src/mx.rs:978 | MX delivery connects to any address a recipient domain's DNS names, internal ones included | Low | fixed |
| SEC-OUT-05 | crates/moqui-devkit/src/deploy/git.rs:1506 | The deploy token is in the `git` child's argument list | Low | fixed |
| SEC-OUT-06 | crates/moqui-drivers/src/embedded.rs:596 | The embedded Postgres socket directory under the temp directory is used without checking its owner or mode | Low | fixed |
| SEC-OUT-09 | crates/moqui-host/src/pdf.rs:684 | Child processes inherit the server's whole environment | Low | fixed |
| SEC-OUT-10 | crates/moqui-core/src/service/mail_capture.rs:86 | Mail capture has no test-run gate | Low | fixed |
| SEC-OUT-11 | crates/moqui-llm/src/provider/mod.rs:1669 | The LLM endpoint check reads only an address written in the URL | Low | fixed |
| SEC-OUT-12 | crates/moqui-lib/src/net.rs:42 | `internal_address` leaves out some non-public ranges; proxy variables may take names past the guard | Low | fixed |
| SEC-OUT-13 | systemd/moqui.service:23 | The systemd unit and the image leave the binary and confs writable by the service user | Low | fixed |
| SEC-OUT-14 | moq:1476 | The default release build carries `devtools`; `moq` downloads Chromium and add-on zips with no checksum | Info | fixed |

### Development tools

Map file: [Attack-Surface-Dev.md](../docs/security/Attack-Surface-Dev.md). Reviewer report: `security-review-evidence/slice-22.md` (and `slice-22b.md` where the slice was continued).

| ID | Location | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| SEC-DEV-01 | runtime/components/dev/dev-aide/src/entities.rs:98-131 | export#Records returns any entity's records to every Dev AIDE tier, including the stored tokens get#Credentials keeps for ADMIN | Medium | fixed |
| SEC-DEV-05 | crates/moqui-core/src/service/mail_capture.rs:176-232 | MailCapture.enable() has no test-run gate, unlike HttpFixtures | Medium | fixed |
| SEC-DEV-06 | runtime/components/dev/demo/data/DemoSecurityData.xml:21-59 | The demo lock-down leaves many ADMIN grants and permissions in place that were added after it | Medium | fixed |
| SEC-DEV-02 | runtime/components/dev/dev-aide/src/services.rs:87-117 and 58-82 | eval#Condition runs a caller-supplied expression at the view tier | Low | fixed |
| SEC-DEV-03 | runtime/components/dev/dev-aide/screen/DevAide/Workspace.xml (transition `restart`, last transition of the file) | State-changing Dev AIDE transitions have no method attribute, so a parameterless GET runs them | Low | fixed |
| SEC-DEV-04 | runtime/components/dev/dev-aide/src/confine.rs:28 | The file deny list covers six runtime names; conf, save, sftp and install are readable at the view tier, and store#Root accepts any other host directory | Low | fixed |
| SEC-DEV-07 | runtime/components/dev/dev-aide/src/services.rs:338-447 | The extend tier is code execution as the server, wider than its description | Info | reported (confirmed in code) |
| SEC-DEV-08 | moq:5206-5208 | A demo archive needs --with-dev, which ships Dev AIDE beside the demo component | Info | reported (confirmed in code) |

## Critical and High findings

Each as its reviewer wrote it. "Not confirmed" is what the reviewer did not trace; "Re-check" is the second reading, where there was one.

#### SEC-WEB-01. An account disabled for good comes back by itself: four failed logins and a wait, or a password change

High; confirmed (lead); fixed.

- **Fix:** Fixed 2026-11-15: a failed login never gives an account disabled for good a lock time (`write_failed_increment`, `increment#UserAccountFailedLogins`), and a password change ends only a tarpit lock (`update_password_fields`, `update#PasswordInternal`, whose `disabled` has no default now). Tests: `user/login_tests.rs`, `framework/test/ClosedAccounts.spec.js`.
- **Where:** `crates/moqui-core/src/user/login.rs:1303-1310`, `:1511-1520`, `:1736-1746`; `runtime/components/base/framework/service/org/moqui/impl/UserServices.xml:150-164`, `:166-283`, `:290`, `:382`; `runtime/components/base/webroot/screen/webroot/Login.xml:105-114`
- **Reached by:** anonymous, `POST /Login/login` (or any request with Basic credentials or the account's login key) four times, then a sign-in five minutes later with the account's own password or key; or anonymous `POST /Login/changePassword` or `/ChangePassword/changePassword` with the account's `username`, `oldPassword` and a new password.
- **What happens:** `disable#UserAccount` leaves `disabled = Y` and `disabledDateTime` null, which the login gate, the per-request session refresh and the map (old line 51) treat as closed for good. Route 1: each attempt on the disabled account is counted; the fourth sets `disabledDateTime = now`, which turns the permanent state into the five-minute lock, and after the window `still_disabled` is false, so the right password or key signs in and `finish_login` writes `disabled = N`. Route 2: `update#Password` never reads `disabled`, and `update#PasswordInternal` writes `disabled` from its parameter, whose default is `N`; `Login/changePassword` then signs the user in. `reset#Password` does not read `disabled` either, so the reset password works as `oldPassword` for route 2.
- **Evidence:** `login.rs:1305` `if still_disabled(account, now) { ... increment_failed(facade, &user_id)?;`; `login.rs:1742` `let when = if disable { now_value() } else { row_value(row, "disabledDateTime") };`; `login.rs:1515-1519` `let Some(disabled_at) = field_millis(account, "disabledDateTime") else { return true; }; ... disabled_at.saturating_add(window_ms) > now`; `login.rs:1389-1392` `if failed_or_disabled(account) { ... fields.push(("disabled", Value::string("N")));`
- **Evidence:** `UserServices.xml:290` `<parameter name="disabled" default-value="N"/>`, `:382` `<field-map field-name="disabled" from="disabled"/>`, `:278-279` `update#PasswordInternal ... in-map="{userId: userId, newPassword: newPassword, newPasswordVerify: newPasswordVerify}"`; `login.rs:1258` `("disabled", Value::string("N")),` in `update_password_fields`
- **Not confirmed:** not run. Whether an application service that closes an account also sets `terminateDate` (which neither route undoes) was not traced; `revoke#AccountAccess` "leaves the account enabled" by the map. `Security.eecas.xml` has no rule on `UserAccount`.
- **Suggested fix:** Never write `disabledDateTime` on an account that is disabled with none (in `write_failed_increment` and `increment#UserAccountFailedLogins`), or keep the administrative disable in its own field. Make `update#Password` and `reset#Password` refuse a disabled or terminated account, and stop `update#PasswordInternal` and `update_password_fields` from writing `disabled` unless the caller asked.
- **Re-check:** Read `gate_account`, `still_disabled` and the failed-login update in `crates/moqui-core/src/user/login.rs`: an account with `disabled=Y` and no `disabledDateTime` gets `disabledDateTime` set once the count passes `max-failures`, and `still_disabled` then lets it through after `disable-minutes`. Read `update#PasswordInternal` in `UserServices.xml`: `disabled` defaults to `N` and is written. looked for a guard on the permanent state in `gate_account`, `increment_failed`, `update#Password`, `update#PasswordInternal`, `reset#Password` and `internal_login`; none. `terminateDate` is the one state neither route undoes (`terminated` in `gate_account`). `reset#Password` does not read `disabled` either, so a mailed reset password works as `oldPassword` for route 2. Not read: which application services set `terminateDate` when they close an account.

#### SEC-RTE-01. `GET /email/{id}` lets anyone move mail to Viewed

High; reported (likely); fixed.

- **Fix:** Fixed 2026-11-15: the tracking image's path is the message's view key, `{id}.{tag}`, the tag an HMAC of the id under a key derived from `session_cookie_key` (`web::email_view_key`, `emailViewKey` in the mail render context, mr-marble's `EmailLayout.html.fancy`); a bare id or a wrong tag changes nothing, and only an `ES_SENT` message becomes `ES_VIEWED`. Test: `web/tests.rs` `view_email_message_sends_the_png_and_marks_a_sent_message_by_its_key`.
- **Where:** `crates/moqui-core/src/web/mod.rs:1306` (`mark_email_message_viewed`); `runtime/components/base/webroot/screen/webroot.xml:277`
- **Reached by:** anonymous, `GET /email/<emailMessageId>` (or `<id>.png`), one request per id.
- **What happens:** The id is the only check, and `emailMessageId` is a plain `type="id"` key (a sequenced id). Any `EmailMessage` not already `ES_VIEWED` is updated to `ES_VIEWED` with `receivedDate` set to now, with authorization disabled. The entity-auto status check allows it from `ES_READY` (the Outbox), `ES_SENT` and `ES_RECEIVED`, so a caller who walks the id range takes queued mail out of `ES_READY` and rewrites the status and received date of received mail.
- **Evidence:** `crates/moqui-core/src/web/mod.rs:1322` `.eq("emailMessageId", Value::string(&id)).disable_authz().one()?;` and `:1334` `("statusId".to_string(), Value::string("ES_VIEWED")), ("receivedDate".to_string(), crate::user::now_effective_value()),` and `:1341` `disable_authz: true,`
- **Evidence:** `runtime/components/base/framework/entity/BasicEntities.xml:670` `<moqui.basic.StatusFlowTransition statusFlowId="Default" statusId="ES_READY" toStatusId="ES_VIEWED" transitionName="View"/>` (and `:673` from `ES_SENT`, `:675` from `ES_RECEIVED`)
- **Evidence:** `runtime/components/base/framework/entity/BasicEntities.xml:538` `<field name="emailMessageId" type="id" is-pk="true"/>`
- **Not confirmed:** that ids are in fact sequential in every create path; whether the mail client still sends a message that left `ES_READY` this way (the send code in `crates/moqui-mail` was not traced); whether a rate limit of web protection covers `/email`.
- **Suggested fix:** Put a keyed MAC of the id in the pixel URL and refuse without it, and apply the change only from `ES_SENT` without touching `receivedDate` of other statuses.

#### SEC-ADM-01. A disabled account re-enables itself with a password change

High; confirmed (lead); fixed.

- **Fix:** Fixed 2026-11-15: `update#Password` refuses an account disabled for good or past `terminateDate` unless the caller has `ADMIN_PASSWORD`, `reset#Password` makes no reset password for one, and the password path writes `disabled` only to end a tarpit lock. Tests: `framework/test/ClosedAccounts.spec.js`, `user/login_tests.rs`.
- **Where:** `UserServices.xml:166-282` (`update#Password`), `:284-385` (`update#PasswordInternal`)
- **Reached by:** anonymous, `POST /ChangePassword/changePassword` or `/Login/changePassword` with the account's `username`, its current (or unexpired reset) password and a new password; a second factor code too when the account needs one.
- **What happens:** `update#Password` never reads `disabled` or `terminateDate`. It calls `update#PasswordInternal` with three parameters, whose `disabled` parameter defaults to `N` and is written to the account. An account disabled for good (`disable#UserAccount`: `disabled` Y, no `disabledDateTime`, the state the map says ends sessions and refuses logins) becomes enabled, and `/Login/changePassword` then signs it in. `reset#Password` does not read `disabled` either, so the reset mail is sent for a disabled account and its reset password works as the old one. `terminateDate` still refuses the login.
- **Evidence:** `UserServices.xml:278-279` `<service-call name="org.moqui.impl.UserServices.update#PasswordInternal" out-map="context" in-map="{userId: userId, newPassword: newPassword, newPasswordVerify: newPasswordVerify}"/>`; `:290` `<parameter name="disabled" default-value="N"/>`; `:382` `<field-map field-name="disabled" from="disabled"/>`; `:396-402` `disable#UserAccount` writes `{userId: userId, disabled: 'Y', disabledDateTime: null}`; `base/webroot/screen/webroot/Login.xml:110-111` `<if condition="updateSuccessful"><set field="loginSuccess" from="ec.user.internalLoginUser(username)"/></if>`
- **Not confirmed:** not run. Whether Mantle's offboarding sets `terminateDate` as well as `disabled` was not read (it would still stop the login).
- **Suggested fix:** In `update#Password` refuse an account that is disabled for good or past `terminateDate` unless the caller has `ADMIN_PASSWORD`; do not pass or default `disabled` from the password path (write it only in `create#UserAccount`).
- **Re-check:** Same reading as SEC-WEB-01 (`UserServices.xml` `update#PasswordInternal`: `disabled` defaults to `N` and is written).

#### SEC-TLS-01. Export's `filterMap` runs a caller's script, by GET, with no script permission

High; confirmed (lead); fixed.

- **Fix:** Fixed 2026-11-15: `filterMap` is parsed with `JSON.parse`, never evaluated, and `EntityExport`, `exportDocuments` and `AutoEditMaster` `export` are no longer `read-only`, so they take a POST with the session token and refuse URL parameters. Tests: `tools/test/EntityExport.spec.js`, `framework/test/client/SecurityScreens.spec.js`.
- **Where:** `runtime/components/base/tools/screen/Tools/Data/Export.xml:18` and `:54-61`; `crates/moqui-core/src/script/oxide/resource.rs:246-272`
- **Reached by:** a user with update on the Tools app (`ADMIN` by seed, whether or not in `ADMIN_ADV`) who requests, or is sent to, `GET /apps/tools/Data/Export/EntityExport?entityNames=<any entity>&filterMap=<script>`. A link on another site is enough: the request is a top-level GET, the cookie is `SameSite=Lax`, and no token is asked.
- **What happens:** `filterMap` is passed to `ec.resource.expression`, which compiles and runs it in the running engine with the transition's own `ec`. The script can call `ec.artifactExecution.disableAuthz()` and then any service or entity operation, and `ec.resource.runTokenCommand('<program> <args>')`, which starts a process on the server. The sealed `GROOVY_SHELL_WEB` permission, which ETL asks for an expression and which no seed group holds, is not asked here. By the guide's click rule this is Medium; it is rated High because it is also a step for an `ADMIN` outside `ADMIN_ADV` from the Tools grant to script and process execution on the host.
- **Evidence:** `Export.xml:18` `<transition name="EntityExport" begin-transaction="false" read-only="true" authz-action="update">`; `:56` `const filter = ec.resource.expression('(' + filterText + ')', null)`
- **Evidence:** `oxide/resource.rs:247-252` `fn expression(...) { let expression = text_arg(scope, 0)?; ... with_additional(scope, true, |scope| evaluate(scope, &expression))`; `:270-272` `run_here(scope, &source, hash, super::compiled::FILE_NAME, Some(expression))` ("compiled through the runtime's cache and executed while the engine is lent, so its names are the caller's", `:68-69`)
- **Evidence:** `crates/moqui-core/src/screen/render.rs:2076` `if transition.read_only || !runs_actions(transition) { return None; }`; `:2668-2673` GET is not token-checked
- **Evidence:** `crates/moqui-core/src/script/host/tools.rs:721`, `:2615` (`runTokenCommand` → `Command::new(program).args(words)`); `crates/moqui-tools/src/etl/access.rs:167` `has_permission("GROOVY_SHELL_WEB")` (the check ETL makes and Export does not)
- **Not confirmed:** not run against a server. No `Sec-Fetch-Site` or Origin check was found for GET (`grep -i sec-fetch` in `moqui-core/src` and `moqui-host/src`: none); a reverse proxy rule would be outside the code. The widget wrapper of `ec` (`oxide/widget.rs`) applies only while the server-static check is on (`runtime.rs:75`, off by default). The demo replaces this screen (`demo/MoquiConf.xml`).
- **Suggested fix:** Parse `filterMap` with `JSON.parse` (or a field=value list) instead of evaluating it. Make the three Export transitions and `AutoEditMaster` `export` `method="post"` and not `read-only`.
- **Re-check:** Read `Export.xml` (the `EntityExport` transition is `read-only`; `filterMap` goes to `ec.resource.expression`), `expression` in `script/oxide/resource.rs` (evaluated in the caller's names), `run_token_command` in `script/host/tools.rs` (starts a process), and `render.rs:290-293` (the effect fence is entered only for an agent's or a write-fenced render, not a browser request).

#### SEC-TLS-02. Hosted-instance rows give host command execution and script execution to a Tools and System holder

High; reported (likely); fixed.

- **Fix:** Fixed 2026-11-15: every `InstanceServices` service asks the new sealed permission `INSTANCE_ADMIN` (`ADMIN_ADV`), a host type's or database type's service must be one of `org.moqui.dev.InstanceServices`, and the `moqui.server.instance` configuration entities that hold a command, a script or a service name are restricted for the generic tools, REST and request loads (`sensitive.rs` `is_host_config_entity`). Test: `tools/test/HostedInstances.spec.js`, `sensitive.rs`.
- **Where:** `runtime/components/base/framework/service/org/moqui/dev/InstanceServices.xml:444-447`, `:195`, `:206`, `:295`; `crates/moqui-core/src/script/host/tools.rs:2602-2621`
- **Reached by:** an `ADMIN` (update on Tools and System, no sealed permission). Step one: write `moqui.server.instance.InstanceImage.authTokenCmd` (or `InstanceImageTypeEnv.envValue`, `InstanceHostType.initService`) through Tools › Data › Record or Browse (`update#${entity}`), a data load or entity REST. Step two: System › Instance, create an `AppInstance` on that image and press Init (`initAppInstance`).
- **What happens:** `init#InstanceDocker` runs the text of `authTokenCmd` as a process on the server, before any Docker call is made. `create#AppInstance` expands `envValue` and `volumeName` as `${}` templates, which are script. `init#AppInstance` calls whatever service `initService` names, on a worker. The sealed permissions that guard the other ways to a host process or a script (`TERMINAL_HOST`, `SSH_SHELL`, `GROOVY_SHELL_WEB`, `COMPONENT_DEPLOY`, `CONF_EDIT`) are not asked on this path, and the same Docker API reach lets the holder start a container with any bind mount on whatever Docker host a row names.
- **Evidence:** `InstanceServices.xml:444-447` `if (instanceImage.authTokenCmd) { try { authTokenPass = ec.resource.runTokenCommand(instanceImage.authTokenCmd)`
- **Evidence:** `InstanceServices.xml:195` `<set field="envValue" from="ec.resource.expand(imageTypeEnv.envValue, '')"/>`; `crates/moqui-core/src/script/oxide/resource.rs:236-238` `let (literal, hash) = crate::script::runtime::template_source(&text); match run_here(scope, &literal, hash, ...)`
- **Evidence:** `crates/moqui-entity/src/entity/sensitive.rs:115-117` `is_restricted_generic_entity` is identity-admin, secret-config and audit entities only; the `moqui.server.instance` entities are in none of those lists
- **Evidence:** `runtime/components/base/tools/screen/System/Instance/InstanceList.xml:32` `<transition name="initAppInstance"><service-call name="org.moqui.dev.InstanceServices.init#AppInstance"/>`
- **Not confirmed:** the write of `InstanceImage` through `update#moqui.server.instance.InstanceImage` was not exercised; an entity ECA or a privileged-entity rule that refuses it was looked for only by grep of `sensitive::` callers. Whether the template form `${...}` of `expand` reaches the full `ec` was read in `oxide/resource.rs` only. The demo's VIEW-only grant refuses both steps.
- **Suggested fix:** Ask a sealed permission in every `InstanceServices` service that starts a process, a container or a database (as `LoadServices` asks `LOAD_RUNNER`), and add the `moqui.server.instance` entities to the generic tools' restricted list. Remove `authTokenCmd`, or take the command from the conf instead of a row.

#### SEC-GIT-01. `import#GitRepository` reads another repository or a server directory with no check of the source

High; reported (confirmed in code); fixed.

- **Fix:** Fixed 2026-11-15: `import#GitRepository` checks its `source` for the caller (`service/mod.rs` `import_source_allowed`): another repository needs the view right on it, a directory or another scheme `TERMINAL_HOST` or authorization disabled, and a URL is https (http with `webhook-private-network`) with no credential, to public addresses only, whoever calls. mr-boss `create#Repository` takes only an http(s) `importUrl`. Test: `service/import_source_tests.rs`.
- **Where:** `crates/moqui-git/src/lib.rs:1470-1530`; `crates/moqui-git/src/service/mod.rs:320-327`; caller `runtime/components/addon/mr-boss/service/ironstone/projects/CodeServices.xml:241-244`
- **Reached by:** a user who manages a project in mr-boss: `ironstone.projects.CodeServices.create#Repository` checks `get#ProjectAccess need: 'manage'` (not ADMIN) when `workEffortId` is given, then passes the caller's `importUrl` on. Also any other caller of `import#GitRepository`.
- **What happens:** `source` is taken as `git://<otherRepositoryId>`, a directory on the server, or a URL for gix. The default branch of the source is copied into the caller's `db` repository. No view right on the other repository is asked, no path rule on the directory, no scheme or address rule on the URL. A project manager can copy another project's private repository into their own, or any git repository directory the server user reads; an internal URL is fetched.
- **Evidence:**  - `lib.rs:1484-1492` `if let Some(other) = source.strip_prefix("git://") { … let other = self.definition(other)?; … Box::new(db::DbStore::of(self.entity()?, &other, self.objects()))` - `lib.rs:1495` `} else if Path::new(&source).is_dir() {` - `service/mod.rs:324-327` `let Some((provider, id)) = provider_and_id(parameters, true) else {…}; match provider.import(&id, text(parameters, "source").as_deref())` - `CodeServices.xml:244` `ec.service.sync().name('org.moqui.git.GitServices.import#GitRepository').parameters({repositoryId, source: String(importUrl).trim()}).disableAuthz().call()`
- **Not confirmed:** whether `mantle.work.GitServices.create#ProjectRepository` or a screen rule before it limits who reaches line 241. Whether a gitECA pre rule of source `import` refuses. gix's handling of `ssh://` and `file://` was not read. The mr-boss `RepositoryDetail/Settings.xml` import transition is ADMIN-only (line 170), in the screen, not in the service.
- **Suggested fix:** in `import`, require view (`AT_GIT_REPOSITORY`) of a `git://` source for the caller, and take a directory or a non-https URL only with authorization disabled. Run URL sources through the address rules `fetch#GitRefs` uses.

#### SEC-GIT-02. A registered runner gives its runner token to every step

High; reported (confirmed in code); fixed.

- **Fix:** Fixed 2026-11-15: a registered runner's steps start from a cleared environment (`PATH`, `HOME`, `LANG` and the like, the `LC_` variables, `--pass-env` names, then the job's own), no runner, pool or job token is passed in any mode (`runner::step_base_env`), and `poll` reads its token from `--token-file` or `MOQUI_CI_RUNNER_TOKEN_FILE` (the variable still works, with a warning). On 2026-11-16: a runner token expires (`git-ci` `runner-token-days`, 90) and the runner rotates it halfway (`POST /ci/runner/rotate`, the token file rewritten); a pool's `allowedAddresses` refuses a register, claim or rotate from another address; and `poll --step-user USER` (a root runner) runs each step as another user, who owns the checkout but cannot read the token file. Tests: `ci/runner/tests.rs`, `server/ci_tests.rs` `a_runner_token_expires_rotates_and_comes_only_from_the_pools_addresses`, `ci_run.rs` `a_rotated_token_replaces_the_token_file`.
- **Where:** `crates/moqui-host/src/ci_run.rs:134`, `:151`; `crates/moqui-git/src/ci/runner/mod.rs:332-344`; `crates/moqui-git/src/ci/registered.rs:206-322`; `crates/moqui-git/src/ci/runner_http.rs:253`
- **Reached by:** any account whose push or merge request runs a job in a pool of registered runners: with no `run-check-service`, an account with the update right on any repository that names the pool; a step of its `ci.xml` reads the variable.
- **What happens:** `moqui ci-run poll` reads `MOQUI_CI_RUNNER_TOKEN` from its environment and runs each step with that environment; only `MOQUI_CI_TOKEN` is taken out. A step sends the runner token elsewhere. The token has no expiry. With it, `POST /ci/runner/claim` from any address takes the next queued job of the pool whenever the runner is idle, and the job token it answers reads that job's repository at its commit and, through `GET jobs/<id>/job`, its secrets. A developer who pushes to an unprotected branch so gets the secrets scoped to protected branches and environments, and those of the other repositories the pool serves. The documentation says nothing isolates a job from the machine; it does not say the runner's token is in a step's environment.
- **Evidence:**  - `ci_run.rs:134` `let runner_token = env("MOQUI_CI_RUNNER_TOKEN")?;` - `runner/mod.rs:332` `let mut spawn = Command::new(&options.shell);` … `:341` `.env_remove("MOQUI_CI_TOKEN");` (no `env_clear`, no other `env_remove`) - `registered.rs:238` `if busy > 0 {` (a runner that is not running a job is given the next one) - `runner_http.rs:253` `let secrets: … = trust::secrets_for(entity, run, &spec, true)?` - `docs/interfaces/Pipelines.md:334-335` "nothing isolates a job from the machine: use such a pool for trusted runs, of one owner" (a doc statement)
- **Not confirmed:** I did not run it. The same user can also read the runner's environment through `/proc/<pid>/environ` and leave files for the next job (a changed `git` on the path, a shell profile), so removing the variable alone is not enough.
- **Suggested fix:** clear the runner's own environment for a step (pass only what the job defines), keep the runner token out of the environment (a file only the runner's user reads, with steps run as another user), and give runner tokens an expiry and a bound source.

#### SEC-MAIL-01. Any mail user can take the inbound mail of an address nobody has claimed

High; reported (confirmed in code); fixed.

- **Fix:** Fixed 2026-11-15: making a hosted or shared account with no store server, giving one another address, or making an account one needs the new permission `EMAIL_ACCOUNT_HOSTED` (`ADMIN`) or authorization disabled (`create#EmailAccount`, `update#EmailAccount`). The own-address exception was left out: a user changes their own `UserAccount.emailAddress` with no verification. Test: `basic-comms/test/HostedMailAddress.spec.js`.
- **Where:** `runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml` `create#EmailAccount` (207-306), `update#EmailAccount` (308-452); `runtime/components/base/basic-comms/service/org/moqui/comms/MailSettingsServices.xml` (the `gateway` branch near 717); `crates/moqui-mail/src/delivery.rs:73-135, 310-360`
- **Reached by:** any signed-in member of `BASIC_COMMS_USERS`, through the MailAccounts wizard (`Settings/MailAccounts/Add`, kind "Hosted here" or "Shared, delivered here"), on a server whose inbound listener or edge node takes mail for the domain.
- **What happens:** The user makes a hosted or shared account with any `emailAddress` and no server; nothing checks the domain, the address or a permission. From then on `RCPT TO` for that address (and its plus-tagged forms) resolves to the user's account and the mail is stored in their Inbox. An address no account has yet (`postmaster@`, `abuse@`, `hr@`, a colleague not yet set up, an application's intake or `bounces@` address) is theirs to read; `update#EmailAccount` also changes `emailAddress` with only the manage check on the account. An address that already has an older account stays with it (lowest id wins), so this takes unclaimed addresses only.
- **Evidence:** `const KINDS = ['EakHosted', 'EakExternal', 'EakShared']; if (!KINDS.includes(accountKindEnumId)) {...}` then `account.setFields({ownerUserId: ownerUserId, ... accountKindEnumId: accountKindEnumId, ... emailAddress: emailAddress, ...})` with no other check of `emailAddress`; `delivery.rs:108-110` `KINDS.contains(&kind) && status != "EasRemoved" && !has_store_server`; `delivery.rs:327-329` `ids.sort_by(|left, right| made_order(left, right)); Ok(first_claim(ids, address, "its address"))`; `docs/interfaces/Mail-Inbound.md:51` `The accounts are made as usual (create#EmailAccount with accountKindEnumId EakHosted or EakShared, the BOSS MailAccounts wizard).` (doc)
- **Not confirmed:** whether an app (mr-boss) replaces the wizard with one that limits kinds or domains; the edge node's `RecipientMap` is said by its comment to answer "by the same rules" and was not traced; `create#EmailIdentity` (an identity's address is matched second) was not read; `mail_dkim_domains` empty signs for "the hosted and shared accounts' domains" (a comment), so a claimed domain may also be signed for, not traced.
- **Suggested fix:** Let only a holder of a mail-administration permission make or re-address an account the listener delivers to, or require the address's domain to be in a configured list of hosted domains and the local part to be approved.

#### SEC-MAIL-61. A request's fileName runs as a server-side expression through emailScreenAsync

High; reported (likely); fixed.

- **Fix:** Fixed 2026-11-15: an attachment's file name from a call is taken as written; only an `EmailTemplateAttachment` row's is expanded (`implementation.rs` `ScreenAttachment.expand_name`). `emailScreenAsync` and `emailScreenUserAsync` pass a fixed set of fields, not the context, and are no longer `read-only`. Test: `implementation_tests.rs` `screen_body_renders_html_and_text_and_sends_with_attachments`.
- **Where:** runtime/components/base/webroot/template/screen/BasicTransitions.xml:20-22; runtime/components/base/framework/service/org/moqui/mail/EmailServices.xml:242-264; crates/moqui-mail/src/implementation.rs:1059, 1206, 1265; crates/moqui-core/src/script/runtime.rs:401-410
- **Reached by:** any signed-in user authorized for a screen that includes `emailScreenAsync` (five mr-marble accounting screens: AssetOnHand, AssetOnHandSummary, AssetStatusByReceipt, ShipmentPackageSummary, FindTransactionEntry), with `GET .../emailScreenAsync?toAddresses=a@b.c&screenRenderMode=csv&fileName=${...}`; also a cross-site link such a user opens, since the transition is `read-only` and a GET is not token-checked.
- **What happens:** Every request parameter is a context name and the transition passes the whole context to `send#ScreenRenderEmail`, which declares `fileName` and puts it in the attachment map it hands to `send#EmailTemplate`. `send_template` expands the attachment's file name with `moqui_core::script::expand`, which runs each `${expression}` in the script runtime, so text the request wrote is evaluated as JavaScript on the server with `ec` in scope, in an async service call.
- **Evidence:** `in-map="Object.assign({}, context, {screenLocation: sri.getActiveScreenDef().location, bodyParameters: ec.web.requestParameters})"`; `<set field="attachments" from="[{fileName: fileName, attachmentLocation: screenLocation, screenRenderMode: screenRenderMode}]"/>`; `let filename = expand_text(&map_text(map, "fileName"))?;` and `let filename = expand_text(&filename)?;`; `/// Expand a template. \`${expression}\` runs in this runtime.`; render.rs:821-826 `for (name, value) in job.parameters { ... script::set_context_name(name, value.clone())?; }`
- **Not confirmed:** not run. Whether `Object.assign` over `context` copies the request's names (the form's own `toAddresses`, `title` and `screenRenderMode` travel the same way); whether service parameter validation changes a `${` in a plain text parameter (a grep of `crates/moqui-core/src/service` for a `${` check found only `remote.rs`); what the script runtime lets an expression do (the Scripts slice); which of the two expand sites the map attachment reaches.
- **Suggested fix:** Never expand a caller's `fileName`: expand only an `EmailTemplateAttachment` row's own name, and treat a call's as literal text. Give the transition a fixed parameter list (`toAddresses`, `screenRenderMode`, `title`) instead of the whole context.

#### SEC-MAIL-63. mr-marble PartyEmails serves any EmailMessage body as inline text/html

High; reported (confirmed in code); fixed.

- **Fix:** Fixed 2026-11-15: the transition finds the message by the party's users and addresses, as the list does, and sends the body with `Content-Security-Policy: sandbox` and `nosniff`. Test: `mr-marble/test/PartyEmailBody.spec.js`.
- **Where:** runtime/components/addon/mr-marble/screen/SimpleScreens/Party/PartyEmails.xml:23-26; crates/moqui-core/src/web/mod.rs:1253-1277
- **Reached by:** any signed-in user authorized for the mr-marble Party screens, `GET .../PartyEmails/EmailMessage?partyId=x&emailMessageId=N` (ids are sequenced); for the script part, anyone who can mail any mailbox on the server and get such a user to open the link.
- **What happens:** The transition finds the message by the request's id alone, with no party or account check, and answers its `body`. That is every user's mailbox mail and system mail: a password reset mail's stored body holds the reset password in clear (C65), so a party-screen user can ask for an administrator's reset anonymously and read it. The answer is `text/html` with `Content-Disposition: inline` on the application's origin, so HTML mail from outside (stored as received, not cleaned on the server) runs its script in the victim's session.
- **Evidence:** `<transition name="EmailMessage" read-only="true"><parameter name="emailMessageId"/><actions>` `<entity-find-one entity-name="moqui.basic.email.EmailMessage" value-field="emailMessage"/>` `<script>ec.web.sendTextResponse(emailMessage?.body, "text/html", null)</script>`; mod.rs `None => headers.push(("Content-Disposition".to_string(), "inline".to_string()))`, `send_bytes_with_headers(200, headers, text.as_bytes().to_vec())`
- **Not confirmed:** whether a response-wide content security policy or `nosniff` header is added in `send_bytes_with_headers` (not read); whether the entity find is refused for a non-admin by an artifact authorization on `moqui.basic.email.EmailMessage` (the screen's own list reads the same entity, and `moqui.*` entities have no row filter); who is granted the SimpleScreens Party screens.
- **Suggested fix:** Remove the transition, or find the message with the screen's own party condition and show it through the `html-view` widget (sandboxed frame, cleaned) instead of as a document.

#### SEC-SCR-01. Data Export `filterMap` runs any server script, on a GET, without the script permission

High; reported (confirmed in code); fixed.

- **Fix:** Fixed 2026-11-15 with SEC-TLS-01 (the same finding).
- **Where:** runtime/components/base/tools/screen/Tools/Data/Export.xml:18 and :56
- **Reached by:** any user of the Tools app (the `ADMIN` group by default), with `GET .../Tools/Data/Export/EntityExport?entityNames=<a defined entity>&filterMap=<script>`; also a third-party page that makes a signed-in admin's browser follow that link.
- **What happens:** The `filterMap` parameter is evaluated with `ec.resource.expression`, which runs it as a server script with `ec` in scope: `ec.resource.runTokenCommand` (a host process), `System.getProperty` (secrets), `ec.artifactExecution.disableAuthz()`, `ec.service.rest()`. The transition is `read-only="true"` with no `method`, so it runs on a GET with URL parameters and no session token, and the cookie is `SameSite=Lax`. The sealed `GROOVY_SHELL_WEB` permission, which ETL expressions require, is not checked here.
- **Evidence:** `<transition name="EntityExport" begin-transaction="false" read-only="true" authz-action="update">` ... `const filter = ec.resource.expression('(' + filterText + ')', null)`
- **Not confirmed:** Nothing between lines 18 and 56 checks a permission; a parent screen of `Tools/Data` with a stricter check than the `TOOLS_APP` grant was not looked for; the effect fence is not entered for a normal request (enter sites listed in the map).
- **Suggested fix:** Parse `filterMap` as JSON, not as an expression (or require `GROOVY_SHELL_WEB`), and give the transition `method="post"`.

#### SEC-SCR-05. Notification titles are expanded as script, and three callers build them from record values

High; reported (likely); fixed.

- **Fix:** Fixed 2026-11-15: the Smithy deployment and push mirror notices pass a fixed title template and put the values in the message; a caller's `link()` is a URL taken as written, no longer expanded (`notify/message.rs`), which also covers the wiki review, chat, event and record links built from caller values. `DataDocument.documentTitle` (`PartyServices.xml`, and search hits) is a configured template, as in Java; on 2026-11-16 writing one with `${`, or a `DataDocumentLink` `linkUrl` or `label` with `${` or a `linkCondition`, came to need `DATABASE_ADMIN`, as a field expression does (`Security.eecas.xml` `DataDocumentTitleTemplate`, `DataDocumentLinkTemplate`). Tests: `notify/tests.rs` `a_callers_link_is_not_expanded`, `SmithyDeployments.spec.js`, `DocumentExpression.spec.js`.
- **Where:** crates/moqui-core/src/notify/message.rs:322 and :899-911; runtime/components/addon/mr-mantle/service/mantle/smithy/SmithyPipelines.js:1279; mantle/smithy/SmithyImport.js:2052; mantle/party/PartyServices.xml:1609
- **Reached by:** a user who can set a deployment environment's name, a repository path, a push mirror URL or a data document's title (a repository maintainer or lower), then waits for the notification to be sent.
- **What happens:** The text given to `.title()` is a template: `${...}` in it runs on the script runtime when the message is sent. These callers concatenate record values into it, so a value such as an environment name holding `${...}` runs as a server script in the sending service's context. Other callers (`Voice.js`, `PurchaseApprovals.js`) pass a fixed template and say why in a comment.
- **Evidence:** `.title("The deployment of " + where + " " + text(deployment.commitId).slice(0, 12) + " to " + environmentName + " waits for approval")` and `self.title_text = Some(expand_template(title_template.as_deref().unwrap_or(""), &message)?);`
- **Not confirmed:** Whether the environment name, repository path and mirror URL are validated to a character set without `$` or `{` where they are stored; who may create a `GitCiEnvironment`.
- **Suggested fix:** Pass a fixed template with `${names}` and put the values in the message map in these three callers; consider a separate `titleText()` that is never expanded.

#### SEC-GIT-81. A registered runner reads a job's report or artifact path through a symbolic link, as root with `--step-user`, and the server serves the file to the run's readers

High; confirmed (lead).

- **Where:** `crates/moqui-git/src/ci/runner/mod.rs:282-322`, `:712-728`, `:747-753`; `crates/moqui-git/src/ci/runner/http.rs:127`; `crates/moqui-git/src/ci/runner/archive.rs:39-43`; `crates/moqui-git/src/ci/runner_http.rs:329-362`; `crates/moqui-git/src/ci/artifact.rs:248-291`; `crates/moqui-git/src/ci/service/mod.rs:148-163`, `:711-731`, `:813-826`; `moqui.rest.xml:340-342`. Reports: round 3 `A15.md` (the server's side, to the readers) and `A4.md` (the runner's side).
- **Reached by:** the author of a merge request from a fork or pushed for review (with `review-push`, any account that views the repository), once the run is approved or the application's run check allows it; also any developer whose push runs on the pool. The pool is one of registered runners started with `--step-user`, the mode the engine requires for untrusted runs (the reach of SEC-GIT-02). Where the runner runs its steps as its own user the step already reads what the runner reads, and this adds nothing.
- **What happens:** the `ci.xml` of the commit names a report or artifact path. A step, running as the step user, makes that path a link to a file only root reads (the runner's token file, `/etc/shadow`, a root SSH key), or makes a directory of the path a link (`d -> /root` with the pattern `d/.ssh/id_ed25519`). After the steps the runner process itself, which is root in this mode, collects reports and artifacts: `matches` tests each fixed segment with `symlink_metadata`, which looks through every link but the last, and a `*` segment lists a directory through a link; a report is kept when `path.is_file()`, which follows a link, and is uploaded with `File::open`, which follows it too. `pack` sets `follow_symlinks(false)`, which stores only a final link as a link: a path through a linked directory is read as a file, and `path.is_dir()` follows a final link to a directory. The server stores the body as a content blob with a `GitRunArtifact` row and no knowledge of where it came from. `get#RunArtifact` (REST `GET git/{repositoryId}/artifacts/{artifactId}`) and `send#RunArtifact` then give it to anyone who views the repository, and for a hidden run to `triggeredByUserId`, who is the attacker. With the runner token the attacker claims the pool's next jobs (`POST /ci/runner/claim`), and a trusted job's `GET jobs/<id>/job` answers its secrets. This undoes the separation that SEC-GIT-02's fix rests on (the runner's token in a file only the runner's user reads).
- **Evidence:** `ci/runner/mod.rs:296-298` `// a link is not followed out of the workspace` / `if path.symlink_metadata().is_ok() {` / `next.push(path);` (the check only says the name exists; a link passes)
- **Evidence:** `ci/runner/mod.rs:714-716` `for file in matches(&options.workspace, pattern)` / `.into_iter()` / `.filter(|path| path.is_file())`
- **Evidence:** `ci/runner/http.rs:127` `let opened = std::fs::File::open(file).map_err(|err| format!("{}: {err}", file.display()))?;`
- **Evidence:** `ci/runner/mod.rs:472` `spawn.uid(user.uid).gid(user.gid).env("USER", &user.name).env("LOGNAME", &user.name);` (set on the step only; the upload is the runner's own process)
- **Evidence:** `ci/runner/archive.rs:39-42` `let added = if path.is_dir() {` / `builder.append_dir_all(relative, path)` / `} else {` / `builder.append_path_with_name(path, relative)`
- **Evidence:** `ci/artifact.rs:249` `let info = match blobs.put(input, &Expected::default(), room) {` and `:275` `row.set("name", Value::string(upload.name));`
- **Evidence:** `ci/service/mod.rs:155` `if !may(&repository, "view") {` and `:161-162` `(authz_disabled() && include_hidden)` / `|| (current_user().is_some() && current_user() == field(run, "triggeredByUserId"))`
- **Evidence:** `ci/runner_http.rs:254` `let secrets: serde_json::Map<String, serde_json::Value> = trust::secrets_for(entity, run, &spec, true)?`
- **Not confirmed:** not run. Confirmed in code for a report; likely for an artifact: `tar::Builder::append_dir_all` with `follow_symlinks(false)` walking a root that is a link was reasoned from the crate's API, not read. Whether a pool's `allowedAddresses` is set in a given deployment, which would stop the stolen runner token from another address. Looked for and not found: a check in `matches` that the resolved path stays under the workspace (`ci/file.rs:233-239` `valid_path` refuses `..`, `.git` and an absolute path in the pattern text only), `O_NOFOLLOW` or a `symlink_metadata` test before the open (a search of `canonicalize`, `NOFOLLOW`, `is_symlink` in `ci/runner/*.rs` and `crates/moqui-host/src/ci_run.rs` finds only the `symlink_metadata` above), the upload being made as the step user, a narrower reader set for an untrusted run's reports.
- **Suggested fix:** in the runner, resolve each matched path (`canonicalize`) and keep it only when it is under the canonical workspace and is a regular file by `symlink_metadata`; open with `O_NOFOLLOW`; refuse a link as a report; with `--step-user`, read and pack reports, artifacts and caches as the step user (a child process with that uid), never as root. Correct the comment at `mod.rs:296`.
- **Re-check:** Two checks by the lead. For `A4.md`: read `ci/runner/mod.rs:288-299` (a pattern segment is kept when `symlink_metadata()` succeeds, so a link is kept, not refused; the comment says otherwise), `:712-717` (`path.is_file()` follows the link), `ci/runner/http.rs:125-127` (`File::open` follows it), `archive.rs:34-43` (`follow_symlinks(false)` holds for a directory walk; a named file is appended by path). For `A15.md`: read `ci/artifact.rs:248-249` (the upload is stored as a blob as sent) and `ci/service/mod.rs:152-162` (`readable_with`: anyone with `view` on the repository reads a run that is not hidden). Confirmed for a report. Holds only in `--step-user` mode, where the runner is root and steps are another user. Not read: the artifact path, the token file's location, the step user setup.

#### SEC-RTE-81. A posted `_isMulti=true` with `<name>_0` replaces an id a screen's always-actions pinned; on mr-marble's My Account one user changes another account's email address

High; likely (lead).

- **Where:** `crates/moqui-core/src/screen/definition.rs:396-406`; `runtime/components/base/framework/template/XmlActions.js:306-307`; `crates/moqui-core/src/script/host/service.rs:446-456`; `crates/moqui-core/src/service/call.rs:255-280`; `runtime/components/addon/mr-marble/screen/MyAccount/User/Account.xml:19-25`; `runtime/components/addon/mr-mantle/service/mantle/party/PartyServices.xml:771-803`; `runtime/components/base/framework/service/org/moqui/impl/UserServices.xml:146-150`, `:659-670`. Reports: round 3 `A2.md` (the weakness), `B2.md` (the same mechanism, and the shortcut with an explicit in-map).
- **Reached by:** any signed-in member of `MY_ACCOUNT_USERS` (the `MY_ACCOUNT` artifact group is an inheritable ALLOW on `MyAccount.xml`, `mr-marble/data/SimpleScreensSetupData.xml:33-34`), by a POST with their own session token to the transition `MyAccount/User/Account/updateUser` with `_isMulti=true&userId_0=<victim>&emailAddress_0=<the attacker's>`. The mechanism reaches every transition written as a `service-call` shortcut whose in-map is the whole context (`in-map` absent, `true` or `context`) under a screen that pins an id in always-actions or a `parameter from`.
- **What happens:** The screen pins the account with `<set field="userId" from="ec.user.userId"/>` in always-actions, and its transition calls `mantle.party.PartyServices.update#Account` with `in-map="context"`. On a plain post the pin holds: the request parameters are put in the context first and the always-actions write over them. But the shortcut's `multi` defaults to `parameter`, which makes the call a multi call when the request carries `_isMulti=true`. A multi call reads the whole context, which holds every request parameter, builds each row from the `<name>_<N>` entries, and takes the unsuffixed context value only for a name the row lacks. So `update#Account` runs with the victim's `userId`. It checks no owner and passes its context to `update#UserAccount`, whose `check#AccountAction` runs with `permissionId: 'NONE'`: another user's account is allowed unless it holds access the caller lacks, after the caller's own sign-in in the last five minutes, which the attacker has just made. The victim's `emailAddress` (where `reset#Password` mails), `username`, `disabled`, `requirePasswordChange` and the other sign-in fields are then the attacker's to set, and a password reset by email takes the account. An account in a privileged group or holding a sealed permission is refused by `heldBeyond`.
- **Evidence:** `mr-marble/screen/MyAccount/User/Account.xml:21` `<set field="userId" from="ec.user.userId"/>` and `:24` `<transition name="updateUser"><service-call name="mantle.party.PartyServices.update#Account" in-map="context"/>`
- **Evidence:** `screen/definition.rs:404` `if call.attribute("multi").is_none_or(|text| text.is_empty()) && !call.attribute_is("async", "true") {` and `:405` `call.set_attribute("multi", "parameter");`
- **Evidence:** `framework/template/XmlActions.js:306-307` ``const flagsExpr = attr(node, "multi") === "parameter" ? `(${flags} | (ec.web && ec.web.parameters && ec.web.parameters._isMulti === 'true' ? 2 : 0))` ``
- **Evidence:** `screen/render.rs:856-860` `for (name, value) in job.parameters { ... script::set_context_name(name, value.clone())?;` (every request parameter, `userId_0` included, is a context name); `script/host/service.rs:300-302` `if multi { return Ok(None); }` then `:450` `let mut visible = call.ops.whole_context()?;` (a multi call reads the whole context, not the declared names)
- **Evidence:** `service/call.rs:258-259` `if let Some(value) = parameters.get(&format!("{parameter}_{index}")) { row.insert(parameter.clone(), value.clone());` and `:271-275` `if row.get(parameter).is_some_and(|value| !is_empty(value)) { continue; } if let Some(value) = parameters.get(parameter).filter(|value| !is_empty(value)) { row.insert(...)` (the pinned value only fills a gap)
- **Evidence:** `mr-mantle/service/mantle/party/PartyServices.xml:775` `<parameter name="userId"/>`, `:779` `<parameter name="disabled"/>`, `:782-783` `emailAddress`, `username`; `:801` `<if condition="userId"><service-call name="org.moqui.impl.UserServices.update#UserAccount" in-map="context"/></if>` (no check of the caller before it)
- **Evidence:** `UserServices.xml:147-148` `<service-call name="org.moqui.impl.UserServices.check#AccountAction" in-map="{userId: userId, permissionId: 'NONE', recentAuth: recoveryChanged}" out-map="accountActionOut"/>`; `:663` `} else if (permissionId !== 'NONE' && !ec.user.hasPermission(permissionId)) {` (skipped for `NONE`); `:669` `else allowed = !recentAuth || !!ec.service.sync().name('org.moqui.sso.RecentAuthServices.require#RecentAuth').call()?.recent`
- **Not confirmed:** not run. The reviewer did not read `Identity.js` `heldBeyond`, `require#RecentAuth` or the session token gate, who is put in `MY_ACCOUNT_USERS`, or `reset#Password` end to end. Looked for and not found: a test in the transition run that the target form is a multi form (`render.rs:2100-2135`); a check in `sync_multi` that the unsuffixed context value wins (it is the reverse); an owner check in `update#Account` or a permission in `update#UserAccount` (`NONE`); an explicit `multi` on the transition. Not affected: mr-boss `Commons/Settings/Account.xml:71`, which passes an explicit map (`in-map="{userId:ec.user.userId, ...}"`): with an explicit in-map the rows are read from it alone, none is found and the service is not called (`call.rs:262-264`, `B2.md`). The factor transitions of the mr-marble screen (`Account.xml:29-45`) reach the same override, but each factor service checks `check#AccountAction` with `ADMIN_IDENTITY_MANAGE` (SEC-ADM-04), and `update#Password` still asks the old password. A REST service call is never a multi call (`rest.rs:613`, `call.rs:185-187`; `B1.md`). The units that searched their own screens found no other affected transition: `A10.md` (chat bridges and texts), `A15.md` (git and CI), `B2.md` and `B3.md` (administration), `B4.md` and `B5.md` (Tools); the other screens of mr-boss, mr-mantle, mr-pass and mr-marble were not surveyed.
- **Suggested fix:** In the framework, make a multi call only for a transition a multi `form-list` of the screen posts to (or only with an explicit `multi="parameter"`), and in `sync_multi` do not let a row value replace a name the caller set outside the request. In mr-marble, pass an explicit in-map with `userId: ec.user.userId` and `partyId`, as mr-boss does. In Mantle, give `update#Account` an owner or permission check of its own.
- **Re-check:** Read `screen/definition.rs:396-407` (the shortcut `service-call` gets `in-map="true"` and `multi="parameter"` by default), `XmlActions.js:306-311` (`multi="parameter"` turns the multi flag on when `ec.web.parameters._isMulti === 'true'`; in-map `true` or `context` is `ec.context`), `service/call.rs:254-272` (each row takes `<parameter>_<index>` first and falls back to the plain name only when the row has none), `mr-marble/screen/MyAccount/User/Account.xml:19-25` and `PartyServices.xml:790-801` (`update#Account` passes its context to `update#UserAccount`). The guard: `UserServices.xml:136-150` and `check#AccountAction` `:656-671`: with `permissionId: 'NONE'` another user's account is allowed unless `heldBeyond` (it holds access the caller lacks), and an email change asks only the caller's own recent sign-in. Not read by the lead: that `parameters` in `call.rs` holds the request's `userId_0` when the in-map is the context; `Identity.js` `heldBeyond`.

#### SEC-SCR-81. A wiki page whose type is `fancy`, `html.fancy`, `md.fancy` or `markdown.fancy` runs as server script for every viewer

High; confirmed (lead).

- **Where:** `runtime/components/base/framework/service/org/moqui/resource/WikiServices.xml:205`, `:592`, `:716-721`, `:1812`; `runtime/components/addon/mr-marble/screen/SimpleScreens/Wiki/wiki.xml:228-230`, `:267-268`; `runtime/components/addon/mr-marble/screen/SimpleScreens/Wiki/ViewWikiPage.xml:50-51`; `runtime/components/base/framework/template/screen/ScreenSupport.js:320`; `crates/moqui-core/src/screen/sri/sections.rs:738-769`; `crates/moqui-core/src/script/template.rs:105-113`, `:345-355`, `:452-461`; `runtime/components/base/framework/conf/MoquiDefaultConf.xml:1034-1035`, `:1043-1044`, `:1048`; `crates/moqui-core/src/context/factory.rs:1421-1426`. Reports: round 3 `A4.md` (found it) and `B9.md` (settled the path from the stored file to the live renderer, and what a template reaches).
- **Reached by:** a signed-in user who may update a page of a wiki space: `get#WikiPageAccess` gives `canUpdate` to any user who can view when neither the space nor the page has `restrictUpdate` Y. Through the Marble wiki's Add Page form and the `updatePage` transition of `EditWikiPage.xml:22`, which calls `org.moqui.resource.WikiServices.update#WikiPage` with the request's `wikiType` and `pageText`. The script runs in the request of whoever opens the page next, an administrator among them.
- **What happens:** `update#WikiPage` takes `wikiType` as a free parameter (default `md`) with no list of allowed values, and a new page's file is `<dir>/<name>.<wikiType>`; an existing page keeps its type. The only cleaning is of an `html` page in a space without `allowAnyHtml`. The Add Page form offers every registered template extension as the type. The wiki screens render the page with `<text type="html,vuet,qvt" template="true" location="${pageLocation}">`; the screen macro emits `sri.renderText($location, "true")`, which calls `render_location_default`, and `renderer_for` takes the longest registered extension the file name ends with, so the stored name alone decides. Four extensions map to the live `FancyRenderer`: `fancy` (no escaping), `html.fancy` (HTML escaping of `${}`), and `md.fancy` and `markdown.fancy` (markdown first, then the live renderer). It compiles the page and runs it on the thread's script runtime with the live context, `ec` and `sri`, which makes it a server script: `${ec.artifactExecution.disableAuthz()}` works from a template (`script_disable_authz` checks no caller), and after it entity finds, `ec.entity.runSqlStatement`, `ec.service.sync()`, `ec.service.rest()`, the resource read and write functions (any `file://` path the process can reach) and the server administration functions are open; only `runAsSystem` is refused, as it needs a service's own code. Because `<text>` writes the result without encoding, the page is stored HTML for the viewer as well. The same gap gives stored HTML without script: a page of type `md` keeps raw HTML (the markdown converter passes it through and `update#WikiPage` does not clean it), and a type with no renderer (`HTML`, `htm`, `txt`) is written to the page as its text, past the space's `allowAnyHtml` rule.
- **Evidence:** `WikiServices.xml:592` `<parameter name="wikiType" default-value="md"/>`
- **Evidence:** `WikiServices.xml:205` ``pageReference = found || ec.resource.getLocationReference(`${dirLocation}/${name}${wikiType ? '.' + wikiType : ''}`)``
- **Evidence:** `WikiServices.xml:720` `<if condition="wikiSpace.allowAnyHtml != 'Y' &amp;&amp; wikiType == 'html'">`
- **Evidence:** `WikiServices.xml:1812` `canUpdate = canView && !readOnly && (canAdmin || ((space.restrictUpdate !== 'Y' || spaceUser?.allowUpdate === 'Y') &&`
- **Evidence:** `wiki.xml:229` `<list-options list="ec.resource.getTemplateRendererExtensionSet()"/>`; `:267` `<widgets><render-mode><text type="html,vuet,qvt" template="true"` with `:268` `location="${pageLocation||''}..."`
- **Evidence:** `ScreenSupport.js:320` ``const call = `sri.${encode ? "renderTextValue" : "renderText"}($location, ${jsString(template)})`;``; `sections.rs:768` `registry.render_location_default(&state.factory, location, None, template)`
- **Evidence:** `template.rs:458` `.filter(|(extension, _)| name.len() > extension.len() && name.ends_with(&format!(".{extension}")))`
- **Evidence:** `MoquiDefaultConf.xml:1034` `<template-renderer extension="fancy" class="fancy"/>`, `:1035` `<template-renderer extension="html.fancy" class="fancy-html"/>`, `:1043` `<template-renderer extension="md.fancy" class="markdown-fancy"/>`, `:1044` `<template-renderer extension="markdown.fancy" class="markdown-fancy"/>`
- **Evidence:** `template.rs:349` `"fancy" => Arc::new(FancyRenderer { escape: Escape::None }),`, `:350` `"fancy-html" => Arc::new(FancyRenderer { escape: Escape::Html }),`; `:107` `let compiled = compiled(source, self.escape, Scope::Live)?;` and `:112` `super::oxide::templates::run_live(&compiled)`
- **Evidence:** `context/factory.rs:1421-1426` `pub fn script_disable_authz(&self) -> bool {` ... `*self.state.script_authz.borrow_mut() = Some(self.disable_authz());` (no check of the caller)
- **Evidence:** `crates/moqui-lib/src/markdown.rs:18` `//! is CommonMark plus GitHub tables, and raw HTML is passed through as Java does: a markdown file is`
- **Not confirmed:** not run, and the form post was not exercised: confirmed in code from the stored file to the live renderer, likely for the write. Which user groups reach the Marble wiki screens in a default installation (their screen authorization was not read); whether the REST or PASS wiki paths render a page through a template renderer (unit B1 read the REST wiki services as reads only); the other screens and services that render a wiki page (the Knowledge and site page services, which pass a fixed `md` or `html`). Looked for and not found: a list of allowed `wikiType` values in `update#WikiPage`, `get#WikiPageFile` and `EditWikiPage.xml`; a check in `render_location_default` or `render_text_value` of who wrote the file or of its scheme (only `.ftl` is refused); a locked mode for a `<text>` location; a `Security.eecas.xml` or resource-facade rule on a `.fancy` name; a fence around a template run.
- **Suggested fix:** Take `wikiType` only from a fixed list (`md`, `html`, and no extension with `fancy`) in `update#WikiPage`, `get#WikiPageFile`, `create#WikiSpace` and the other space services, refuse `/`, `\` and `..` in it, and offer only those types in the Add Page form. Render a wiki page with a renderer chosen from that list, not by the stored file name: markdown for `md`, cleaned HTML for `html`, text for anything else, and never the live renderer for a location under a wiki space's root. Clean the HTML the markdown gives (`StringUtilities.cleanHtml`) unless the space has `allowAnyHtml`.
- **Re-check:** Two checks by the lead. For `A4.md` (likely): read `WikiServices.xml:592` (`wikiType` a free parameter, default `md`), `:716-721` (only an `html` page is cleaned; an existing page keeps its type), `wiki.xml:228-230` (the Add Page form offers `ec.resource.getTemplateRendererExtensionSet()` as the type) and `:267-268` (the page is rendered with `template="true"` from `pageLocation`), `script/template.rs:107-117` (`FancyRenderer` runs with the live context, `ec` and `sri`). With `B9.md` (confirmed): read `MoquiDefaultConf.xml:1034-1035`, `:1043-1044` (`fancy` to class `fancy`, `html.fancy` to `fancy-html`, `md.fancy` and `markdown.fancy` to `markdown-fancy`, "a trusted fancy template"), `script/template.rs:343-353` (`fancy` and `fancy-html` are `FancyRenderer`, the live one) and `:451-461` (`renderer_for` takes the longest registered extension the location's file name ends with, so the stored name alone decides); `update#WikiPage` (`WikiServices.xml:607-640`) checks `pageName` and nothing of `wikiType`. Not read by the lead: the fancy compiler, and who may edit in a space (the space editor check).
