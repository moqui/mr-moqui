# Comms Whiteboard: boards, diagrams, and screen share in meetings

Plan, 2026-11-21; nothing in it is built. The whiteboard is a tool of its own: a board to draw and diagram on, used
in Comms meetings, in wiki spaces, and in Dev AIDE's HEMP design work. It is built first (MW1, §11), with its drawing
and diagram support, and [Dev-HEMP](Dev-HEMP.md) depends on it. The plan covers:

1. **The whiteboard**: a new wiki page type drawn with Excalidraw and co-edited over the existing `yrs` rooms, also
   usable on a plain file with no co-editing (Dev AIDE's `hemp/` boards), with Mermaid frames that keep their source,
   shape libraries, element links to records and artifacts, and boards built from a skeleton a service supplies.
2. **Boards in meetings**: a room's boards, a board on a call's stage, guests drawing through their call socket.
   **Process editing** on a board, workflow-like editing of a structured process (a HEMP story, a BOSS flow map, a
   routing, a status flow), is covered briefly (§7); it needs a plan of its own.
3. **Screen share** beyond what is built, for the group calls of Comms (BasicComms Calls, a chat room's call, a call
   link's meeting): tab audio, the choice of surface, rules for who presents, host controls, annotation, presenting a
   Moqui screen by following it instead of by pixels, the desktop app and Android.

It follows [Comms-Messaging](Comms-Messaging.md) (§8 calls) and the archived
[Comms-Framework](archive/Comms-Framework.md) (W2 built screen share), and reuses the co-editing of
[Revision-Management, Co-editing](../docs/data/Revision-Management.md#co-editing). In the tables, **Built** means it
is in the source today, **Partial** names what is missing, and **Planned** means this plan adds it. *Unverified*
marks a claim from outside sources that a spike must check before the work depends on it.

## 1. Where we start

### 1.1 Screen share

| Part | Status | Where |
|---|---|---|
| Share the screen from the browser | Built: `getDisplayMedia({ video: { frameRate: 15 }, audio: false })`, its own `sendonly` transceiver, a `publish` frame with `source: "screen"`; a direct call moves to the media server first (`toRelay("share")`) | `webroot/js/WebrootCall.js` `shareScreen`, `stopShare`, `renegotiate` |
| See a share | Built: a leg's tracks split into its camera stream and its screen stream by the SFU's track sources; the speaker view shows the share large, the grid as a tile, the floating window the share first | `WebrootCall.js` (`isScreen`, the layout's `sharing`) |
| Layers by tile size | Built for every publisher (high, mid, low, none) | `crates/moqui-rtc/src/sfu/` |
| Share audio | Partial: the SFU knows `Source::ScreenAudio` and `AudioRoom` decodes a leg's screen audio as its own stream (Comms-Framework M0); the browser never asks for audio | `crates/moqui-rtc/src/sfu/mod.rs` |
| Recording a share | Built: the screen track as VP8 in WebM, its audio as Opus in Ogg | [Call-Recording](../docs/interfaces/Call-Recording.md) |
| E2EE share | Built: the screen sender gets SFrame like the camera | `moqui.callKeys.attachSender` |
| Who may share | None: any participant, any number at once. Roles are `CprHost`, `CprMember`, `CprGuest`, `CprForward` (`CallSetupData.xml`); no presenter state | |
| Host stop, banner, presenter's own view | None | |
| Surface choice, content hint, region or element capture, scrolling the captured tab | None | |
| Desktop app | Partial: Tauri 3 on CEF; whether CEF shows a source picker for `getDisplayMedia` in our build is *unverified* (reports differ by CEF version and runtime style) | `crates/moqui-desktop` |
| Android app | None: Android's WebView has no `getDisplayMedia` (*unverified* for the current WebView; Chrome on Android has none). The patched wry has a `DisplayCapture` permission kind with no backend | `crates/moqui-mobile/patches/wry/src/permissions.rs` |
| Tests | The `moqui-rtc` speaker and share cases; Future-Plans lists share as something headless Chromium cannot drive | [Future-Plans](Future-Plans.md) |

### 1.2 Whiteboard and co-editing

| Part | Status | Where |
|---|---|---|
| Co-editing rooms | Built: `moqui-collab` holds a `yrs` document per key, joins with access checks, relays in a cluster (`collab-update`), writes the state a second or two after an edit and a version on save, idle and last leave | `crates/moqui-collab/src/lib.rs`, `crates/moqui-host/src/notws/collab.rs` |
| Document kinds | Partial: the key is always `WikiPage:<id>` and the document is always markdown in Tiptap's structure (`collab/markdown.rs`) | `moqui-collab` `markdown.rs` |
| Storage | Built: `moqui.resource.CollabDocument` (state, snapshot text, base hash, editors) | `framework/entity/ResourceEntities.xml` |
| Wiki page types | `md`, `html`, `txt`, and the `.fancy` script types for `DATABASE_ADMIN` (`get#WikiPageTypes`) | `framework/service/org/moqui/resource/WikiServices.xml` |
| Yjs in the browser | Built inside the Tiptap bundle (`yjs` 13.6.33, `y-protocols`, exported as `Tiptap.Y`) | `webroot/screen/webroot/libs/refresh.mjs` |
| Mermaid | Built: Mermaid 11, `m-mermaid` and mermaid fences in markdown, drawn on first use | `WebrootVue.qvt.js` `moqui.mermaid` |
| Whiteboard | None, and no plan: mr-three says "a shared whiteboard is intended later, as another kind of wiki document" ([concept](../runtime/components/addon/mr-three/plan/concept.md)); its [technical design](../runtime/components/addon/mr-three/plan/design-technical.md) says none is planned in the tree | |
| Wiki screens | BOSS only: IronStone Commons Knowledge and Notes with `m-coedit` (mr-boss `screen/Commons/Knowledge.qvue`, `Notes.qvue`). BasicComms has no wiki screen | |
| Rooms and wiki | A chat room has members, files from its messages and a record it may belong to; no wiki space | `ChatEntities.xml` `ChatRoom` |

The research for BOSS ([research-drawers-commons §13](../runtime/components/addon/mr-boss/plan/research-drawers-commons.md#13-notes-wiki-knowledge--discussions-commons))
already notes that Nextcloud's whiteboard is Excalidraw over a sync server with follow, presentation, timer and
voting.

## 2. Decisions (2026-11-21)

| Question | Decided |
|---|---|
| Whiteboard library | **Excalidraw** for boards; **drauu** as the light overlay for marking up a screen share or an image |
| Where a board lives | **Always a wiki page** of a new type. A room has its own wiki space, made on first use, whose access is the room's membership; a call links the boards it shows |
| Guests and E2EE | Guests in a meeting **view and edit** the call's boards through their call socket. A board in an encrypted room is offered with the "not end-to-end encrypted" label; E2EE boards are a later phase |
| Screen share scope | All four: the browser upgrades, presenting a Moqui screen (follow), the desktop app, Android |

Guests and encryption never meet: an encrypted call already refuses guests (`encryptedNoGuests`,
[WebRTC](../docs/interfaces/WebRTC.md)).

### 2.1 Libraries considered

| Library | License | Fit | Verdict |
|---|---|---|---|
| [Excalidraw](https://github.com/excalidraw/excalidraw) 0.18 | MIT | A full board: shapes, arrows, text, frames, images, laser pointer, follow; collaboration left to the host; elements carry `id`, `version`, a fractional `index` and `customData`. React 17 to 19 | **Chosen.** Bundled with React as an IIFE like Tiptap, loaded only on a board |
| [@excalidraw/mermaid-to-excalidraw](https://docs.excalidraw.com/docs/@excalidraw/mermaid-to-excalidraw/api) | MIT (*unverified*: the repository's file) | Mermaid flowcharts to editable shapes, other diagram types as an image; one way | **Chosen**, inside the board |
| [drauu](https://github.com/antfu/drauu) | MIT | Headless SVG drawing (freehand, stylus pressure, lines, rectangles, ellipses, undo), no framework, made for Slidev's annotations | **Chosen** for annotation |
| [y-excalidraw](https://github.com/mizuka-wu/y-excalidraw) | *unverified* | Excalidraw elements over a `Y.Array`, awareness cursors | Read for ideas; the binding is ours (§4.3) so the server can read the document |
| [tldraw](https://tldraw.dev/legal/tldraw-license) 4 | tldraw license | Needs a license key in production; without one it stops rendering after five seconds; the hobby key keeps a watermark | Refused |
| [draw.io](https://www.drawio.com/docs/reference/embed-mode/) | Apache-2.0 files, not all sources published | An iframe app; "not suitable as a framework for building other products"; imports Mermaid | Refused |
| Quickdraw, Kritzel, ue-too | various | Young; little known | Not now |

Mermaid itself only draws text. "Editable Mermaid" is two things here: the text with a live preview (the markdown
fence and `m-mermaid`, which exist), and a board's Mermaid import that keeps its source so it can be edited again
(§4.5).

## 3. Screen share

### 3.1 The stage

A meeting has one **stage**: what the speaker view shows large. It is one of a leg's screen share, a board (§5.2), or
a followed screen (§3.4), or nothing (the active speaker). The stage is live call state, like raised hands, kept by
the call engine and sent to the call's legs; it is not a role.

- `CallSession.shareModeEnumId`: `CsmAnyone` (default for a room's call and a direct call), `CsmHosts` (hosts only),
  `CsmAsk` (a member asks; a host lets them). A call link has the same field for its meetings
  (`CallLink.shareModeEnumId`, default `CsmAnyone`), copied when a meeting opens. A host changes it in the call.
- **One share at a time.** A member who shares while another shares is asked "Take over from Ana?"; a host takes
  over at once. The one taken over is told and their share stops. A second simultaneous share is an open question
  (§10).
- **Enforced at the media server.** The SFU forwards a `screen` and `screenAudio` track only from the leg the stage
  names. A share that starts without the stage (an old page, a modified one) is not forwarded, and the leg is told.
- **Host stop.** A host's Stop sharing ends the stage; the presenter's page stops its capture. Removing a
  participant already ends their legs.
- **The banner.** "Ana is presenting" over the stage for everyone; for the presenter, "You are presenting" with Stop
  and, while their own tab is the capture, a warning before they open the call's full screen (a mirror of a mirror).
- **Presenter's view.** The presenter sees their share small, not on the stage, by default.

Frames: `rtc: "stage"` (ask, take, stop, lower) from a page; `rtc: "stage"` with the current stage to the legs. The
services: `set#CallShareMode`, and the stage kept in the engine with `get#Call` showing it.

### 3.2 Browser upgrades

| Item | Work |
|---|---|
| Tab and window audio | `audio: true` with `systemAudio: "exclude"` by default (Chrome offers audio for a tab or window, *unverified* for a whole screen); the audio track published as `screenAudio`. A share's audio is mixed for phone legs as the microphone is |
| What to share | A choice before the picker: **This tab** (`preferCurrentTab`, for presenting a Moqui screen with the call kept off it by Element Capture), **A window or screen** (`displaySurface: "window"`), with `surfaceSwitching: "include"` and `selfBrowserSurface: "exclude"` when not this tab. Options a browser does not know are ignored |
| Text or motion | "Optimize for text" (default; `contentHint = "detail"`, 15 fps, the resolution kept) or "for video" (`"motion"`, 30 fps); the encoder's `degradationPreference` to match |
| Share part of this tab | Element Capture (`RestrictionTarget.fromElement`, `track.restrictTo`) for a board or a screen's main area, so the call's controls and notices are not captured (Chrome; *unverified* version). Region Capture (`CropTarget`) where Element Capture is missing |
| Scroll and zoom the captured tab | Captured Surface Control (`CaptureController`, `forwardWheel`, zoom level): the presenter scrolls a shared tab from the call view without switching to it (Chrome; needs the `captured-surface-control` permission) |
| Conditional focus | `CaptureController.setFocusBehavior("no-focus-change")` when the presenter shares a window and stays in the call |
| Viewer's controls | Pinch and wheel zoom and pan on the stage; Fit and 100%; full screen of the stage alone |
| Low bandwidth | The share keeps priority over cameras: a viewer short of bandwidth drops cameras to `none` before the share drops a layer |
| Phone view | On a phone width the stage is the call screen's main picture, landscape on rotate |
| Terminal | The terminal call shows "Ana is presenting" and a board's link; it shows no picture |

### 3.3 Annotation (drauu)

- **Over a share.** Annotate in the stage's menu opens a drauu layer over the shared picture for anyone the presenter
  allows (Off, Hosts, Everyone; default Off). Strokes are in coordinates of the picture (0 to 1 each way), sent as
  `rtc: "ink"` frames through the call server to the call's legs, never stored, and fade after 5 seconds unless
  pinned; Clear removes all. A laser pointer is a stroke that fades at once.
- **Seen by the presenter.** When the share is this tab, the presenter's page draws the strokes on itself, so they
  are in the capture and in a recording. For a window or a screen they show on the presenter's small view, and on the
  desktop app's overlay window (§3.5).
- **To a board.** Snapshot to board takes the stage's current frame (a canvas from the video), makes a new board in
  the room's space (§5.1) with it as a locked image and the strokes as shapes, and puts it on the stage.
- **Over an image.** The same layer over a picture in chat or a record's image (Markup), saved as a new image beside
  the old one. This is the small form; a larger one is a board.

### 3.4 Present a Moqui screen (follow)

A screen shown by following, not by pixels: each viewer's own page opens the screen the presenter is on, with the
viewer's own permissions and data. Text is sharp at any size, and nobody sees a record they may not.

- **Start.** Present this screen (in the call menu, on a screen of an app) takes the stage as a follow: the
  presenter's page sends its screen path and parameters on every navigation, as `rtc: "follow"` frames.
- **Followers.** A follower's page opens it in the main pane (the call moves to the side panel, as when the call's
  screen is left). Stop following keeps them where they are; Follow again returns.
- **Checks.** The path must be a screen of this server under `/qapps` or an app root; the follower's own request
  is authorized as any other. A screen the follower may not open shows "Ana is on a screen you cannot open" and
  no parameters. Parameters are only sent to members' pages, never to guests or phone legs.
- **Where.** The scroll position by the screen's sections (the nearest `id` above the top), the presenter's pointer as
  a marker over the element it is on, and an open dialog by its name. Field values are not sent.
- **Guests.** A guest has no screens, so for them a follow is a pixel share: the presenter's page shares this tab
  with Element Capture at the same time, forwarded only to guest legs.
- **mr-three.** This is the same act as its Follow me ([design-world](../runtime/components/addon/mr-three/plan/design-world.md));
  the plan's frames are the ones it would use.

### 3.5 The desktop app

- **Spike: the picker.** Does `getDisplayMedia` in our `tauri-runtime-cef` build show Chromium's source picker on
  Linux (X11 and Wayland with the PipeWire portal), macOS and Windows? Reports differ by CEF version and by the Chrome
  or Alloy runtime style (*unverified*).
- **If it does:** grant `DisplayCapture` in the app's permission handler and add macOS's screen recording usage
  description and entitlement.
- **If it does not:** the app's own picker. The app lists screens and windows natively, and the page asks for the
  chosen source by its media source id through CEF's media access callback. This is a larger piece; its design
  follows the spike.
- **Overlay window.** While a window or screen is shared, a transparent, click-through, always-on-top window over
  that display draws the annotation strokes (§3.3) and the presenter's laser. Later in the phase, after the picker.

### 3.6 Android

The WebView cannot capture the screen, so the app shares natively, as a **screen leg**: a second leg of the
participant whose only track is the screen, which the SFU and the pages treat as that participant's share.

- **Capture.** MediaProjection, after the system's consent each time, with a foreground service of type
  `mediaProjection` (Android 14 needs both, and a fresh projection for each session).
- **Encode and send (spike).** Option A, preferred: MediaCodec (hardware VP8, else the platform's software VP8
  encoder) into `str0m` in the app's Rust, which the workspace already uses (MIT OR Apache-2.0, no C), connected to
  the SFU as the screen leg. Option B: a libwebrtc Android build. The spike measures A's latency, its keyframes on a
  new subscriber, and its bandwidth control; B only if A fails.
- **Signalling.** `add#ScreenLeg` makes the leg (type `CltScreen`, the participant's) and the app connects it with the
  same rtc frames and credential rules as the page's leg; ending the share or the call ends it.
- **E2EE.** An encrypted call's screen leg needs SFrame. The app has the `sframe` crate's suites available in Rust;
  the leg's key comes from the page's call keys over the page bridge. Until that is built, an encrypted call on
  Android offers no screen share and says so.
- **Annotation on Android.** None at first: Android draws nothing over other apps without a special permission, which
  this plan does not ask for.

## 4. The whiteboard page type

### 4.1 The type

- A new plain page type `excalidraw` (the file `<Page>.excalidraw`, Excalidraw's own JSON scene format) in
  `get#WikiPageTypes`' `plainTypeList`. Anyone who may create a page may create one; nothing in it is script.
- The file is the scene: `type`, `version`, `elements` (sorted by `index`), the board's settings from `appState`
  (background, grid), and `files` naming the images by attachment, not as data URLs. A board saved in a git space
  diffs by element, one per line.
- Its images are the page's attachments (`<Page>/` as any page's), uploaded through the existing attachment services,
  with the board's limits (§8).
- **A preview.** On each version the editor's page uploads an SVG export as the attachment `_board.svg`. It is shown
  only as an `<img>` (no script runs in an SVG image), used for the chat card, a page list's thumbnail, printing and
  PDF. A board with no preview yet shows its name and "Open the board".
- **Search.** The snapshot text the server keeps is the board's text elements in reading order, so a board is found
  by its words.

### 4.2 `m-whiteboard`

A Vue component in webroot over Excalidraw (React mounted in the component's element, unmounted with it):

- **Props:** the page (`wikiPageId`), read only or editable by access, `follow` (a leg to follow), `stage` (in a call:
  no menus but the tools, the call's presence).
- **File mode.** With no page, the screen gives the scene (`scene`, the `.excalidraw` JSON, and `files-url`, where
  its images are) and a save transition (`save-url`); the board is edited by one person with no co-editing and saved
  as a whole on Save. Dev AIDE uses it for the boards in a component's `hemp/` directory, saved through its own save
  pipeline ([Dev-HEMP](Dev-HEMP.md) H23). The file and its images are laid out as a page's are (§4.1), so the same
  board can move between a component and a wiki space.
- **A skeleton.** A screen may give `skeleton`: Excalidraw's element skeletons (a frame, a labelled rectangle, an
  arrow between two ids, text), which the component turns into full elements with `convertToExcalidrawElements`
  when the board is new or on the screen's Regenerate. The server never computes text sizes or element defaults. Dev
  AIDE's Draw sketch (Dev-HEMP §7.2) and the Mermaid frames (§4.5) use it.
- **Element links.** An element's link may be a web address, a screen path of this server (opened in the shell), or
  a link a screen declares (`link-schemes`, such as `hemp:`), which the component shows as a chip and hands to the
  screen to open. The board's file keeps the link as written, so an index can read it (Dev-HEMP H24).
- **Libraries.** Excalidraw shape libraries (`.excalidrawlib`) served from `libs/excalidraw/library/` and from a
  component's `library/` directory named in a screen's `libraries`: webroot ships a wireframe library (buttons,
  fields, a form, a table, tabs, a dialog, a menu, in the shell's shapes) and a flowchart one. Excalidraw's public
  library site stays off.
- **Bound elements** (later, §7): elements whose `customData` names an item of a model the screen owns, edited
  through the screen's operations rather than as drawing. MW1 only keeps `customData` as written.
- **Off:** Excalidraw's embeddables (web pages in iframes), its link to its public libraries site, its own share and
  collaboration UI, its AI text to diagram (§9 has ours), and anything that fetches from outside the server. Fonts and
  assets are served from `libs/excalidraw`.
- **On:** the tools, frames, images, the laser pointer, follow (by our presence), dark mode by the shell's
  `QUASAR_DARK`, the shell's language where Excalidraw has it.
- **Fallback:** where the board cannot start (no notification socket, an old browser), the preview and Download.

### 4.3 Co-editing a board

`moqui-collab` today assumes markdown. It gains a **codec per document kind**, chosen by the page's type:

| Codec | Document | Snapshot (the page text) |
|---|---|---|
| `markdown` (today's) | the Tiptap XML fragment `default` | the markdown |
| `excalidraw` (new) | a `Y.Map` `elements`: element id to the element as a JSON value; a `Y.Map` `files`: file id to its attachment; a `Y.Map` `app`: the board's settings | the `.excalidraw` JSON, elements sorted by `index`; deleted elements dropped on a version |

- **Merge.** An element is replaced whole when it changes, the later `version` (then `versionNonce`) wins, as
  Excalidraw reconciles its own collaboration; the fractional `index` keeps the order under concurrent moves. The
  binding (in `m-whiteboard`) writes changed elements on Excalidraw's `onChange` and applies remote ones with
  `updateScene`, throttled to animation frames.
- **Presence.** Awareness carries the pointer, the selection, the user's name and colour, and the laser; Excalidraw's
  collaborators map is filled from it.
- **Yjs once.** Yjs comes from one global: the Tiptap bundle's `Tiptap.Y` today; the board bundle takes it as
  external. If that proves awkward, `yjs`, `y-protocols` and `lib0` move into a `libs/yjs` bundle both use.
- **Versions** as for a page: on Save with a note, after a while without edits on database files, when the last
  editor leaves. Excalidraw's own undo stays local to each editor.
- **Outside changes** as for a page: a board uploaded or restored with no edits open is read again at the next join.

### 4.4 Viewing and editing outside meetings

- **BasicComms:** a Board screen (`BasicComms/Boards/Board?wikiPageId=`) with the board, its history (the page's
  versions, each with its preview), Save and Download (`.excalidraw`, SVG, PNG). Qvt only; the terminal shows the
  page's name, its editors and its link.
- **BOSS:** Knowledge and Notes show an `excalidraw` page with `m-whiteboard` where they show `m-coedit` for
  markdown, and New page offers Whiteboard. That work is in mr-boss.
- **A board in a markdown page:** a wiki link to a board page draws its preview with a link to open it.

### 4.5 Mermaid on a board

- **Insert, Mermaid** opens a text box with CodeMirror and a live preview (`m-mermaid`), then converts it with
  mermaid-to-excalidraw: a flowchart to editable shapes, other kinds as an image.
- **Kept source.** The converted elements are put in a frame whose `customData` holds the Mermaid text. **Edit as
  Mermaid** on that frame reopens the text and replaces the frame's elements; edits made to the shapes since are lost,
  with a warning that says so. This is the "editable Mermaid": the text stays the source until someone chooses to
  draw by hand.
- **Mermaid once.** The converter imports Mermaid; the bundle marks it external and uses the `mermaid` global that
  `moqui.mermaid.load()` gives, so Mermaid is not shipped twice (*unverified* that the converter takes it as a peer).

## 5. Boards in rooms and meetings

### 5.1 A room's space

- `ChatRoom.wikiSpaceId`: the room's space, made on first use by `ensure#ChatRoomSpace` (a database-files space, the
  room's name, `restrictView` and `restrictUpdate` Y).
- **Access is the room's.** `get#WikiPageAccess` answers for a room's space by the room: a member may view and
  update, a room manager may administer, an archived room is read only, and someone who leaves loses access. No
  `WikiSpaceUser` rows are written, so membership has one source.
- **Boards tab.** The room's details (Comms-Messaging §5.1) gain Boards: the space's boards, newest first, with
  previews; New board.
- **In the timeline.** A new board, or one put on a call's stage, posts a board message: its preview, name and Open.
  A board in a public or record room follows the room's own visibility.
- **A record room** (a project, a case) shows its boards on the record's chatter where the record's screen has one.
- **Retention.** A room's retention and disappearing messages do not delete boards; deleting the room deletes its
  space and boards after the same confirmation.

### 5.2 A board on the stage

- **In a call:** Whiteboard in the call's menu opens the room's boards (New board, or one of them, or any board page
  the user may see), and takes the stage (§3.1) with it. A call with no room (a call link's meeting, an ad hoc call)
  uses its link's room or, with none, the starter's own space; the board is listed on the call's record.
- `moqui.call.CallBoard` {callSessionId, wikiPageId, openedByUserId, openedDate, closedDate}: the boards a call has
  shown, for its history and for the guests' access below.
- **Everyone draws** by default; a host can make the board view only for members or guests.
- **Follow the presenter.** The one who put the board on the stage leads: others follow their view (Excalidraw's
  follow) until they pan away, and Follow again returns.
- **A board and a share.** A presenter can share their screen while a board is open; only one is on the stage, and
  Switch moves between them.
- **After the call.** The board's last version is saved when the call ends (a note names the call). A recording's
  record links the call's boards; a recording does not capture the board unless someone shared it as a tab.
- **A call link's board.** `CallLink.wikiPageId`: a recurring meeting's board, opened on its stage each time the
  meeting starts.

### 5.3 Guests

- A guest joins a board only through `/callws`, with `collab` frames (`join`, `update`, `awareness`, `leave`) that the
  socket accepts only for a page in an open `CallBoard` row of the guest's own call while they are admitted.
- `moqui-collab` takes an **actor** in place of a user id: a user, or a guest by `callGuestId` with the access their
  call gives. A guest's edits are named in the version note as "Ana (guest)"; the version is written as the call's
  host.
- A guest cannot open the board's page, its history, or other boards of the space.
- Removing or denying a guest, or the call ending, closes their collab documents with the socket.

### 5.4 Encrypted rooms

- A board in an encrypted room's call or Boards tab carries the label "Not end-to-end encrypted: the server stores
  this board", by the label rules of Comms-Messaging §6.2, and the first board in such a room asks once.
- **Later, E2EE boards.** Yjs updates encrypted with a room key before they leave the page; the server keeps only
  ciphertext, merges nothing, writes no snapshot or preview, and the room's members' pages compact the document. Needs
  its own design.

## 6. Other places a board fits

| Where | Use | Owner |
|---|---|---|
| Wiki and Knowledge | A page of type Whiteboard in any space | mr-moqui type; BOSS Knowledge and Notes screens |
| Chat | A board message; Boards tab; a quick sketch in the composer that sends as an image (drauu) | BasicComms |
| Mail | Insert a drawing as an image in Compose | BasicComms, later |
| Projects (BOSS) | A project's boards (its record room's space): planning, retrospectives, a story map | mr-boss |
| Maintenance (BOSS) | Mark up a photo of an asset or a work order's image (§3.3's Markup) | mr-boss |
| Dev AIDE, Dev-HEMP | Boards as files in a component's `hemp/` (file mode, §4.2), an optional tool: layout sketches for prototyping and talking about a screen's layout beside its Screen Outline (the Functional Wireframe, a prototype screen, is recommended once the general idea is known), interview and design boards whose elements link to activities and other artifacts, a story or the Screen Flow Diagram opened on a board as a Mermaid frame, the book's figures; later, stories edited on a bound board (§7) | [Dev-HEMP](Dev-HEMP.md) H23 to H26, §7.6; needs MW1 |
| Assist | Assist writes Mermaid for a board from a description (§9); a board's text as context | [LLM-A2A-Assist](LLM-A2A-Assist.md) |
| mr-three venues | The venue's big screen shows the meeting's stage, a board included | mr-three |
| PASS | Not planned: a board is internal; a portal customer sees a board only as a guest in a meeting | |

## 7. Process editing on a board

Workflow-like editing of a process: the process is a structured model (steps, the lane or actor of each, their
order, what follows what), and the board is a view of it that can also be edited. This is a significant piece of work
and needs a plan of its own; this section sets the direction, names the editors it would serve, and says what MW1
must leave room for.

### 7.1 A bound board

- **The model stays in its own store**: a HEMP story file, a flow map Definition, a routing's operations, a status
  flow's transitions. The board is a view of it, kept beside it (a HEMP story's `story/<Name>.excalidraw`, a flow
  map's layout in its Definition), and holds only positions and the drawing around the model.
- **Bound elements.** Lanes are frames, steps are shapes from the model's library, order and transitions are arrows
  bound to their shapes. Each bound element's `customData` names its item (`{"model": "hemp-story",
  "ref": "FulfilOrder#a3"}`).
- **Edits are operations.** Moving a step to another lane changes its actor; moving it within the lane reorders it;
  an arrow drawn between two steps adds a transition where the model has them; editing a label renames; a new shape
  from the model's library in a lane adds a step; Delete removes one after the model's own checks. Each is one
  operation through the screen's services, as Dev-HEMP's forms are (its H7), never a write of the board's JSON. A
  refused operation puts the element back and says why.
- **Layout.** The model gives a generated layout (lanes as columns or rows, steps in order, branches beside the main
  flow). A position the user sets is kept in the view by the item's ref and survives regeneration; a new item is
  placed by the generator.
- **Drawing beside the model.** Notes, sketches and arrows to nothing are allowed and stay drawing: a workshop's
  marks are not the model. Make step on a note turns it into a bound step.
- **The lists stay.** The model's list form (Dev-HEMP's sentence rows, a flow map's ordered lanes and steps) is the
  terminal's and the accessible form, and the one a large change is made in. The board is the browser's view.

### 7.2 What Excalidraw gives and lacks

- **Gives:** free layout, frames, arrows bound to shapes that follow them, text in shapes, libraries, a clean or a
  hand-drawn style (roughness 0 for a process), `customData` on every element, and `onChange` with element versions.
- **Lacks:** ports, constraints, a model's rules, automatic layout, and an event per edit. So `m-whiteboard` gains a
  **binding mode**: a model adapter in the screen's script that maps the model to elements and an `onChange` diff (by
  element version) to operations; bound elements locked against changing type or being grouped away from their
  lane; the model's palette in place of the general tools.

### 7.3 To compare first

The process editing plan starts by comparing a bound Excalidraw board with a graph editor library on two models,
Dev-HEMP's story and Insights' flow map, at phone width and with 50 steps:

- **Vue Flow**: Vue 3 nodes, edges and handles (MIT, *unverified*).
- **maxGraph**: the TypeScript successor of mxGraph (Apache-2.0, *unverified*).
- **diagram-js**: MIT. bpmn-js, built on it, needs its bpmn.io watermark kept visible
  ([bpmn.io forum](https://forum.bpmn.io/t/licensing-question/15164)), so bpmn-js itself is not taken.

Excalidraw's case is one tool for sketching and processes, and a workshop's notes and the model on one board. A
graph editor's case is ports, edge routing and layout built in.

### 7.4 Where it applies

| Editor | Today | With a bound board |
|---|---|---|
| Dev-HEMP stories (`Requirements/Stories`, Edit) | Planned: sentence rows, Move up and down, forms (H7) | Lanes by actor, activities in order, alternate flows as branches, a high-level activity's detailed story as a frame that opens it; Make step from an interview note |
| BOSS Insights FlowMapEditor ([design-insights §9](../runtime/components/addon/mr-boss/plan/design-insights.md#9-business-flow-maps)) | Built (P9): ordered lane and step lists; "no new freeform diagram-authoring engine is required"; FlowMap drawn with XML widgets; the framework node-graph panel rejected as it is (F26) | The map's lanes and steps on a bound board: drag a step between lanes, reorder, a step's target and count on its shape; FlowMap gains a board view with the lists kept; a HEMP story imported as a flow map (Dev-HEMP H19) brings its layout |
| BOSS Manufacturing RoutingDetail ([design-manufacturing §5](../runtime/components/addon/mr-boss/plan/design-manufacturing.md#5-boms-and-routings)) | Built (P2): the operation sequence as rows | Operations as a flow with work centers as lanes and dependencies as arrows; drafts and release unchanged |
| Tools StatusFlows (`runtime/components/base/tools/screen/Tools/StatusFlows.xml`) | Transitions as a list | Statuses as shapes and transitions as arrows; an arrow drawn adds a `StatusFlowTransition` |
| BOSS Marketing journeys | Not in the first release ("no new visual journey canvas", design-marketing) | A branching journey, if Marketing plans one |
| BOSS Customer Service assignment rules, approval rules in Purchasing and Marketing | Rule lists | Not a process canvas: they stay lists |

Each BOSS change belongs in mr-boss's own design and impl plans, after this one's process editing plan.

## 8. Security and limits

- **Board files.** A board's JSON is parsed, never run; Excalidraw draws text as text. A link element opens only
  `http`, `https` and `mailto` targets, in a new tab, after the shell's external link rule.
- **Previews.** An uploaded `_board.svg` is stored as an attachment and served with `Content-Type: image/svg+xml`,
  `Content-Disposition: attachment` unless shown through `<img>`, and the attachment CSP. It is never inlined.
- **Images.** The page attachment rules: types `image/png`, `image/jpeg`, `image/webp`, `image/gif`, `image/svg+xml`
  as an image only; `board@max-image-bytes` (10 MB) and `board@max-images` (200) as settings on a new `board` element
  in `MoquiDefaultConf.xml` with its XSD and schema entry.
- **Frames.** A collab update over `/notws` or `/callws` is limited by size (`board@max-update-bytes`, 1 MB) and
  rate as the socket limits other frames; a board document over `board@max-document-bytes` (20 MB) takes no more
  edits and says so.
- **The stage.** Enforced at the SFU (§3.1); `ink` and `follow` frames are relayed only within the call; `follow`
  parameters never reach guests or phone legs.
- **Permissions.** A share is always the browser's (or the system's) prompt; nothing captures without it. The
  desktop app's own picker, if built, shows every source and asks each time.

## 9. Later

- E2EE boards (§5.4).
- Board templates (a retrospective, a flow, a week's plan) as pages in a templates space.
- Assist's text to diagram: Assist writes Mermaid from a description, put on the board as a kept-source frame
  (§4.5), never sent to Excalidraw's own service.
- A presenter's timer and voting on a board, as Nextcloud's whiteboard has.
- Several shares on the stage at once, if §10 decides so.
- Board export to PDF with the page's print layout.
- Share and annotation on iOS.

## 10. Open questions

- **One share or several.** One at a time with take over (§3.1) is the default; should a host be able to allow two
  side by side?
- **The space of a call with no room.** The starter's own notes space (BOSS Notes has one per user; the framework has
  `ensure#UserNotesSpace`) or a hidden space per call link?
- **Guest drawing default.** Everyone draws by default; should a meeting from a call link start view only for guests?
- **The board bundle's size** (Excalidraw, React and the converter) is *unverified*; measure it in MW1's spike and
  decide whether the converter loads only on Insert, Mermaid.

## 11. Phases

Each phase follows Comms-Messaging §18.2's rules: focused tests per phase commit, the specs it touches, and
`./moq test-framework` once at the end of the work. Acceptance tests run only where a phase adds or changes one.

MW1 is first: the whiteboard with its drawing and diagram support, which Dev-HEMP's design work then uses. The
meeting and screen share phases follow in the order below.

| Phase | Work | Depends on |
|---|---|---|
| **MW1** | The spike (bundle size, Yjs as one global, Mermaid as external); the `excalidraw` page type, the codec in `moqui-collab`, `m-whiteboard` with file mode, skeletons, element links and the wireframe and flowchart libraries, the preview, the Board screen in BasicComms, Mermaid with kept source, the `board` conf element (§4) | — |
| **MS1** | The stage and share mode (§3.1), tab audio, the surface choice, the content hint, Element and Region Capture, Captured Surface Control, viewer zoom, bandwidth priority, phone view, the terminal line (§3.2) | — |
| **MW2** | Room spaces and their access, the Boards tab, board messages, `CallBoard`, a board on the stage with follow, guests over `/callws`, the encrypted room label, `CallLink.wikiPageId` (§5) | MS1 (the stage), MW1 |
| **MA** | drauu: annotation over a share, the laser, Snapshot to board, Markup of an image (§3.3) | MS1, MW2 for Snapshot |
| **MS2** | Present a Moqui screen (§3.4) | MS1 |
| **MD** | The desktop spike and then the picker, the permissions, the overlay window (§3.5) | MS1, MA for the overlay |
| **MM** | The Android spike (str0m and MediaCodec, or libwebrtc), the screen leg, MediaProjection, then SFrame for encrypted calls (§3.6) | MS1 |
| **BOSS** | Knowledge and Notes open boards; Projects' and Maintenance's uses (§6), in mr-boss's own plans | MW1, MA |
| **Dev-HEMP** | Boards in `hemp/` and their index, Draw sketch, Open as board, the book's figures ([Dev-HEMP](Dev-HEMP.md) §11) | MW1 |
| **MP** | Process editing (§7): its own plan first, with the comparison of §7.3; then the binding mode of `m-whiteboard`, Dev-HEMP's stories, and the BOSS editors in mr-boss's plans | MW1; Dev-HEMP P2 for the stories |

### 11.1 Done when

- **MS1.** A member shares a tab with its audio; others hear it and a phone leg hears it mixed. A second member's
  share asks to take over and the first one stops. In `CsmHosts` a member's share is refused at the SFU. A host's Stop
  ends it on the presenter's page. Element Capture leaves the call's controls out of a shared tab.
- **MW1.** Two users draw on one board in two browsers and see each other's pointers; the page's versions hold the
  `.excalidraw` JSON and a preview; a Mermaid flowchart becomes shapes and Edit as Mermaid reopens its text; a board
  saved in a git space is a readable diff. In file mode a board loads from a screen's scene and saves whole through
  its transition; a skeleton becomes a board; an element's `hemp:` link is handed to the screen; the wireframe
  library's shapes are in the library panel with no request outside the server.
- **MW2.** Whiteboard in a room's call makes a board in the room's space, posts it in the room, and puts it on the
  stage; a guest by call link draws on it and is refused for any other board; a member who leaves the room loses it.
- **MA.** Strokes over a shared tab show in the capture and a recording; Snapshot to board makes a board with the frame
  and the strokes.
- **MS2.** A follower's page opens each screen the presenter opens, with the follower's own data; a screen they may
  not open says so; a guest sees the presenter's tab as pixels.
- **MD.** The desktop app shares a window of another application on Linux, macOS and Windows.
- **MM.** An Android phone shares its screen into a group call that a browser sees within 2 s of consent; ending the
  share or the call ends the projection and its notification.

### 11.2 Tests

- **`moqui-rtc`:** the stage's forwarding (a leg not on the stage is not forwarded; take over; host stop); a screen
  audio track mixed for a phone leg; a screen leg as its participant's share.
- **`moqui-collab`:** the `excalidraw` codec (scene to document and back; concurrent edits to one element by version;
  an order kept by `index`; deleted elements dropped on a version); a guest actor's access.
- **Specs** (`./moq test basic-comms --spec ...`): `Calls.spec.js` for the share mode, the stage and `CallBoard`;
  a `Boards.spec.js` for room spaces, access by membership, the board message and the call's boards; the wiki
  services' type list for `excalidraw`.
- **Client** (`./moq test-client webroot --offline --spec WebrootCall`): the stage frames, the banner, take over, the
  follow frames; `m-whiteboard` mounting and its Yjs binding against a fake socket. Whether headless Chromium can
  capture its own tab with `--auto-accept-this-tab-capture` is a spike in MS1 (*unverified*); if it can, one
  regression client spec shares a tab, else that stays in the acceptance suite.
- **Acceptance:** one browser walk of a meeting with a share, a board on the stage and a guest drawing
  (`test/acceptance/client/`), added in MW2.
- **Real devices:** the desktop app on each OS (MD) and an Android phone (MM), listed with the other live checks in
  [Future-Plans](Future-Plans.md).

## Sources

- [Chrome: privacy-preserving screen sharing controls](https://developer.chrome.com/docs/web-platform/screen-sharing-controls)
  (`preferCurrentTab`, `selfBrowserSurface`, `surfaceSwitching`, `systemAudio`)
- [Chrome: Captured Surface Control](https://developer.chrome.com/docs/web-platform/captured-surface-control)
- [MDN: Element Capture and Region Capture](https://developer.mozilla.org/en-US/docs/Web/API/Screen_Capture_API/Element_Region_Capture)
- [W3C Screen Capture](https://www.w3.org/TR/screen-capture/)
- [Android: Media projection](https://developer.android.com/media/grow/media-projection)
- [CEF issue 3667: getDisplayMedia picker by version](https://github.com/chromiumembedded/cef/issues/3667)
- [Excalidraw](https://github.com/excalidraw/excalidraw), [its npm package](https://npmjs.com/package/@excalidraw/excalidraw),
  [mermaid-to-excalidraw](https://docs.excalidraw.com/docs/@excalidraw/mermaid-to-excalidraw/api)
- [y-excalidraw](https://github.com/mizuka-wu/y-excalidraw)
- [drauu](https://github.com/antfu/drauu)
- [tldraw license](https://tldraw.dev/legal/tldraw-license), [tldraw license key](https://tldraw.dev/sdk-features/license-key)
- [draw.io embed mode](https://www.drawio.com/docs/reference/embed-mode/), [draw.io repository](https://github.com/jgraph/drawio)
