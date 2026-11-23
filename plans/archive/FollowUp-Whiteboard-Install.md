# Follow-up: whiteboards, presentation stage and follow, and the per-user install

> Archived 2026-11-22 after phases 1–7 and the basic-comms acceptance checks passed.

This covers a review of the commits that brought [Comms-Whiteboard](../Comms-Whiteboard.md)'s boards, meeting boards, ink
and markup, the presentation stage and follow mode, and [Install-DevCA](../Install-DevCA.md)'s per-user install, shared
`~/.mr-moqui` definitions, development certificates and separate listener ports. The review found the core models
sound:
- **Boards:** guest and member grants checked on the server, all four `board@*` limits enforced, SVG shown only
  through `<img>`.
- **Stage:** presenting authorized on the server, with races between presenters fenced.
- **Follow:** a follower opens the relayed path and params under their own session.

It also found bugs, scaling risks, missing upgrade paths, failure-tolerance gaps, missing tests, stale security docs
and missing es/zh rows. The phase table records the original work. The verification record below covers the follow-up.

Decisions:
- **Shared `~/.mr-moqui`:** with `--installed` it is the runtime, used alone (production and desktop installs). Without
  it, the runtime directory on disk wins, then the binary, and `~/.mr-moqui` supplies only files neither has (the dev
  CA, a shared add-on).
- **Stage:** links keep the "Anyone can present" default, and a non-host may never take the stage from a host.
- **Follow:** members follow automatically; a member with a changed form or a dialog of their own is paused with
  "Follow again" instead of being navigated.

## Built

| Phase | What | Notes |
|---|---|---|
| 1 | Meeting-board frames answered from the room's in-memory grant (`moqui_collab::meeting_allows`), not a `get#MeetingBoardGrant` per frame | `may_deliver` judges by the frame's client, else any membership of the socket. Meeting grants are asked again every 10 s, and at the next tick after a `collab-recheck` nudge (`org.moqui.resource.CollabServices.recheck#MeetingBoards`). The nudge is published from CallBoards open/close/drawing, `Calls.closeLeg`, `endSession`, `revokeGuest`, denied admission, `removeParticipant` and chat `remove#Member` |
| 2 | Stage: no non-host takeover of a host; `change()` re-reads after `bump` and publishes once; a bad `stageState` read as empty; a rebound page gives up its screen stage | `bump()` returns whether it published |
| 2 | Follow: presenter and member gate before the rate bucket; no `as_user`; per-socket resolution cache; a `locate` op for pointer, section and dialog; `followRate` code; guests' follow frames refused; follow needs a local identity; guests marked for screen video at every join; dirty-form and dialog pause | Client and server path rules share `webroot/test/client/pages/follow-vectors.json`. Chat's `chatRoomId` moved to its read-only `room` transition, and `follow::resolve` accepts read-only transition parameters |
| 2 | SFU: a follow stage's members reserve no screen bandwidth; a presenter's undeclared tracks default to camera and mic | |
| 3 | Layer order without `--installed`: disk, binary, `~/.mr-moqui`; `--installed` alone and first-argument only; boot logs naming where runtime files come from | Run-Deploy "Where runtime files come from" |
| 3 | Install: `.install-manifest` with prune and a conflict list, `.install-version` with a warning on mismatch, `--with-data` refused while the source runs and without instance and edge identity files, links skipped with a warning | `.env` and cert keys are still copied (the seal keys read the copied data); a warning says so |
| 4 | Renamed gRPC attributes refused, naming the replacement, in conf files and env forms (`RETIRED_ATTRIBUTES` in `conf_env.rs`) | XSD docs, Run-Deploy rows and upgrade note, pre-production skill step |
| 5 | Dev certs: tests isolated from the real shared signer (`ensure_dev_cert_with`); `moqui dev-cert --replace` and `--rotate`; 2-year intermediate; checkout signer deleted after promotion; a daily leaf renewal check; `ca.crt` trusted only when owned by the user and not writable by others; trust.sh NSS paths, rotation cleanup and untrust by name | `http://localhost:8080` leftovers fixed in docs, the component template and the Python fixtures; `-p` documented |
| 6 | Boards: tolerant seed (a bad element is skipped and logged); Board.qvue fallback on an unreadable scene; compaction when a room closes with nothing unsaved, with saved tombstones; `board@max-total-image-bytes` and pruning on `save#Board`; collab update and awareness rate limits (`socket-updates-per-minute`, `socket-awareness-per-minute`); `delete#WikiPageRows` taking a page's board rows; Excalidraw locale codes; meeting version note by name; nits | Pruning drops images from older versions' previews |
| 7 | `WebrootTabCapture` moved to the webroot acceptance client suite; mr-boss and mr-marble READMEs point at `https://localhost:8443` | |

## Verification, 2026-11-22

- The owner opens the configured board once on an empty stage. A guest or a member with view-only board access can arrive before the owner. Reopening the link preserves an occupied stage.
- Revoking a link ends its guests’ board and image grants. The owner can create another link through the existing workflow. Follow relays a declared, permitted `userName`; a member can receive that value even if their own session cannot open the destination.
- The security, Comms, recording, client, revision and deployment guides now describe the shipped behavior. The 174 additions to each of the Spanish and Simplified Chinese catalogs cover the board, stage and follow UI and socket messages, enum descriptions and the affected tools and Gallery screens. Error codes and silent follow refusals retain their existing behavior.
- On macOS 27.2.1 (arm64), the real user trust tools installed and rotated generated roots in a disposable keychain. With the login-keychain lookup returning no preference, the fallback located the same disposable keychain. Repeated removal succeeded with and without the root file. The unrelated certificate remained and the test keychain was removed. On Debian 13.7 (Linux aarch64), five real NSS SQL databases passed the same checks, including the SSL trust flag. They covered both Chromium user paths, snap Chromium, Flatpak Chrome and a Firefox profile selected by an absolute path in profiles.ini.
- The initial webroot run took 55.6 seconds of feature time. After moving Mermaid conversion to acceptance, a quiet release run still took 52.2 seconds. The keyboard regressions now wait for actual selections and submissions and read completed key events directly. The full offline suite then passed all 548 features in 47.3 seconds of feature time. The final framework run measured **47.2 seconds against the 48-second budget** (69.68 seconds wall time). Board merges and compacted deletions remain in regression. Grid’s date checks use the existing offline mount.
- The live RTC scenario exposed a lock during call-end saves. The save timestamp now uses the same transaction as the collaboration checkpoint. The rebuilt RTC-enabled MeetingBoards scenario executed and passed. Ending the call saved the guest’s rectangle under the host with the meeting and guest names in its note. The service regression passed both features, including the clean-board revision count, image grants before and after revocation and issuing a replacement link. Both Rust lints and screen parsing passed. The focused SFU track test passed. Webroot acceptance passed all seven features after the terminal walk was made independent of the host shell’s prompt and menu focus.
- The first basic-comms acceptance run covered 77 features. MeetingBoards was among the 36 passes. Seven failed and 34 were skipped. The encrypted-device verification passed twice on isolated working-tree runs (7/7 each). The other six failures reproduced using the committed framework sources at `d679c4ae3`.
- All eight groups of `./moq test-framework --release --nobuild` passed. The `./moq test-apps --release --nobuild` run passed 4,961 service and 48 client features, plus 31 proofs with three skipped. `cargo lint`, `cargo lint-min`, and screen lint passed (2,417 screens, zero parse failures). Both catalogs passed checks for matching keys, uniqueness, length, unchanged entries and placeholders. All 461 relative file links in the eleven edited Markdown files resolved. The language-pack commit is `4155868`. The Rust workspace passed 6,488 tests with 67 ignored, using one Rust test thread on macOS. The redirect test now supplies its own session-cookie fixture and passes in isolation.

## Closeout, 2026-11-22

The full RTC-enabled basic-comms acceptance suite passed in 117.13 seconds. Across 13 specs, 43 features passed, including MeetingBoards and all six chat checks that kept this review open. The harness skipped 34 network-shaping cases that require Linux network namespaces and `tc netem`.

| Spec | What the passing walk verified |
|---|---|
| ChatLabels (phone and desktop) | A configured test device supplies setup and per-room trust. Both widths show the verified lock and open Bob’s device list from the other room’s Verify button. |
| PhoneChat (attachments and Back navigation) | The menu offers Camera, Gallery, File and Location. Back closes the menu, sheet, thread and room in order. The test shell loads the expiry-label script used by the shipped shell. |
| ChatDetails (phone) | The same expiry-label script lets the room details render. The walk checks every details tab, swipes the media viewer and closes the pane. |
| ChatPalette | Mod+K, two letters and Enter open the room created for the walk. The resulting route, room ID and server-returned name all match. |

The encrypted-chat walk also passed all seven features on its own and in the full suite. It uses dedicated accounts so devices registered by earlier call scenarios cannot change which room members have verified devices.

All eight framework regression groups passed on the closeout run with one Rust test thread. The earlier two-thread run exhausted a PostgreSQL shared-memory slot and timed out in an XMPP test. Both tests passed twice in isolation and again in the serial suite. Webroot passed 548/548 in 47.2 seconds of feature time against its 48-second budget (69.79 seconds wall time). The service runner reported timing advisories for framework (30.0 seconds against 29) and basic-comms (9.4 seconds against 9).

[Comms-Whiteboard](../Comms-Whiteboard.md) and [Install-DevCA](../Install-DevCA.md) continue to track their own work.

## Opus 5.5 medium Cost Calibration

This plan costs:
- review and agent plan completed with overage at $1.91 (meter reading at start)
- cost of the commits included is from agent plan to completed tests, including writing this file (FollowUp-Whiteboard-Install.md)
- a smaller amount of work in main agent, 3 subagents running ~30m, ~37m, ~38m (the last with 306k tokens reported for bg agent); total time after all tests around 1 hour
- meter reading at end following lint, test-framework, test-apps: $46.60
- cost of the following commits with token pricing instead of plan pricing is 46.60 - 1.91 = $44.69

Overall costs:
- most of the original work being reviewed and adjusted in this plan was done by Astra 6.2 medium (not including the l10n follow ups, most had been done before this but a few remained) for ~46% of 1 week of $100/mo plan (so for around $11 at plan pricing, not token pricing)
- bulk of work done so far has been with Opus 5.5, with a fair amount over the last 2 weeks on Fable 5.2 because of the free Fable week reset; using one personal and one work Claude $200/mo accounts, plus ChatGPT $100/mo, plus Grok $30/mo; 6 weeks in on 20 Nov 2026, not all plans active the whole time, so high estimate of cost from plans completed to date is 1.5 months by $530/mo so ~$795; this is also a high cost because I've worked on a few side projects here and there as well on these plans (grok-the-rock, home-scale, ghigs-tdg), but compared to the mr-moqui, mr-mantle, mr-boss, mr-pass, mr-marble, mr-l10n, mr-home, and mr-three repos the changes in those are quite small; this is low because each Claude $200/mo account has had 1 free full week reset and 1 free Fable week reset, and the ChatGPT $100/mo account has had something like 3 week resets (though GPT models, even Astra, are inferior to Claude 5.5 medium for writing code and even thoroughly maintaining docs; GPT most useful so far for planning/design, testing, and reviews... with each vetted by Opus or Fable to fill in technical gaps and details, fix issues found, etc)

This plan commits so far:
```
== .  master -> origin/master [ahead 12, behind 0]
  unpushed:
    d425fcc71 Record the follow-up to the whiteboard, stage and follow, and per-user install commits: what is built and what remains.
    e83f07978 Merge branch 'worktree-agent-a4779338cbb1137ee'
    9b42a3955 Merge branch 'worktree-agent-ae38cc2c2c528617c'
    fa5f4468b Keep a host's stage from a non-host, publish each stage change once, and make follow cheap, member-only and safe for unsaved work.
    9552aefdd Leave a follow stage's cameras the bandwidth of the screen that only guests get, and label a follow presenter's undeclared tracks camera and mic.
    81f8e95a1 Keep boards open past one bad piece and bounded as they grow: a tolerant seed, compaction on close, a total image limit with pruning, per-socket collab rate limits, and page deletion that takes a board's rows with it.
    e29766ee1 Merge branch 'worktree-agent-adb9f94bc578760a8'
    2f0295f61 Move the tab capture media test to the webroot acceptance client suite.
    4911dacd8 Dev certificates: isolate tests, replace and rotate, trust only own roots.
    ff4e29182 Refuse the renamed gRPC listener attributes, naming their replacements.
    df964d23a Read ~/.mr-moqui only for files the runtime and binary lack, and prune installs.
    237ed8218 Answer a meeting board's frames from the room's grant in memory, and ask the grant again when the call changes, instead of calling get#MeetingBoardGrant for every update, awareness and outgoing frame.
  clean
== runtime/components/addon/mr-boss  master -> origin/master [ahead 1, behind 0]
  unpushed:
    14305d1f Point the quick start at the dev server's HTTPS address.
  clean
== runtime/components/addon/mr-home  master -> origin/master
  clean
== runtime/components/addon/mr-l10n  master -> origin/master
  clean
== runtime/components/addon/mr-mantle  master -> origin/master
  clean
== runtime/components/addon/mr-marble  master -> origin/master [ahead 1, behind 0]
  unpushed:
    c719ffc Point the quick start at the dev server's HTTPS address.
  clean
== runtime/components/addon/mr-pass  master -> origin/master
  clean
== runtime/components/addon/mr-three  master -> origin/master
  clean
```

## Token pricing and plan pricing

Recorded 2026-11-22 from the meter above. The nine framework commits in this plan, plus the two README lines in mr-boss and mr-marble, are 3,737 changed lines (lines added plus lines deleted). The meter moved $44.69 for that work, including the three subagents and the lint and test runs. That is **$12 per thousand changed lines** of Opus 5.5 medium overage.

The same count on the MR work those plans paid for, David E. Jones's non-merge commits from 9 October through 22 November 2026 in mr-moqui, mr-boss, mr-mantle, mr-marble, mr-pass, mr-l10n, mr-home, and mr-three:

- 6,501 commits, **4.41 million changed lines**, after leaving out lockfiles, `webroot/libs`, minified assets, and `patches/` (the yrs fork was added and removed the same week).
- At $12 per thousand lines that is about **$53,000**.
- The plan stack in the notes above is about **$800** at the high estimate (1.5 months × $530). A plan dollar bought about **66 token dollars**.
- Over the 44 days that is about **100,000 changed lines a day**. The heavy week of 26 October–1 November was 1.03 million lines, about **150,000 a day**.

Lines that remain in the trees are about 3.2 million. Priced the way this follow-up's net lines price ($44.69 for 2,885 net lines) that is about $49,000. The two cuts agree. XML and Markdown are a larger share of the 4.41 million than of this follow-up, which was mostly Rust. If those cost half as much per line as the Rust fix pass, the total would sit closer to $40,000.

Side projects on the same plans (home-scale, ghigs-tdg) are about 30,000 changed lines, a few hundred dollars at this rate. The free week resets are already in the git history, so they are already in the $53,000. They are not extra cash on top of the $800.

The original boards, stage, follow, and per-user install implementation is 25,100 changed lines in eleven commits, the Astra work noted above at about $11 of the $100 plan. At this Opus meter rate those lines are about $300, roughly 27 times the plan allocation for that slice. The six-week multiple is higher because the bulk of the work was Opus.

## Manual effort and the same prices

The same count on David E. Jones's non-merge commits in the Moqui Java repositories under `../moqui/` (the framework repo, runtime, and the nineteen component repos, through 23 October 2026). Minified files and vendored webroot libraries are left out (about 226,000 lines, mostly `assets/lib`).

- 9,369 commits, **741,000 changed lines**, from 8 October 2011 through 23 October 2026.
- About 144,000 of those lines are the 23 January 2016 imports ("initial files from moqui/moqui" and "from moqui/mantle"). PopCommerce and HiveMind keep their history back to 2011 and 2012. The framework, runtime, and Mantle before the split appear only as the files that existed that day.
- At the same $12 per thousand lines that is about **$8,900** of token pricing and about **$130** at the 66× plan discount.
- 2017, the busiest full year after the split, is 106,000 changed lines, about **290 a day**.
- The trees hold about 358,000 non-vendor lines today. The diffs are about twice that.

An allowance for framework and Mantle edits before the 2016 split, which these clones do not contain, is on the order of 50,000–150,000 lines if those years looked like 2017–2020. That adds roughly $600–$1,800 at the token rate. The Java total stays around $10,000 token and $150 plan.

### Peak spans

Changed lines are lines added plus lines deleted in commits that landed. Net lines are added minus deleted, the lines that stayed. Import commits are left out of the peaks. A span is any 7 or 30 consecutive days, not a calendar week or month.

The best sustained week through 2025 is **29 August–4 September 2016**: 17,000 changed lines, **2,500 a day**, net +400. 67 commits, every day, and the busiest day is a quarter of the week. It is the conversion of the entity and service runtime from Groovy to Java (EntityValueBase, EntityFindBuilder, ExecutionContextImpl, ServiceDefinition, ServiceCallSyncImpl). The net is small because the Java replaced the Groovy.

A week with a higher raw total, **4–10 January 2016**, is 23,000 changed lines (3,200 a day) and a net of +5,600, on 5 of the 7 days. That week moves screens out of HiveMind into SimpleScreens. The heaviest 30 days are **17 August–15 September 2016**: 33,000 changed lines (1,100 a day) and a net of about zero, the same Groovy-to-Java conversion.

The busiest stretch that kept adding application code on nearly every day is **14 January–12 February 2017**: 18,000 changed lines (**590 a day**), net +6,700 (**225 a day**), 333 commits on 29 of the 30 days. Order, receiving, payment-method, calendar, and shipment screens in SimpleScreens, and the Mantle services beside them.

| | Changed lines per day |
|---|---:|
| 2017, the whole year | 290 |
| Best sustained month of application work (14 Jan–12 Feb 2017) | 590 |
| Best sustained week through 2025 (29 Aug–4 Sep 2016) | 2,500 |
| MR average, 9 Oct–22 Nov 2026 | 100,000 |
| MR heavy week, 26 Oct–1 Nov 2026 | 150,000 |

### The 29 August–4 September 2016 week

67 commits in the framework, the runtime, and mantle-usl. The framework's entity and service runtime moved from Groovy to Java, and the commits around that are the rest of the week.

EntityValueBase and EntityFindBuilder were converted on 31 August, ServiceCallImpl on 2 September, and ServiceDefinition and ServiceCallSyncImpl on 3 September. ExecutionContextImpl was converted the same day as ServiceCallImpl, and the internal paths went over to direct final fields. RelationshipInfo and FieldInfo were split out. A new EntityInfo class holds the parsed definition, and the CrUD path uses it. View-entity finds reject unknown fields. Data loads call EntityAutoServiceRunner directly. In-parameter checks got faster, and a parameter can again depend on one later in the list. MessageFacadeImpl no longer lets a caller change the error lists from outside. A ThreadLocal leak on destroy was fixed, and a few tight loops moved to Java. The XML-RPC client was removed.

Java 8 `java.time` replaced Joda Time, in the framework and in mantle-usl, and the runtime uptime display followed. Groovy is pulled in as selected modules. The Gradle wrapper went to 3.0, and the OWASP HTML sanitizer and Shiro were updated.

REST schema generation for JSON Schema, RAML, and Swagger was gathered into `RestSchemaUtil`. DbResource directory creation locks and checks for a record that is already there. Artifact-hit timing had been recording microseconds as milliseconds. An IMAP BODYSTRUCTURE workaround went in for issue #97.

Forms gained print width (zero means leave the field off), alignment, and a text button. The CSV button defaults to off, and a `saveFilename` parameter sets Content-Disposition. `display-entity` can supply a default text. A user can subscribe to every message on a notification topic, and the account screen in the runtime has that dialog.

Screen output in the runtime: print CSS and macros, CSV export through links and container bodies with a CSV button on the form list, and a fixed-width pad and truncate. The text macros were lined up with the CSV.

### The 14 January–12 February 2017 month

331 commits in the framework, the runtime, SimpleScreens, mantle-udm, mantle-usl, HiveMind, PopCommerce, and a library bump in moqui-fop. The new screens are in SimpleScreens. Mantle holds the services and the model. PopCommerce and HiveMind include the same screens.

**Wiki and content.** The wiki screens moved from HiveMind into SimpleScreens: spaces, edit, history, a jsdiff compare, and a published version. DbResource files gained versions, a version suffix on a resource location, and a published version name. A wiki space can allow any HTML. Product content gained list, edit, and compare screens, and the content services save text onto the content record. PopCommerce shows the long description, streams the product images, and has a content root for the store's about and contact pages.

**Orders through shipment.** An order item records a quantity change reason on the audit log. Return screens landed, and receiving landed on the shipment. A receipt creates a lot or uses an existing one. Lots show on assets, reservations, and shipments, and a reservation can require a single lot or newer inventory. Packing can put an item in a package and change its quantity. The pick document and the by-package document follow the lots. Placing an order warns when the ship-to or the shipment method is missing. Promotions are `ProductStorePromotion` records with parameter entities, and a buy-get discount service. Order updates were reworked so tax and shipping recalculate across more kinds of change. A first sales-tax service and per-store ship options landed. An open order can be cancelled.

**Parties.** Duplicate finding matches an identification, an email, a phone, a postal address, a name, and a birth date. The party screens cover the calendar, messages, related parties, time entries, classifications, settings, and bank-account payment methods, including checks, files, and transactions.

**Email.** Order-placed and shipment-shipped messages, HTML and plain text, with a view-tracking image and links built from the store's domain. The body is saved on `EmailMessage`. Orders and shipments keep the messages sent for them, and the order link records the revision. Store settings trigger the sends. Tests wait for the worker pool so the async sends finish before the test moves on.

**Inventory, time, and payroll.** Asset find summarizes by product, location, and lot, with dialogs for a physical inventory. An asset can move, including a partial quantity. Project time entries gained rates and a path to an invoice. Payroll adjustments can be a percentage, and a work type can override the GL account and the tax code.

**Under the screens.** Vue uploads a file through `FormData`. Select2 empty values, optional dynamic-option dependencies, dialog conditions, and focus were fixed. A JSON request gets a JSON error. The transaction cache remembers a row already locked in the transaction, and a for-update read misses the cache. `SimpleEtl` landed, with a flat-XML extractor. A multi-service call feeds each call's results into the next.