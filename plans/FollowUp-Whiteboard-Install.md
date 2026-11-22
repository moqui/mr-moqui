# Follow-up: whiteboards, presentation stage and follow, and the per-user install

Follow-up, 2026-11-22. Phases 1 to 6 are built; Phase 7 (docs, localization) and the items under **Remaining** are not.
This covers a review of the commits that brought [Comms-Whiteboard](Comms-Whiteboard.md)'s boards, meeting boards, ink
and markup, the presentation stage and follow mode, and [Install-DevCA](Install-DevCA.md)'s per-user install, shared
`~/.mr-moqui` definitions, development certificates and separate listener ports. The review found the core models
sound:
- **Boards:** guest and member grants checked on the server, all four `board@*` limits enforced, SVG shown only
  through `<img>`.
- **Stage:** presenting authorized on the server, with races between presenters fenced.
- **Follow:** a follower opens the relayed path and params under their own session.

It also found bugs, scaling risks, missing upgrade paths, failure-tolerance gaps, missing tests, stale security docs
and missing es/zh rows. **Built** means it is in the source; **Remaining** names what is left.

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

## Remaining

**Docs (Phase 7):**
- **Attack-Surface-Routes.md:** `/callws` now carries collab frames, up to about 1.4 MB.
- **Attack-Surface-Comms.md:**
  - the stage, follow (`navigate`, `locate`, `catchUp`) and ink ops, `followRate`, and the `follow:true` error flag;
  - `member_screen` gating, and the presenter and member gate (`note_stage`);
  - guest-only screen video, and guests marked at every join;
  - the cross-node follow frames;
  - the `CallGuest.xml` `boardFile` and `uploadBoardFile` transitions;
  - the meeting-scoped collab join, the in-memory meeting grants, `collab-recheck` and the 10 s recheck;
  - the collab rate limits.
- **docs/tools/Comms.md:**
  - the share modes, and that a non-host can't take over from a host;
  - Present this screen;
  - Follow again, and the pause for a changed form;
  - a reload gives up a screen share.
- **docs/interfaces/Call-Recording.md:** a follow-mode recording leaves out the presentation.
- **docs/ui/Qvt-Client.md:** `m-whiteboard`, with its props, file mode, `link-schemes`, `libraries` and `skeleton`;
  `m-ink-layer`; `m-image-markup`.
- **docs/data/Revision-Management.md:** the board codec, meeting actors, compaction and tombstones, and pruning.
- **docs/Run-Deploy.md settings:** `board@max-total-image-bytes`, `socket-updates-per-minute`,
  `socket-awareness-per-minute`.
- **The basic-comms README:** Boards.
- **[Comms-Whiteboard](Comms-Whiteboard.md):** §4.1 (the server preview and the browser's SVG), §4.4 (history has no
  thumbnails), §5.2, §5.3, §8 (rate limits now built) and §10 (the bundle size), updated to what is built.

**Localization (Phase 7):**
- **Missing es/zh rows (about 100):** literal `t()`/`localize` keys from these commits. They cover the board screens,
  Chat's Boards tab, the call board, ink and snapshot controls, the markup dialog, Recordings, the stage and follow
  labels and refusals ("Only a host may take over from a host", "{name} wants to present"), board service messages
  ("Board files exceed the board total size limit", "Snapshot too large", "Meeting ${name} ended", "Meeting ended",
  "Uploaded in a meeting by ${name}", "This board could not be read"), the CallEntities enum descriptions, and the
  Monitor, Explore, Calendar, Grid and Gallery Wizard strings from 43c61c2c3 and b43ac94bf.
- **Method:** collect the literal keys added since 05dd583d5 and check them against the rows and the `unchanged-*.txt`
  lists, as for the earlier localization pass.
- **Rust socket messages:** send codes the client localizes, or English keys it passes through `t()`. They are in
  `notws/collab.rs` ("too many updates; slow down", "too many updates; join the page again"), `board.rs` and
  `notws/rtc.rs`. Follow errors are now dropped by the page, so the rtc ones matter least.

**Not built, needing a decision:**
- **Recurring board for a guest or view-only first joiner:** `openLink` runs only for the link owner. Opening it for
  whoever joins first needs a new open path in `guestJoin` and `join`, with a stage presenter who may be a guest.
- **A board's image link token:** `revokeLink` removes guests, which now rechecks and ends drawing. There is no
  token-rotation operation to align with image access.
- **`*Name` params sent to followers** (a `userName` a member cannot open): left as they are.

**Tests not written:**
- client page tests for the whiteboard's `board.language` and tombstones;
- `save#MeetingBoard` at call end;
- the SFU's new-track labelling;
- `queueFollow` choosing `locate` (the page test cannot set the page location);
- the real trust store removal and keychain paths (only `bash -n` and dry runs).

**Checks to make:**
- **Webroot client budget:** the suite was at 45.5 of 48 s before the board and follow client tests. Measure it after
  these changes; if it is over, move WebrootWhiteboard's Mermaid conversion to acceptance.
- **`tools/test/client/GridDates.spec.js`:** 2.7 s, but Data/Grid is also rendered by `tools/test/DataGrid.spec.js`
  and mounted offline in `WebrootVue.qvt.markdown`. Decide whether its date checks fold into one of those renders, per
  the render-once rule.
- **Acceptance `MeetingBoards`:** run it once, since meeting-board access now answers from memory and is rechecked on
  call changes.

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
