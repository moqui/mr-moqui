# Comms Messaging Plan

Plan, 2026-11-16; revised the same day against the source and the
[BasicComms UI review](ui-review-basic-comms.md) (findings `COM-UI-…`). One plan for the Comms chat as a messaging
app, for one app for all messaging, and for RCS. Most of the phone layout is built (PR #203); the rest is a plan. The
build order is in §18.

In the tables, **Built** means it is in the source today, **Partial** names what is missing, and **Planned** means
this plan adds it. A file and line is where the statement was checked on 2026-11-16.

The goal: one MR Moqui app for all of the user's messaging and calls.
- On a phone it works like a phone messaging app: room list, bubbles, reactions, media.
- On a wide screen it works like a team chat desktop app: sidebar, threads pane, quick switcher.
- It is the user's phone too: their own number for texts, calls and voicemail, through the server's SMS and SIP provider.
- It keeps end-to-end encryption (E2EE) wherever the network allows. It says plainly when a chat or call is not E2EE.
- It replaces GroupMe, iMessage, Google Messages (SMS, MMS and RCS), Signal and Cheogram, and Android's own Phone and
  Contacts apps (§14.7, §14.8). The people on the other side change nothing.
- It keeps every protocol MR Moqui speaks today, so people on XMPP apps, SIP phones, mail and the web reach the user too.

Parts:
1. **The app** (§1–§10): layout, reactions, details, E2EE, notifications, calls, the user's number, SIM texts.
2. **One app for all messaging** (§11–§15): reach to each network, and the equivalent of each feature of each app.
3. **RCS** (§16): what is possible, options and phases. Its research findings are in Appendix A.
4. **Work and order** (§17–§19): new work for other networks, the implementation phases, decisions and open questions.
   Sources in Appendix B.

| Part | Status | Phase (§18) |
|---|---|---|
| §2: phone layout | Built (PR #203, phase 1) except voice notes (phase 7) and the checks on a real phone (§2.3) | 1, done |
| §3: desktop gaps | Planned; the palette and its Rooms source are built for the web | 8 |
| §4: reactions and the message menu | Built (phase 6): one action list, the emoji picker, who reacted and voted by touch, Info; a real phone's feel left | 6, done |
| §5: details tabs and new services | Partial: People, Pinned and Files (All, Pictures) panes are built; the services are planned | 7 |
| §6: E2EE policy and labels | Planned; the call rule and what E2EE by default makes are decided (§19). First sign-in on a new phone still needs a design (§6.5) | 3 |
| §7: notifications | Planned; payload encryption reuses the Web Push code | 2 |
| §8: voice and video calls | Partial: one call button, the media server and TURN are built | 4, 5 |
| §9: the user's own phone number | Partial: most of it is built; the gaps are listed | 9 |
| §10: the phone's SIM texts (`sim-sms`) | Phone only with an opt-in sync, decided; MMS and the synced room need a design | 10+ |
| §11–§15: other networks and app equivalents | Plan; each network needs an adapter or a bridge | 10+ |
| §16: RCS | Research done; business RCS through Telnyx is ready to build | 10+ |

# Part 1: The app

## 1. Where we start

**Built** (BasicComms `Chat.xml` and `Chat.qvue`, `Panel/ChatPanel.qvue`, the mobile app in `crates/moqui-mobile`):

| Area | What exists |
|---|---|
| Messages | Send, edit (24 hours by default: `chat_edit_window_minutes`, 0 for no limit), redact, reply, threads with a thread pane, forward, pins, polls, mentions, drafts, link previews (text only, three a message) |
| Send state | Outbox rows (sending, waiting, not sent, with Retry) and bridge delivery rows (pending, failed, uncertain, delivered). An ordinary message has no "sent" tick. No scheduled send |
| Reactions | `react` transition; aggregated per message as `[{key, count, userIds, mine}]` in the timeline; six quick reactions and a fixed grid of 40 emoji in `Chat.qvue`. Who reacted shows in a hover tooltip only |
| Read state | Read markers (`markRead`, `markAllRead`); read receipts from each member's `lastReadEventSeq` (`prefs.readReceipts`), under own bubbles on a phone |
| Files | `upload`; `files` transition over `find#RoomFiles` (one `contentTypePrefix`, `searchText`, `limit`, `beforeEventSeq`); a Files pane with All and Pictures; an image viewer |
| Search | `search` over `search#ChatEvents` (not for an encrypted room). "Find a room or person" filters the rooms already loaded; it finds no new person |
| Desktop layout | Sidebar with filters (All, Unread, Mentions, Direct, Rooms, Records, Archived), Pinned and Recent groups, unread and mention counts, presence and status, hover toolbar, Up arrow edits. Mod+K is the shell palette, with a `#` Rooms source (web only; the desktop and mobile shell has no palette) |
| Phone layout (PR #203, phase 1) | The room list fills the home; a Start chat button opens a full-screen picker of people (with several, a name makes a private room) and phone numbers; bubbles with own messages on the right; a one-row composer whose Attach offers the camera, the gallery or a file; a long press (450 ms) or right-click opens the message actions as a sheet; a swipe right on a bubble replies; swipe right pins and swipe left archives a room; a long press on a room opens pin, mute, mark read and archive; Back (`moqui.back`) closes a menu or sheet, then a pane or thread, then the room |
| Archive | Per member (`archivedEventSeq`) and per room by an owner (`archivedDate`, read-only) |
| Encrypted chat | Megolm rooms, cross-signing, emoji and QR verification, key backup ([Chat-Keys](../docs/interfaces/Chat-Keys.md)). Opt-in for direct and private rooms when the room is made; never turned on or off later. An encrypted direct room is a separate room from the plain one with the same person. The shell page owns the crypto device, on the desktop and on Android ([Desktop-Client](../docs/ui/Desktop-Client.md#chat-crypto)); on Android its passphrase is sealed by the Android Keystore |
| Calls | WebRTC with an SFU; SFrame E2EE with `requireE2ee` or the room's `callE2eeRequired`, set at the start and never lowered ([WebRTC](../docs/interfaces/WebRTC.md)). One call button in the room header; it always starts with video |
| Phone number | Telnyx SMS and MMS ([SMS](../docs/interfaces/SMS.md)); SIP trunks, `place#PhoneCall`, the Dialer, voicemail ([SIP](../docs/interfaces/SIP.md)). One room per outside number holds its texts, calls and voicemail |
| Mobile push | The server sends FCM data messages of kind `ring`, `ringEnded` and `notification` (every persisted notification). The app shows only the ring (`PushService.kt`, `CallManager.kt`); a `notification` is dropped ([Mobile-Client](../docs/ui/Mobile-Client.md)) |
| Mobile shell | Bundled `Root.qvue`. Home is `Panel/ChatPanel.qvue` at full size with the `home` prop; the shell header hides while a room is open. A room opens `Chat.qvue`'s `chat-room` in `compact` mode with bubbles |
| Mail | The Comms Inbox: accounts and identities, folders and labels, threads, To, Cc and Bcc, reply, reply all (with Reply-To) and forward with a copy of the attachments, drafts, undo send, uploads that resume, Save to Files and Attach from Files, saved searches, snooze, Spam and Trash, block and unsubscribe, invitations, text blocks (private or shared with a group), ordered rules with stop, automatic replies with a time window, shared mailboxes with read, send and manage rights, removal of an account with its mail kept, `.eml` and print output. This plan must keep all of it working (COM-UI-019, 022, 024, 029) |

**Components in `Chat.qvue`** (2941 lines, one file): `ChatRoom` (room view, thread pane, details),
`ChatMessage`, `ChatComposer`, `ChatEdit`, `ChatPending`, and the root (sidebar plus room). They are local to the
file. `ChatPanel.qvue` already shares the room: it loads `Chat.qvue` with `moqui.loadComponent` and takes its
`chat-room` (`ChatPanel.qvue:290`). It copies the list helpers instead of sharing them (`ROOM_ICONS`, counts, snippet,
time format: `ChatPanel.qvue:152-163`).

**Layout thresholds as built** (`Chat.qvue:1300-1303`, `2613`):

| Name | True when | Effect |
|---|---|---|
| `stacked` | The screen is under 1024 px (Quasar `md`) | The room list is the whole screen; a room replaces it |
| `narrow` | The room is `compact`, or its own width is under 640 px | The header actions move into the menu |
| `useBubbles` | `narrow`, or the parent asks (`stacked`, the phone home) | Bubbles; the long-press sheet |
| `sideOverlay` | `compact`, or the room is under 900 px wide | A details pane replaces the timeline, with a back arrow |

**Gaps on a phone that remain:**
- No voice notes.
- Who reacted and who voted show only on hover, so not on touch.
- The Files pane has only All and Pictures. No links, voice, or reactions list.
- The call button always starts a video call.
- No push for chat messages: a message only shows when the app is open.

## 2. Phone layout

### 2.1 Design

The width selects the layout, with the thresholds in §1. They stay as built: the phone layout is the `stacked` Chat
(a screen under 1024 px) and the phone home; a room under 640 px wide, in any place, shows bubbles.

| Area | Phone layout | Status |
|---|---|---|
| Home | The room list at full size, not a card. Search bar on top. Start chat button (FAB) at the bottom right | Built (`Root.qvue:154`, `ChatPanel.qvue:102`). The search bar shows only with more than six rooms |
| Room row | Avatar, name, last message, time, unread badge, E2EE or SMS icon | Partial: no E2EE icon (§6); a phone-number room has the SMS icon (`CrtExternal`, phase 0) |
| Room row actions | Swipe right: pin. Swipe left: archive. Long press: pin, mute, mark read or unread, archive | Built (`ChatPanel.qvue:56-60`, `118-134`) |
| New chat | Full-screen picker: search people and phone numbers, select one or several, Start. A name makes it a room | Built (phase 1): `searchUsers` with `numbers=true` offers a typed number, `textRoom` opens its room, a name with two or more people calls `createRoom` (`moqui.chat.startChat`) |
| Room header | Back, avatar, name, E2EE status icon (§6), call and video buttons, menu | Partial: one call button; the E2EE icon is planned |
| Messages | Bubbles. Own on the right in the primary color. Others on the left with the avatar in groups. Day labels. Group by sender within `GROUP_MILLIS` (5 minutes) | Built. The day label is a line with a label, not a chip |
| Reply | Swipe right on a bubble. The quote shows above the composer and above the sent bubble. Tap a quote to jump | Built (phase 1): past 64 px, in the room's timeline |
| Composer | Fixed at the bottom, above the keyboard: attach (camera, gallery, file), emoji, text, then a send button that becomes the voice button when empty | Partial: one row with attach (camera, gallery, file), emoji, poll and Send. `MainActivity.kt:52-57` pads the page for the keyboard; check it on a device. The voice button comes with phase 7 |
| Back | Back closes the open layer: menu or sheet, then a details pane or thread, then the room | Built (phase 1): `moqui.back`; Android back asks it before the WebView's history |

### 2.2 Work

Steps 1 to 7 are built (phase 0 and phase 1); the voice button of step 5 comes with phase 7.

1. **Shared helpers.** `moqui.chat` in `webroot/js/WebrootChat.js` (the shell already loads it) gets the helpers that
   `Chat.qvue` and `ChatPanel.qvue` each have a copy of: the room icon, the unread and mention counts, the snippet,
   the layout thresholds, and the time and day labels on `moqui.l10n`'s zoned helpers. `Chat.qvue`, `ChatPanel.qvue`
   and `Root.qvue` (`when`, line 428) use them, so a time reads the same everywhere (COM-UI-007).
   - No component moves to a new file. The shell has no template compiler, and `bundled-screens.txt` takes only
     `.qvue` files, so a plain `.js` file with templates would not render in the app. A new component that other
     screens can use (the emoji picker, §4) is an `m-` component in `webroot/js`, added to the list of
     `client_components_are_precompiled`. A chat-only template stays in `Chat.qvue`.
2. **Back layers.** One registry in the page, `moqui.back`: an open sheet, menu, details pane, thread or room pushes a
   function that closes it, and takes it off when it closes another way. `moqui.back.pop()` closes the top one and
   says if there was one.
   - Esc calls it first (`closeTopmost`, `WebrootVue.qvt.js:10022`, and Chat's own Esc at `Chat.qvue:2546`).
   - The shell's Back button and `goBack` (`Root.qvue:365`) call it before the shell's own `backStack`.
   - Android: `WryActivity`'s back callback asks the page (`moqui.back.pop()`) before the WebView's history and before
     it leaves the activity. At the home with nothing open, back leaves the app as now.
   - On the web, a room and a thread are already in the browser history (`Chat.qvue:2730`, `2741`); panes and sheets
     are not, and browser Back does not close them. That stays.
   - The room's Back and a pane's Back both read "Back" today. Each names where it goes ("Back to chats", "Back to
     the room").
   - As built: the layers are the message sheet, an edit, the side pane or thread (`Chat.qvue`), and the room, the
     picker, a room's actions and the archive of the chat panel (`ChatPanel.qvue`, the phone's home). A room of the Chat
     screen is not a layer: it is in the history (the browser's, or the shell's, `moqui.back.base`). The shell's drawer
     over a narrow window is a layer. A link followed while a layer closes by Back (`moqui.back.closing`: a thread) is a
     step back in the shell's history, so the next Back does not open the thread again; any other link is a new entry.
     What is in front closes first: with the tray open (or a floating window focused), Esc closes its layers, then the
     tray, never a layer of the screen behind.
   - As built on Android: Tauri's `TauriActivity` turns off `WryActivity`'s back callback (`handleBackNavigation` false)
     and handles back in its app plugin, so the app's `MainActivity.kt` adds its own callback after Tauri's, once the
     webview is made; when the page closes nothing, Tauri's back goes on.
3. **Swipe to reply** on a bubble: a touch move to the right past a threshold starts a reply; it cancels the long
   press, as a move does today (`Chat.qvue:359`).
4. **Picker.** A phone number starts or opens the number's room (`CrtExternal`) where the user has a number to text
   from. With two or more people, an optional name makes a private room (`createRoom`) instead of a group direct chat.
5. **Composer.** The Send button keeps only its icon in the `stacked` Chat too, not only in `compact` (COM-UI-002;
   `Chat.qvue:580`), with its accessible name. Attach offers camera, gallery and file on a phone. The voice button
   and voice notes come with the Voice tab (§5).
6. **Room rows.** The home list gets the `CrtExternal` icon; `setNotification` returns `archivedEventSeq`, which
   `ChatPanel.qvue:245` already reads.
7. Regenerate the bundled screens (`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`).

### 2.3 Done when

| Check | How it is checked |
|---|---|
| At 390×844 the list fills the screen, and Start chat opens the full-screen picker | Acceptance client spec (a phone-width check is an acceptance test) |
| A direct chat with one person, a named group with two, and a text to a number can be started from the phone | Acceptance client spec `PhoneChat`, on a stand-in server; the real `searchUsers` and `textRoom` path is covered by `BasicCommsTexts` |
| Bubbles show own messages on the right; a list time and a room time agree for a user whose zone is not the browser's | Regression client spec for the time; the rest in the acceptance spec |
| Back closes a sheet, then a pane, then the room, and does not leave the app from inside a room | Regression client spec for `moqui.back`; the Android gesture on a real phone |
| The composer stays above the keyboard; a long press vibrates | Real phone. The manifest has no `VIBRATE` permission, so `navigator.vibrate` may do nothing in the app; if so, add a host command |
| The desktop layout at 1440×900 looks and works as it did before | The existing regression specs still pass |

The client test kit has no touch emulation (`test_client/browser.rs:689` sends `mobile: false`), so a spec drives the
long press and the swipe with touch events it makes itself, or with `contextmenu`.

## 3. Desktop gaps

The desktop layout exists (§1). Add:

| Item | Work | Status |
|---|---|---|
| Quick switcher | Mod+K is the shell palette (`WebrootVue.qvt.js:10010`), and BasicComms gives it a Rooms source with the `#` prefix (`BasicComms.xml:34`, `search#PaletteRooms`). Do not add a second Ctrl+K listener (COM-UI-006). Add: people the user has no room with (choosing one starts the direct chat), phone-number rooms, and rooms without the prefix while Comms is the current app. Bring the palette to the desktop and mobile shell, which has none (`Root.qvue`) | Partial |
| Activity view | A sidebar entry: mentions, reactions to my messages, and replies to my messages and in my threads, newest first. New service `find#ChatActivity` (§5) | Planned |
| Threads view | A sidebar entry for all threads I am in. `find#RoomThreads` with Mine works in one room only (`ChatServices.xml:2196`); across rooms the view reads the thread replies of `find#ChatActivity` | Planned |
| Sidebar sections | Fold the groups. Add groups: Phone (rooms of type `CrtExternal`) and Bridged. Today the groups are Pinned and Recent, and a phone room shows only under All | Planned |
| Mail on a person's row | Where the user has a direct chat with a person, that person's latest mail thread shows on the row as a second line ("Email: subject") and opens in the Inbox, with "Email is not end-to-end encrypted" (§6.1 rule 7). No mail-only rows; mail stays in the Inbox (decided, §19). A user of this server matches by the mail address of their account; an outside person waits for unified contacts (§11). Also on the phone home | Planned |

**Done when:** in Comms, Mod+K, two letters and Enter open a room, on the web and in the desktop app. The Activity
view lists a mention and a reaction from another user (regression service spec for the feed; the views in an
acceptance client spec).

## 4. Reactions and the message menu

| Item | Phone | Desktop | Status |
|---|---|---|---|
| Open the menu | Long-press a bubble (450 ms, with a short vibration) | Hover toolbar and right-click | Built (phase 6): a right click on a desktop row opens the menu at the pointer; on a link, an editor or selected text the browser's menu stays |
| Menu | Quick reaction bar with `QUICK_REACTIONS` and "+", then Reply, Reply in thread, Copy text, Copy link, Forward, Edit, Pin, Mark unread from here, the apps' actions, File on a record, Info, Delete | Same list | Built (phase 6): one list (`chat-message-menu`, `messageActions`) for the sheet, the hover toolbar's More (its quick reactions are on the toolbar) and the right-click menu |
| Full picker | A searchable emoji picker with categories and recent emoji (kept in `storage`) | Same | Built (phase 6): `m-emoji-picker` (`WebrootEmoji.js`) over `libs/emoji/emoji.json` (unicode-emoji-json 0.9.0, MIT, Emoji 15.1 and earlier); recent emoji per user under the storage scope; also the composer's |
| Who reacted | Long-press a reaction chip: a list of people. A tap still adds or removes my own | Hover a chip | Built (phase 6), and the same for who voted on a poll option; the tooltip is for a pointer only |
| Info | See below | Same | Built (phase 6): `moqui.chat.messageInfo`, the message read again (`timeline?eventSeq=`); the terminal's per message (`ChatTui.xml`, `infoSeq`) |

**Info** (COM-UI-008) shows only what the server knows:

| Line | Meaning | Source |
|---|---|---|
| Sent | The server stored the message, with the time | The event |
| Delivered | Only for a bridged message (SMS, another network): accepted, failed or uncertain, per network | `ChatBridgeDelivery` |
| Read by | Each member whose read marker is at or after this message, with "and N more" | `lastReadEventSeq` |
| Not shown | Delivery to a member's device: the server does not know it, and a push acknowledgement (§7) is not it | — |

- A member with read receipts off is not listed, and a viewer with receipts off sees no "Read by" (as the reader
  avatars do today, `Chat.qvue:1332`). The line then says receipts are off; it does not say "not read".
- A read marker is a position, not a per-message record: "Read by" means "has read up to here or later".
- In a thread, Info shows Sent and Delivered only; threads have no read markers.

**Done when:** on a phone a user can react, remove a reaction, see who reacted, and open Info, all by touch
(acceptance client spec with made touch events; a real phone for the feel). A regression spec covers the Info rules:
receipts off on either side, and a bridged message's delivery line.

## 5. Details tabs and new services

### 5.1 Tabs

The room's details (full screen on a phone, the right pane on a desktop) become one view with tabs. Today Members,
Files, Pinned, Threads and Search are separate panes opened from the header or the menu (`Chat.qvue:941-1082`);
Threads and Search stay panes of their own. Each tab loads more as the user scrolls.

| Tab | Source | Status |
|---|---|---|
| People | Members | Built as a pane |
| Media | `files` with `image/` and `video/` together. Grid of thumbnails. Full-screen viewer with swipe | Partial: "Pictures" is `image/` only; the viewer has no swipe |
| Files | `files`, all types except image, video and audio | Partial: "All" includes images |
| Voice | `files` with `audio/`. Inline player. Voice notes are recorded in the composer (`MediaRecorder`) and sent as an audio attachment | Planned |
| Links | New `links` transition (§5.2) | Planned |
| Reactions | New `reactions` transition (§5.2) | Planned |
| Pinned | Pins | Built as a pane |

- `find#RoomFiles` takes one `contentTypePrefix` today. It gets a list of prefixes to include and a list to leave out.
- In an encrypted room the server knows no file type or name (`find#RoomFiles` returns them empty and refuses a
  filter). The client loads the files, decrypts their details and sorts them into the tabs itself.
- People: when the last owner leaves, the confirmation names the member who becomes the owner and says so
  (COM-UI-009). The server picks the longest-standing moderator, else member (`remove#Member`,
  `ChatServices.xml:1823-1833`). The event of that change names the leaving owner as the one who made the other an
  owner; it should read as an automatic change.

### 5.2 New services (`org.moqui.chat.ChatServices`, next to `find#RoomFiles`)

All are read-only. All check that the user can read the room, the same check as `find#RoomFiles`. `limit` is 1 to
100, default 40.

**One paging rule for all of them, and for `find#RoomFiles` and `find#RoomThreads`** (COM-UI-005, COM-UI-010):
- A page is cut only between messages, never inside one. A message with more items than `limit` returns all of its
  items, so a page can be longer than `limit`.
- The cursor names the last message of the page, and the next page starts after it. Rows left out after the query
  (a redacted message) do not end the list early: the service says there is more by the cursor, not by the row count.
- `find#RoomFiles` breaks this today: with `limit` 1 and a message with two files, the second file is never returned
  (`ChatServices.xml:3524-3537`). `find#RoomThreads` pages by a strict "earlier than" on `threadLastDate`, so threads
  with the same date at a page end are skipped. Both are fixed first (§18 phase 0).

| Service | In | Out | Source |
|---|---|---|---|
| `find#RoomLinks` | `chatRoomId`, `searchText`, `limit`, `beforeEventSeq` | `links`: `eventSeq`, `url`, `title`, `siteName`, `senderUserId`, `sentDate`; `nextBeforeEventSeq` | New entity `ChatEventLink` (room, event, link number, URL, URL hash), written when a message is sent, joined to the `ChatLinkPreview` cache by the hash for the title. Newest first |
| `find#RoomReactions` | `chatRoomId`, `reactionKey`, `onlyToMe`, `limit`, `beforeReactionSeq` | `messages`: `eventSeq`, `snippet`, `senderUserId`, `reactions` (the aggregated form); `nextBeforeReactionSeq` | Reaction events that are not taken back, grouped by the message they point to, ordered by each message's newest reaction. The cursor is that reaction's own `eventSeq`, not the message's |
| `find#ChatActivity` | `kinds`, `limit`, `cursor` | `items`: `kind` (`mention`, `reaction`, `reply`, `threadReply`), `chatRoomId`, `eventSeq`, `actorUserId`, `snippet`, `date`; `nextCursor` | New entity `ChatUserActivity`, below. Only rooms the user can still read |

Why these sources:
- `ChatLinkPreview` is a cache keyed by the URL's hash, with no room or event, so it cannot list a room's links. A
  message's previews are in `ChatEvent.linkPreviewJson`: three at most, none when the fetch failed or the sender
  removed the previews. `ChatEventLink` lists every link. No image is kept, by design, so there is no `imageUrl`.
- `eventSeq` counts within one room, and a mention row has no date, so a feed across rooms cannot page by
  `beforeEventSeq`. **`ChatUserActivity`** has one row for each thing a user should hear about: user, kind, room,
  event, actor, date. `send#Message` and `react#Message` write it in the message's transaction:
  - `mention`: the user is mentioned (the rows `ChatEventMention` has today).
  - `reply`: a reply to the user's message.
  - `threadReply`: a reply in a thread the user started or wrote in.
  - `reaction`: a reaction to the user's message.
  - A redaction and the retention job remove its rows. It has an index on user and date.
  - The cursor is one opaque text of date, room, event and kind, so rows with the same date keep their order.
  - The same rows are the "Mentions" level of push (§7.2 rule 1), so the rule is written once.

Encrypted rooms: the server cannot read links or snippets. `find#RoomLinks` returns nothing for them, and the client
finds links in the messages it has decrypted. `find#RoomReactions` cannot filter by `reactionKey` (the key is
encrypted). It and `find#ChatActivity` return the event and reaction only, and the client decrypts the snippet.

Tests: a regression service spec for each one: paging across a message with more items than the limit, equal dates,
a room the user cannot read gives no rows, an encrypted room. One for the `find#RoomFiles` fix. Full scrolling walks
are acceptance specs.

**Done when:** each tab shows the right items for a test room with photos, a video, a PDF, a voice note, two links
and three reactions, on a phone and on a desktop (acceptance client spec).

## 6. E2EE policy

### 6.1 Rules

1. Every room and call shows its E2EE status in the header and in the room info.
2. A room that is not E2EE says why, in one short sentence (§6.2).
3. Verified people get a check mark. A new device of a verified person shows a warning before the next send.
4. Every call in an encrypted room is E2EE (SFrame), in a direct room too. A device that cannot do it (a SIP phone, a
   guest, a browser without the transform, a user with no crypto device) is refused with the reason, and the call
   rings on for the others. A call's encryption is set at its start and never lowered: there is no fallback to a
   plain call, with or without the user's agreement. A plain call is started only from a plain room. (Decided, §19;
   COM-UI-004.)
5. New direct rooms and private groups are encrypted by default when every person in them has a crypto device
   (decided, §19; the rest of the design is §6.5).
6. A lock icon means E2EE and nothing else. A private room (members only) that the server can read does not show a
   lock (COM-UI-001).
7. Mail is never shown with a lock or a chat E2EE label. Where a person's mail shows beside their chats, it says
   "Email is not end-to-end encrypted" (COM-UI-019).

Rules 1 to 4, 6 and 7 are built (phase 3, `fdd1ab597`, and its review, `1de64af65`): the labels of §6.2 in the room, its
lists, its members pane and settings; rule 3's checks by a verified person (Members, their messages), verifying another
person by emoji in Chat, and the warning before a send to a verified person's new device or new identity (Verify, or
Send anyway); rule 4 on the server and in Chat's call. Rule 7: no lock in Mail; the "Email is not end-to-end encrypted"
line waits for the place a person's mail shows beside their chats (phase 8, the `mail` state of the label). Rule 5 is
phase 10.

Server work for rule 4 (built):
- `roomRequiresE2ee` read only the room's `callE2eeRequired`. It now also returns true for a room with `encrypted`.
  The room's call setting shows on and fixed in an encrypted room, and `set#RoomCallEncryption` refuses to turn it off
  there.
- `set#RoomCallEncryption` refuses a direct room; a plain direct room keeps the per-call choice ("start an encrypted
  call").
- The refusals exist and stay: `callNotEncrypted`, `unsupportedClient`, `noCryptoDevice` (`Calls.js:717-722`,
  `749-754`).

### 6.2 Labels

This is the one table of labels. §16.5 adds only the RCS rows. Built in phase 3 (`fdd1ab597`): `moqui.chat.e2eeLabel` in
`WebrootChat.js` answers each state (the call states too, for the call screen of phase 4); the room's header, banner,
start and composer, Chat's rail and the home list use it, and the terminal shows it as one line.

| State | Icon | Banner at the top of the room | Composer hint |
|---|---|---|---|
| Encrypted, all devices verified | Lock with check | "Messages and calls are end-to-end encrypted. Only people in this chat can read them." | — |
| Encrypted, not all verified | Lock | Same, plus a "Verify" link | — |
| Plain private room or plain direct chat | Open lock | "Not end-to-end encrypted. The server can read these messages." | — |
| Public channel | Open lock, no banner | — | — |
| SMS or MMS | Open lock | "Not end-to-end encrypted. Texts go through the phone network and the SMS provider." | "Text message" |
| Bridged network | Open lock | "Not end-to-end encrypted on this server. The bridge reads messages to pass them on." | The network's name |
| Phone call | Open lock | "Phone calls are not end-to-end encrypted." | — |
| Encrypted call | Lock | "This call is end-to-end encrypted." | — |

- The room type has its own icon in the room list (person, people, `#`, record, SMS), separate from the lock. Today a
  private room's type icon is a lock (`ROOM_ICONS.CrtPrivate`, `Chat.qvue:234`); it becomes a people icon.
- The label state is one function in `moqui.chat` (§2.2), from the room's `encrypted` flag, its type, its bridges and
  the devices' trust. The header, the room info, the room list, the call screen and a notification all use it.
- A bridge's label comes from its adapter: `Capabilities` (`chat_bridge.rs:110-119`) gets an `encryption` field with
  the values `None`, `Transport`, `BridgeDecrypted` and `EndToEnd`, and `get#BridgeCapabilities` returns it.

States the review found without a label (both built: COM-UI-013 in phase 3, `fdd1ab597`; COM-UI-011 in phase 0):
- **A recipient who cannot read a message.** With "verified devices only" on, a message goes out and looks sent, and
  a member with an unverified device gets "missing key" (COM-UI-013). The sender sees who cannot read it, with a
  Verify link. The recipient's text says the sender has not verified this device; it is not the "restore from backup"
  text.
- **Search in an encrypted room.** The header offers Search, and the server refuses after the query
  (`ChatServices.xml:4042`; COM-UI-011). The search pane says first that an encrypted room is not searched on the
  server.

### 6.3 By network

| Network | E2EE | Why not, when not |
|---|---|---|
| MR Moqui chat | Yes, Megolm, for encrypted rooms | Plain rooms and public channels: the server reads them, so search and agents work |
| MR Moqui calls | Yes, SFrame, for an encrypted call: every call in an encrypted room (rule 4), or when the caller or the room's call setting asks | Guests have no crypto device yet (Future-Plans); a SIP phone cannot |
| XMPP | OMEMO between XMPP apps only | The server keeps OMEMO as it came. Megolm rooms are not served over XMPP |
| Matrix (C6a, C6b) | Possible, Megolm end to end | Only if the room stays encrypted across the bridge. Today an encrypted room is never bridged (`ChatBridgeServices.xml:953`), so this needs its own design |
| SMS, MMS, phone calls | Never | The phone network and the provider read them |
| RCS | See §16 | Depends on the path |
| WhatsApp (C4) | On WhatsApp's side only | The bridge on the server decrypts and encrypts again |
| Telegram (C4) | No | Bot and normal chats are not E2EE |
| Mail | No (TLS in transit only) | Mail servers read it |

### 6.4 What encrypted rooms give up

The UI explains this when the user picks an encrypted room:
- Server search, link lists and link previews.
- Polls, forwards, bridges, agents and terminal send.
- Phones, guests and call recording in its calls (rule 4).
- The message text in a notification on a phone, until the app is opened (§7.4).
- The server still sees who is in the room, when each person sends, and which message a reply or reaction points to.

### 6.5 Design needed before E2EE by default

- **Rooms.** Today an encrypted direct room is a separate room from the plain direct room with the same person
  (`directKey` with an `e2ee:` prefix, `ChatServices.xml:1428`), and encryption cannot be turned on or off after the
  room is made. Decided (§19): `createDirect` and a new private group make an encrypted room when every person in
  it has a crypto device, else a plain room with the plain banner. The plain direct rooms that exist stay. When a
  person has both, the picker opens the encrypted one.
- **Device verification.** Emoji verification stalls today: after Compare emoji, no emoji show on either device
  (COM-UI-012, P1). Fix it before any label or default depends on "verified" (§18 phase 0).
- **Android.** The crypto device already runs in the Android WebView (the shell page, as on the desktop), and its
  store passphrase is sealed by the Android Keystore. What is open is the check on a real phone
  ([Exit checks, MD6](Mobile-Desktop-Exit-Checks.md#md6-the-android-app)), and the My devices screen, which is 907 px
  wide on a 390 px phone (COM-UI-014).
- **First sign-in on a new phone.** Restore from key backup with the recovery key, or verify from another device.
- **Search on the device.** The offline store is plain SQLite and relies on the OS disk encryption
  ([Desktop-Client](../docs/ui/Desktop-Client.md)). Decided (§19): no search in an encrypted room at first, and the
  search pane says so before a query. Later, search in memory over the messages the device has decrypted. Nothing
  decrypted is written to disk for search.
- **Message text in a phone notification.** The crypto store is the WebView's, with one owner, so native code cannot
  decrypt a push. Decided (§19): a notification for an encrypted room shows the room and sender only (§7.4). No
  hidden WebView and no crypto store in Rust is planned for it; revisit only if daily use asks for it.

**Labels done when:** each state in §6.2 shows on a phone and on a desktop, and the SMS room shows the SMS banner
(acceptance client spec `ChatLabels`; the call states on the call screen come with phase 4). A regression spec covers rule 4: a call in an encrypted room is encrypted, a phone and a
guest are refused, and an encrypted direct room's call is encrypted.

## 7. Notifications

A message the user should see reaches every device of theirs: a phone with the screen off, after a reboot, with the
app closed, and a desktop. A message already read on another device stops showing. §7.2 says exactly what is
promised, and what happens when a push cannot be sent.

### 7.1 Today

The server side of phase 2 (steps 2a to 2d, 2h) is built (2026-11-17): the rows below marked "since phase 2". The
Android app (2e), the desktop and mobile `notify` (2f), the check screen (2g) and the app's UnifiedPush (2h) are built
too (2026-11-17).

| Path | What it does |
|---|---|
| Chat socket | Each event goes to members' open pages (`chatRoom:<id>`). Nothing when the app is closed |
| Mentions | Since phase 2: the framework seeds the `ChatMention` topic (`ChatSetupData.xml`), so `notify#Mention` stores every mention in the list. It is not pushed by `send#WebPush`; the chat push worker pushes it with its message (no double push) |
| Activity | Since phase 2: `ChatUserActivity` (mention, reply, thread reply, reaction), written by `send#Message`, `edit#Message` and `react#Message` in the message's transaction, removed by a redaction, a reaction taken back and the retention. The feed service (`find#ChatActivity`) is phase 7 |
| Direct and room messages | Since phase 2: pushed by the chat push worker (`service/chat_push.rs`, `ChatPushServices.xml`): `append#Event` writes a `ChatPushJob` in the transaction; after the commit the worker applies rules 1, 3 and 4 and writes a `PushOutbox` row per device, merged per device and room with a count; a missed call and a voicemail push as messages in their room (rule 6) ([Qvt-Client, Chat push](../docs/ui/Qvt-Client.md#install-and-web-push)) |
| Web Push | Title, link, topic and tag, encrypted to the browser (RFC 8291). Never the body, but Mantle's `ChatMention` title holds up to 120 characters of the message for a user with previews on (since phase 2 that topic is not pushed by `send#WebPush`). Disabled after 404, 410 or 5 failures in a row. A chat push to a browser is the encrypted chat payload, with TTL, `Urgency` and a `Topic` per room, and retries |
| FCM | Data messages of kind `ring`, `ringEnded` and `notification`, and since phase 2 `chat` and `clear`. Since phase 2 a device gives its push keys at `register#PushDevice` and a `notification` or chat push carries only `{kind, pushId, enc}` (encrypted, COM-UI-031); a device with no keys gets no text. `send#WebPush` tries three times in the request; the chat outbox retries 429 and 5xx with backoff, jitter and `Retry-After`, at most an hour a wait, 12 tries or the 4-week TTL, and deletes a registration FCM says is gone. Since 2e a ring to a keyed device is sealed too, and the app opens `enc` in Rust (`Native.openPush`) and shows a chat push, a `clear` and a test in Kotlin with no WebView (`ChatNotifications.kt`), then acknowledges it. The registration is tied to the device key |
| UnifiedPush | Since phase 2: `register#PushDevice` with `provider=unifiedpush` keeps a distributor's endpoint (`PushUnified`, keys required); the worker and `send#WebPush` post the same encrypted payload to it as Web Push. The Android app registers an endpoint per account with its distributor (`org.unifiedpush.android:connector`) and opens its messages with that account's own keys |
| Order | Since phase 2 the push of a message and the call's ring and its end go after the commit (`registerOnCommit`, COM-UI-032); the mention notification is still stored in the transaction (force-cache), but not pushed |
| Settings | Room level `CnlAll`, `CnlMentions`, `CnlNone`; `CHAT_NOTIFY_DEFAULT`; `CHAT_NOTIFY_PREVIEW`. A muted room counts and notifies as Mentions (`ChatEntities.xml:107`) |
| Do not disturb | A server job sets and clears the `dnd` status by the user's hours every 5 minutes (`apply#DndHours`). Since phase 2 the push worker holds a user's chat pushes while the status is `dnd` and sends one summary per room and device when it ends, and a call rings the devices only from a person on the user's allowed list (`CHAT_DND_ALLOWED`, Settings, Chat) or as the same person's or number's second call within 3 minutes, on the apps and the desk and soft phones alike (`filter#RingUsers`). The client still silences toasts and sounds itself (COM-UI-015) |
| Health | Since phase 2: each device keeps sent, acknowledged and failed counts and its last registration, push and acknowledgement (`ack#Push`, `/qapps/ackClientPush`); System, Push Health lists them, the devices with unacknowledged pushes of the last day first |

### 7.2 Rules

1. **Who gets a push.** Each member except the sender, by the room level:
   - All: every message.
   - Mentions: the user's `ChatUserActivity` rows (§5.2) of the kinds mention, reply and thread reply. Replies are
     not tracked today.
   - None: nothing.
   - A muted room counts as Mentions, as built: ordinary messages are silent and a mention still notifies. (Decided,
     §19.)
   - Direct rooms start at All; the user can change the level.
2. **When.** After the commit, and only then. No batching delay on the server. The phone groups notifications by room.
3. **Not when read.** The worker checks the user's read marker when it sends: a user who has read up to the event on
   any device gets no push. When the read marker moves (`chatUser` `read` change), the server sends a `clear` push, so
   other devices remove that room's notification. A push can still arrive just after a read on another device; the
   `clear` follows it.
4. **Do not disturb.** The worker holds a user's pushes while their status is `dnd`, set by hand or by the hours job,
   and sends one summary for each room when it ends. A call still rings when it is from a person on the user's
   allowed list (a new list for each user, in Chat settings beside the hours), or when it is the same person's
   second call within 3 minutes. (Decided, §19.)
   - A reaction to the user's message never pushes; it shows in Activity (§3) and on the message. (Decided, §19.)
5. **Content privacy.** The push carries no readable text. The server encrypts a small payload (room, sender name,
   and the snippet when the room is plain and `CHAT_NOTIFY_PREVIEW` is not N) with a key that belongs to the device,
   with the Web Push code the server has (`notify/push_crypto.rs`, RFC 8291). Google and Apple see only that a push
   came. This also ends the clear-text title of today's FCM `notification`. An encrypted room: room and sender name
   only (§7.4).
6. **Texts and calls.** A text from an outside number pushes like a direct message. A call rings through the existing
   ring path, after the commit. A missed call and a voicemail push as messages in the number's room.
7. **Every device.** Each registered device of the user gets the push: phones by FCM, browsers by Web Push, the desktop
   app by its socket while running.
8. **What is promised.** Each push is tried until the device's push service takes it, the push's time to live ends,
   or the tries run out. A push that ends without being sent is counted and shown (§7.3 Health), and the app shows
   what it missed when it next opens (§7.5). An acknowledgement from the app means the device got the push. It does
   not mean the person read the message.

### 7.3 Server work

| Item | Work |
|---|---|
| Topics | The base components seed the `ChatMention` topic, so a mention is stored without an add-on. The stored notification stays for the notification list; its push goes through the new path only, so a mention is not pushed twice |
| Fan-out | `append#Event` writes one job row for the event (`ChatPushJob`: room, event) in its transaction and wakes the worker after the commit, as it does for bridges (`wake#BridgeDelivery`). The worker reads the members and applies rules 1 to 4 then, and writes one `PushOutbox` row for each device. A message in a room of 5,000 members writes one row in the sender's transaction, not thousands |
| Durable queue | `PushOutbox`: device, payload, attempts, next try, status. A restart loses none. Pending rows for the same device and room merge into one, with a count |
| Worker | New `crates/moqui-core/src/service/chat_push.rs`, on the pattern of the bridge delivery worker (`chat_bridge.rs`): a row is claimed with a lease, a failure waits with backoff, a sweep picks up rows a crash left, and `run#…` runs one round at a given time for the tests |
| Retries | FCM 429 and 5xx: retry with exponential backoff and jitter, up to 1 hour, with FCM's `Retry-After`. FCM "gone" (404 `UNREGISTERED`): delete the registration. Web Push 404 or 410: disable, as now. Each device fails on its own |
| TTL and collapse | Messages: TTL 4 weeks, collapse key per room so a phone offline for a week gets one per room, not hundreds. Calls: TTL = ring time, sent directly, not through the outbox |
| Priority | High priority for messages that will show a notification, normal for `clear` and silent updates (Android removes high priority from apps whose high-priority pushes show nothing) |
| Device key | `register#PushDevice` gets the device's push public key and auth secret (`WebPushSubscription` has the fields). Payloads are encrypted per device |
| Acknowledgement | The app reports each push it showed (`ack#Push`, push id). The server keeps sent, acked and failed counts per device |
| Health | System screen: per device: last registration, last push sent, last ack, failures. Alert when a device has had no ack for pushes sent in the last day |

### 7.4 Android app work

| Item | Work |
|---|---|
| Show without the WebView | `PushService` decrypts the payload and shows the notification in Kotlin, as the ring does today. It must not wait for the WebView or a network fetch. Today a `notification` push is dropped |
| Encrypted rooms | The notification shows the room, the sender and "New message". The text shows when the app is opened. Native code cannot decrypt: the crypto store belongs to the WebView (§6.5). (Decided, §19) |
| Channels | Messages, Mentions, Texts, Calls (full screen), Missed calls, Other, beside the two that exist (`calls-ring`, `calls-ongoing`). The user sets sound and vibration per channel in Android settings |
| Style | `MessagingStyle` with the sender's avatar and name. One notification per room, grouped under a summary. Conversation shortcuts so Android shows them as conversations (bubbles, priority) |
| Actions | Reply (inline text) and Mark read go through the Rust outbox, which already sends with no WebView (`chat-send`, `chat-read`; `FlushWorker.kt`), and work offline. In an encrypted room Reply opens the app, since the message must be encrypted by the page. Mute 1 hour. Tap opens the room at the message |
| Clear | The `clear` push and a read in the app remove the room's notification |
| Permissions | Ask for `POST_NOTIFICATIONS` (Android 13+) on first sign-in, with a reason. Ask for full-screen intent (Android 14+) for calls |
| Battery | Detect battery optimization and maker task killers. Show a one-tap fix to allow the app to run in the background |
| Token | Register the FCM token at every start and on `onNewToken`, and retry until the server answers. Today a failed registration waits for the next start, token or sign-in |
| Device key | The app makes the push key pair and keeps the private key sealed by the Android Keystore, as the device keys are (`KeyVault.kt`) |
| After reboot and update | Nothing to start: FCM delivers to a stopped app. Test it (§7.6). A force-stopped app gets nothing until it is opened; the app says so |
| Without Google Play | A UnifiedPush distributor (for example ntfy) where the phone has no Google Play. Same encrypted payload, same outbox; the device registers its endpoint as it registers an FCM token. In the first release, in phase 2 (decided, §19) |

### 7.5 Desktop and browser

- The desktop app shows native notifications from its socket, with the same rules (§7.2) and the same `clear`. Its
  `notify` command has a title, body and link today; it gets a tag, so a room's notification can be replaced and
  removed. The mobile `notify` drops the link and tag; it keeps them.
- On start and on resume it reads the changes feeds and shows notifications for unread messages it missed.
- Browsers keep Web Push. The payload gets the room and sender, still no body.
- One device shows the notification at a time where possible: a page in focus marks the room read, which clears the
  others (rule 3).

### 7.6 Notification check screen and tests

**In the app:** the check screen is in the shell (the account view of `Root.qvue`), not under Comms Settings.
Settings is a server page, and a server page has no device commands; the shell gets a new host command for the state.
- A checklist: permission given, channels on, battery optimization off, token registered, last push received.
- "Send a test notification": the server sends one through the full path. The screen shows the time it took.

**Automated tests:**
- Server (regression, with the worker's simulated clock): rules 1 to 4 (levels, mute as Mentions, read on another
  device, DND), no push for a rolled-back message, outbox retries and merge, a gone token deletes the device, TTL and
  collapse key, encrypted payload.
- Android instrumented tests with a fake FCM message: notification shows with the screen off, reply works, `clear`
  removes it. The app has no `androidTest` setup today (only a JVM unit test), so this is new, and it runs on request.

**Soak test on real phones** (a Pixel and a Samsung, on request before a release):

| Case | Pass |
|---|---|
| App open, app in background, app swiped away, phone in Doze (`adb shell dumpsys deviceidle force-idle`), after reboot | 100% of 500 messages shown |
| Time from send to notification | 95% under 3 s, all under 30 s, in each case above |
| Read on the desktop | Phone notification gone within 5 s |
| Phone offline for 1 hour, then online | One notification per room, with the right count |
| Encrypted room | Notification shows the room and sender at once; the text shows on opening the app |

**Done when:** every soak test case passes on both phones, and the System screen shows no device with missed acks.

## 8. Voice and video calls

Calls must feel like the calls in a phone's own messaging app: one tap from a chat, they ring at once, they connect fast,
and they keep going when the network gets bad or changes.

### 8.1 Today

| Part | What exists |
|---|---|
| Media server | `crates/moqui-rtc` on str0m: ICE-lite, DTLS-SRTP, three simulcast layers, TWCC bandwidth estimate, audio before video, audio only below a floor ([WebRTC](../docs/interfaces/WebRTC.md)) |
| Codecs | Opus with in-band FEC; RED is off (`enable_opus(true, false)`, `sfu/mod.rs:542`) and DTX is not set. VP8 with RTX by str0m's default. Phase 5: RED on, DTX asked by the page (§8.3 item 8) |
| E2EE | SFrame, per room or per call link, set at the start and never lowered |
| Start from a chat | One call button in the room header; it rings everyone in the room and always starts with video (`Chat.qvue:807`, `1418`). Phase 4: Voice call and Video call (§8.3 item 1) |
| Ring on Android | FCM high-priority `ring` to `CallManager`; self-managed Android Telecom call (`CallConnectionService`), incoming only; CallStyle notification; full-screen ring over the lock screen; Decline with no WebView ([Mobile-Client](../docs/ui/Mobile-Client.md)). This is MT2 of [Mobile-Desktop](Mobile-Desktop.md), built |
| Media on Android | The WebView's WebRTC (`WebrootCall.js`). A native audio client is a later item (Mobile-Desktop, MT3's record) |
| Reconnect | A closed socket keeps its leg for 30 s (`RECONNECT_GRACE_MS`, `Calls.js:45`). When the connection fails the page joins again (`WebrootCall.js:613`); it does not restart ICE. The media server takes a leg's new offer. Phase 5: 60 s, and an ICE restart before a join (§8.3 items 10, 11) |
| TURN | Embedded TURN server over UDP, TCP, TLS, and TLS on 443 ([TURN](../docs/interfaces/TURN.md)). Built; not checked live |
| Not checked on a real phone | Bluetooth and speaker routing, two-way audio, encrypted calls on the phone's WebView |

### 8.2 What "seamless" means

| Moment | Behavior |
|---|---|
| Start | Call and video buttons in every direct and group chat header. One tap starts. The other side rings on all devices within 2 s |
| Answer | From the lock screen, a headset, a car or a watch, through Android Telecom. Audio flows within 1 s of Answer |
| During | Earpiece by default for voice, speaker for video. Proximity sensor turns off the screen at the ear. Bluetooth and wired headsets switch at once |
| Voice to video | Tap the camera to add video. The other side sees a prompt, not a new call. Video to voice the same way |
| Leave the app | The call keeps going (foreground service with microphone and camera types). Video goes to picture-in-picture |
| Move the call | "Move to this device": take an ongoing call to the desktop or back to the phone with no drop, like a handoff |
| Bad network | Audio stays clear. Video drops quality first, then pauses, then comes back. A small quality icon shows the state |
| Network change | Wi-Fi to mobile data and back with no drop and no user action (ICE restart) |
| Network gone | The call shows "Reconnecting" and comes back by itself when the network returns, up to 60 s |
| Missed call | Shows in the chat and as a notification (§7), with Call back |
| Group | Grid up to 9 tiles, active speaker larger, raise hand, reactions, screen share |
| Second call | Call waiting: hold and answer, or decline with a quick text |
| Encryption | The call screen shows the call's E2EE state with the §6.2 label. A call in an encrypted room is always encrypted (§6.1 rule 4) |

### 8.3 Work

**Call flow and UI**
1. A voice button and a video button in the room header on both layouts (§2, §3), in place of the one button. A
   direct room calls the person; a group room starts a group call that members can join from the chat.
2. A full-screen call screen on the phone: big avatar for voice, self view and remote video for video, mute, speaker,
   camera, flip, add people, hang up. The same screen in a side panel on the desktop.
3. Voice-to-video upgrade and downgrade in the same call (renegotiation exists).
4. Picture-in-picture on Android; a small floating call window on the desktop.
5. Audio routing on Android through Telecom `CallEndpoint` (earpiece, speaker, Bluetooth, wired). Proximity wake lock.
6. "Move to this device": a second device of the same user joins the call as a new leg, then the first leg leaves.
   The other side hears no gap.
7. Call waiting and decline with a quick text.

Status of items 1 to 7 (phase 4, web part; [WebRTC, The call's flow](../docs/interfaces/WebRTC.md#the-calls-flow)):

| # | Status |
|---|---|
| 1 | Built: Voice call and Video call in the room header on both layouts (the tray's narrow Chat tab has them in its menu); a direct room calls the person, a group room starts a group call its members join; an encrypted room passes `requireE2ee` |
| 2 | Built on the web: `m-call-compact`, full screen at a phone's width (big avatar for voice, the video and self view for video, mute, speaker where the browser chooses its output, video, flip, add people, hang up), and the same in the desktop's side panel (`state.panel`); the header shows the call's label from `e2eeLabel`. The Android app's native call screen is later |
| 3 | Built: `addVideo` and `dropVideo` on the same connection (renegotiation, SFrame on the new sender); the other side gets a prompt |
| 4 | Built on the desktop web: a Document Picture-in-Picture window (Chrome and Edge, and their automatic one), else the floating LiveCall window. On Android a video call on its call screen goes to picture-in-picture when the user leaves the app (by itself on Android 12+), the screen then showing only the video (phase 4, Android part) |
| 5 | Built on Android (phase 4): every call in Telecom (a joined one placed as outgoing), its audio through `CallEndpoint` (14+) or the `CallAudioState` routes: the earpiece for voice, the speaker for video, a headset at once, the call screen's audio button choosing; the proximity wake lock for a voice call on the earpiece; the foreground service's microphone and camera types ([Mobile-Client, During a call](../docs/ui/Mobile-Client.md#during-a-call-phase-4)). Not checked on a real phone yet (§18.4) |
| 6 | Built: `move#Call`, a new leg that closes the user's others once its media connected (`note#MediaEvent`); the other side's connection goes on (acceptance `CallFlow`). On the direct path the move first goes to the media server, a short gap. To and from the Android app (phase 4): the page's move, a moved-in call placed in Telecom and a moved-away one ended there |
| 7 | Built: Hold and answer (`hold#OwnLeg`), End and answer, Swap or Resume, Decline with a message (ready texts or one's own, to the caller's direct room; sealed in an encrypted room). On Android (phase 4): the page's hold is Telecom's hold and Telecom's hold reaches the page; a second ring rings through Telecom, and its Answer on the phone holds the call the page is in first |

**Network resilience** (the test results from §8.4 decide the order)

| # | Item | Status |
|---|---|---|
| 8 | Opus in-band FEC | Built. Checked: the answer has `useinbandfec=1` (`network_tests.rs`); it stays in an encrypted call, inside each frame |
| 8 | RED (redundant audio) at low bandwidth, and DTX | Built in phase 5: RED negotiated whenever the browser offers it (`SfuConfig::red`), both ways, with the SFU recovering on the way in; at 10% loss 99% of the audio arrives against 91%. Always on rather than at low bandwidth: the in-memory loss rows gain at every rate. Left out of an encrypted call's offers and answers (its transform sees the RED payload whole). DTX: the page asks `usedtx=1` of its Opus. Also from the results: the SFU's hold on a leg's audio for a lost packet, 300-500 ms, is now at most 120 ms (`audio_reorder_wait`) |
| 9 | NACK and RTX for video; the simulcast layer switch from the TWCC estimate | Built. Checked: NACK and RTX are negotiated (`network_tests.rs`). The switch takes about 2.3 s in memory, with a freeze of about 1.8 s while the estimate falls; the 2 s and 1 s lines are not met yet (§8.4 results) |
| 10 | ICE restart on a network change: Android's `ConnectivityManager` callback tells the page, and the page restarts ICE | Built in phase 5: the page restarts ICE on a disconnected or failed connection, on the browser's `online` and connection `change` events, on `moqui.call.networkChanged()` (Android's `NetworkWatch` in `MainActivity.kt`, not built on a device yet), and when nothing comes in for 4 s; a restart that does not reconnect in 8 s joins again. In memory a new interface with a restart loses 40 ms of audio |
| 11 | Keep the leg for 60 s. Media resumes on the new path without a new ring | Built in phase 5: 60 s (`RECONNECT_GRACE_MS`); a guest's socket retries for as long |
| 12 | TURN over TCP and TLS on port 443 for networks that block UDP. The client tries UDP, then TCP, then TLS | Built on the server. The client's order is the browser's ICE: every candidate is gathered at once and the pairs are checked by priority, UDP before TCP before TLS; the page adds nothing. The browser suite's UDP-blocked row checks ICE-TCP; TURN TLS on 443 live is not checked |
| 13 | Keepalives tuned for mobile NATs (STUN consent and the socket's ping), so a quiet call does not lose its path | Built in phase 5: the notification socket is pinged every 10 s instead of 30 while it carries a call. The UDP path keeps itself: the browser's consent checks every few seconds and DTX's packets every 400 ms (the SFU, ICE-lite, drops a peer after 15 s without a binding request) |
| 14 | A quality icon from `getStats` (loss, RTT, jitter), and "Your network is weak" when the uplink is the cause | Built in phase 5: `m-call-network` in the call's header, with "Reconnecting…" while the path is restarted or joined again |

**Direct path for one-to-one calls** (decided, §19)

15. A one-to-one call goes peer to peer when both sides can reach each other, and falls back to the media server.
    - Only between two users of this server in a direct room. Guests and phone calls use the media server.
    - A user setting "Always relay my calls" keeps the user's calls on the media server, so the other side never
      sees the user's IP address.
    - When a third person joins, the call moves to the media server.
    - SFrame stays on in an encrypted call on either path, so a move keeps the call's keys and its label.
    - Built in phase 5, after the §8.4 tests. The browser suite of §8.4 runs each profile on both paths.
    - Status: built in phase 5 ([WebRTC, The direct path](../docs/interfaces/WebRTC.md#the-direct-path)). The call's view
      says `direct`; the pages signal through the call's media node (`peer` frames), with STUN from the call's TURN
      servers, and move to the media server when they do not connect in 8 s, or a third person, a recording, captions
      or a screen share comes. "Always relay my calls" is in Settings, Calls (`CALL_ALWAYS_RELAY`). Acceptance
      `CallDirect` passes; the browser suite on both paths is written and not run yet (it needs root).

### 8.4 Network tests

Calls and the chat socket must work on bad mobile networks. No network test suite exists yet. It is built in two
layers, before the resilience work, so the results order that work:

1. **In memory.** `crates/moqui-rtc/src/sfu/testnet.rs` already runs clients against the media server with a set
   delay and a downlink limit. Add loss, burst loss, jitter with reordering, a bandwidth step, an outage and a new
   client address. These are Rust tests; one that runs under 15 seconds is a regression test.
2. **Real browsers.** Two headless browsers call each other through the media server, with network conditions made
   by standard open-source tools (Linux network namespaces with `tc netem` and `tbf`, and recorded cellular traces).
   It needs root or `CAP_NET_ADMIN`, so it is an opt-in acceptance test.

| Profile | Pass |
|---|---|
| Good LTE, congested LTE, 3G | Call connects in under 2 s. Audio clear. Video adapts |
| EDGE (about 200 kbit/s, 400 ms RTT) | Audio only, clear |
| Random loss 5% and 10% | Audio clear (FEC, RED). Video recovers |
| Random loss 20%, burst loss | Audio understandable. Call stays up |
| Jitter 200 ms with reordering | Audio without gaps longer than 200 ms |
| Bandwidth drop 5 Mbit/s to 300 kbit/s and back | Video goes down within 2 s and back up within 10 s. No freeze longer than 1 s |
| Outage 2 s and 10 s | Call stays up and recovers within 2 s of the network coming back |
| Outage 30 s | Call shows "Reconnecting" and comes back |
| Wi-Fi to mobile data (new IP) | Call goes on with less than 1 s of lost audio |
| UDP blocked | Call works over TURN TCP or TLS 443 |
| Satellite (600 ms RTT) | Call works; echo canceled |
| Uplink 50 kbit/s | Audio goes out; video off on that side |
| Chat socket: 30 s outage with 20 messages sent | All 20 arrive once, in order, after it comes back |

The browser suite is an acceptance test: it runs on request and before a release, and when one of its own tests is
added or changed. It does not run because `moqui-rtc`, `WebrootCall.js` or the TURN server changed.

**First results (phase 5, 2026-11-17).** In memory (`crates/moqui-rtc/src/sfu/network_tests.rs`;
`cargo test -p moqui-rtc --lib network_profiles -- --ignored --nocapture` prints them): A and B in a call through the
SFU, both sending audio and three video layers, B's link shaped both ways, a 10 s window. "Before" is the SFU as it was
(no RED, str0m's audio hold of 15 packets), "now" with RED and the 120 ms hold. The share of audio frames that arrive
(B hears A: B's downlink; A hears B: B's uplink), the longest gap in what A hears of B (the SFU's hold), and B's video
of A. A client there has no adapting encoder and no browser jitter buffer, so an uplink's capacity is the browser
suite's, and "gap" is arrival, before any buffer smooths it.

| Profile | Before: B hears A / A hears B, gap | Now: B hears A / A hears B, gap | Video now | Pass in memory |
|---|---|---|---|---|
| Loss 5% | 96% / 96%, 400 ms | 99.8% / 100%, 40 ms | high layer, freeze 120 ms | Yes |
| Loss 10% | 91% / 91%, 380 ms | 99% / 99%, 180-200 ms | high, 150-300 ms | Yes |
| Loss 20% | 84% / 81%, 500 ms | 97% / 97%, 200-240 ms | mid or high, about 330 ms | Yes (understandable, up) |
| Burst loss (about 8%, three in a row) | 89% / 89%, 440 ms | 97-98% / 97-98%, 180-220 ms | high, about 250 ms | Yes |
| Jitter 200 ms, reordered | 100% / 100%, 140 ms | 100% / 97-99%, 170-180 ms | low layer, frozen 4.7-6.2 s | Audio yes; video no (the estimate reads jitter as congestion) |
| 3G (1.5 Mbit/s, 300 ms, 1%) | 99% / 99%, 340 ms | 100% / 100%, 40 ms | mid layer | Yes; connects within 2 s |
| EDGE (200 kbit/s, 400 ms) | 90% / 100% | 78% / 100% | low layer, then audio only about 5 s in | No: the estimate takes about 5 s to fall from 2.5 Mbit/s, and the audio is lost with the video meanwhile |
| Satellite (600 ms RTT) | 100% / 100% | 100% / 100% | high | Yes; connects in about 2.4 s (ICE and DTLS's round trips) |
| Outage 2 s and 10 s | | Back in 0.3 s; the peer is kept | | Yes |
| Outage 30 s | | The SFU drops the peer at 15 s; the page's join brings the call back in 0.1 s | | Yes, with the join (and the 60 s leg) |
| Bandwidth 5 Mbit/s to 300 kbit/s and back | | Low layer after 2.3 s, up after 2.7-6 s, longest freeze 1.8 s | | Partly: up yes; down and the freeze miss the 2 s and 1 s lines |
| New interface (Wi-Fi to mobile) with an ICE restart | | 40 ms of audio lost | | Yes |
| New NAT port, no restart | | B's downlink back after 13 s (str0m's test client does not nominate again; a browser nominates on each ping to an ICE-lite peer, every 2.5 s), and the page's restart after 4 s with nothing coming in | | Left to the browser suite |
| Uplink 50 kbit/s, UDP blocked, Wi-Fi to mobile in a browser, chat socket outage | | | | Browser suite (UDP blocked: ICE-TCP passes in memory, `ice_tcp_reaches_a_client_without_udp`) |

What the results changed: RED on, the 120 ms hold, 64 kbit/s kept per audio track. What they leave: the estimate's
reaction to a sudden drop (EDGE, the bandwidth step), which a loss-based cut from the receivers' reports could speed;
video under heavy jitter. The browser suite (`CallNetwork`, `sudo scripts/call-network.sh`) is written and runs every
profile on both paths; it needs root, so it has not run yet: it skips without it.

### 8.5 Done when

- Every row of §8.2 works between two Android phones, and between a phone and the desktop app (real phones).
- Every row of §8.4 passes.
- An encrypted call (SFrame) passes the same tests as a plain one.

## 9. The user's own phone number

The app replaces an XMPP phone-number app (such as Cheogram Android with a gateway service). The user ports the number
to the server's Telnyx account. The server already does texts, MMS, calls and voicemail for a number (§1).

### 9.1 Gaps

| Item | Today | Work |
|---|---|---|
| Port the number | — | A Telnyx port order (a portal step by the user, not code). Then add the number in System, Voice: Numbers with the messaging bridge, the trunk and the user's routes |
| 10DLC | Required for texts from a US local number | The user registers the brand and campaign in the Telnyx portal ([SMS](../docs/interfaces/SMS.md)) |
| Checked live | Telnyx calls, SMS and MMS are tested with fakes only | Test on the real number: text, MMS, call in, call out, voicemail. Only the user starts a test that calls or texts a real number |
| Push for messages | The app shows only the call ring | §7 |
| Phone rooms in the list | Rooms of type `CrtExternal` with an SMS icon in Chat; no icon on the phone home (`ChatPanel.qvue:153`); listed only under All | The icon on the home (§2.2); a Phone group in the sidebar and a phone filter on the phone layout (§3) |
| Start a text | The Text action on a call in Call history (`Calls/History.xml:303`) | A phone number in the new-chat picker (§2.2) |
| Group texts | Not built (Future-Plans SMS) | Group MMS: one room for a set of outside numbers |
| Android dialer and contacts | A self-managed `PhoneAccount` is registered before each ring, for incoming app calls only (`CallManager.kt:146-164`). The Android dialer and Contacts app cannot call through a self-managed account | A managed account for the `tel` scheme with an outgoing path, so the dialer and contacts can call through the app with `place#PhoneCall`. Android deprecates the self-managed API at API 36 ([Mobile-Desktop](Mobile-Desktop.md)), so move to the current Telecom library in the same work |
| Phone book | The server has a `call-context` provider (number to contact) that an app implements; the framework alone keeps no contacts | Read Android contacts (asked on first use), on the phone only. Show names on phone rooms. Search contacts in the new-chat picker. On the server, names come from `call-context` where an app gives one (§14.8) |

### 9.2 Done when

- The ported number sends and gets texts and MMS in the app, with the SMS banner.
- A call to the number rings the app with the screen off. A call out from the Android dialer goes through the app.
- Voicemail shows in the number's room.
- A text while the app is closed shows an Android notification. Reply from the notification works.

## 10. The phone's SIM texts (`sim-sms`)

Optional: the app becomes the Android default SMS app for the phone's own SIM.

**Decided:**
- A cargo feature `sim-sms` in `moqui-mobile` turns on an optional crate `tauri-plugin-sim-sms`. Its Android library is
  merged only with the feature, so the default build asks for no SMS permissions. It is the crate's first cargo
  feature: today its one build switch is Firebase, which the Gradle build turns on when `google-services.json` exists.
- The plugin has the parts Android needs for the default SMS role: an `SMS_DELIVER` receiver, a `WAP_PUSH_DELIVER`
  receiver for MMS, the `RESPOND_VIA_MESSAGE` service, and handlers for `sms:`, `smsto:`, `mms:` and `mmsto:` links.
- Release builds make two APKs: the default one and one with `sim-sms`.
- SIM texts stay on the phone by default. A per-user switch, off by default, syncs SIM threads to the server as rooms,
  so they show on the desktop and the web too. Both are in the first `sim-sms` version (decided, §19). The switch
  says that synced texts are stored on the server.

**Design needed:**
- MMS send and receive: the PDU encoding and the download of MMS content.
- A synced SIM thread: its room type and bridge type, and how a text sent from the desktop goes out through the
  phone (the phone as the bridge, through a push and the Rust outbox) and what the room shows while the phone is off.
- How SIM threads and server rooms show together in the list, and what happens when a person has both a SIM thread
  and a room on the user's Telnyx number.

# Part 2: One app for all messaging

## 11. Principles

- **One inbox.** Every conversation, from any network, is a room in one list. A person with several networks is one
  contact, and the user can pick the network per message.
- **Best path first.** To a person who has MR Moqui: Moqui chat (E2EE). Else the network they use. Else SMS.
- **Say what is private.** Every room shows its network and its E2EE state (§6).
  A bridge always gets the label "Not end-to-end encrypted on this server. The bridge reads messages to pass them on."
- **Official ways first.** An official API before an unofficial one. Each unofficial bridge is an opt-in build feature,
  marked experimental, and runs as a separate process.
- **No lock-in.** History comes in from the old app (§15), and the user can export it again.

## 12. The protocols MR Moqui keeps

These are built ([Future-Plans, Comms](Future-Plans.md#comms)). The app must keep them working and show them in the one
inbox.

| Protocol | Who it reaches | E2EE | Notes |
|---|---|---|---|
| Moqui chat (web, desktop, Android) | MR Moqui users | Yes in encrypted rooms (Megolm) | The home network |
| WebRTC calls | MR Moqui users and guests by link | Yes (SFrame) | §8 |
| XMPP (C3a) | People on XMPP apps (Conversations, Cheogram, Monal, Dino, Gajim) with an account on this server | OMEMO between XMPP apps | Federation to other XMPP servers is C3b, not built |
| SIP | Desk phones, soft phones, the phone network through a trunk | No | Calls, IVR, voicemail |
| SMS and MMS | Any phone number, through Telnyx | No | One room per outside number with its texts, calls and voicemail |
| Mail | Any mail address | No (TLS only in transit) | The Comms Inbox; a person's mail can show beside their chats |
| Matrix (C6a client API, C6b federation; planned) | Matrix users | Megolm possible | The appservice API (C6c) is also the way to host Matrix bridges (§13.2) |

## 13. Reaching each network

### 13.1 Summary

| App | How the app reaches its people | Official? | E2EE through MR Moqui | Needs |
|---|---|---|---|---|
| Google Messages: SMS, MMS | The user's number, ported to Telnyx (decided) | Yes | No | Built; live checks (§9) |
| Google Messages: RCS | Business RCS through Telnyx; personal RCS only through a Google Messages for Web bridge | Business: yes. Personal: no | No | §16; see §13.4 |
| iMessage | A Mac the user owns runs BlueBubbles Server; an adapter talks to its API | No | No | An always-on Mac signed in to the Apple ID |
| Signal | A linked device (like Signal Desktop) through `signal-cli` | No (Signal allows linked devices; the client is third party) | No, the bridge decrypts | A Signal account on a phone to link from |
| GroupMe | GroupMe's public API v3 with the user's access token | Yes | GroupMe has no E2EE | The user's token (`request_secret`) |
| Cheogram | Replaced: the number moves to Telnyx (decided). XMPP contacts reach the user on this server's XMPP | — | OMEMO only between XMPP apps | C3b federation for contacts on other XMPP servers |

### 13.2 How bridges plug in

All networks go through the chat bridge layer ([Chat-Bridges](../docs/interfaces/Chat-Bridges.md)): a
`ChatBridgeAdapter` (the trait is in `crates/moqui-core/src/service/chat_bridge.rs`; the adapters are in
`crates/moqui-channels`, which has one today, Telnyx SMS), one room per outside conversation, the bridge chip and the
E2EE label from the adapter's `encryption` capability (§6.2). A new network also needs its bridge type (`Cbt…`) where
`ChatEntities.xml` has none.

| Way | Use for | Why |
|---|---|---|
| A Rust adapter on a documented API | GroupMe (REST), BlueBubbles (REST and its event socket), Telnyx RCS | Small, no extra process, same as the SMS adapter |
| A Rust adapter on a sidecar's JSON-RPC | Signal (`signal-cli` daemon, GPL-3) | The Signal protocol stays in a maintained client. The sidecar is a separate process |
| The Matrix appservice API (C6c) with a mautrix bridge as sidecar | Personal RCS (`mautrix-gmessages`), later WhatsApp, Telegram, Discord | One API in MR Moqui opens many maintained bridges. Each bridge is AGPL and runs as a separate process |

Each sidecar is optional (a cargo feature and a System screen to start it; `moqui-channels` has no cargo features
yet). The sidecar's license applies to the sidecar, not to MR Moqui; a legal check is still needed before shipping one.

### 13.3 Per network

**iMessage**
- Apple has no API. The only working way is a Mac that stays on, signed in to the user's Apple ID, running BlueBubbles
  Server. Messages, attachments, read state and group chats work. Tapbacks, edits, unsend and typing need its Private API,
  which needs System Integrity Protection turned off.
- Adapter `bluebubbles.rs`: send and receive through the server's REST API and event socket; tapbacks map to reactions.
- Risks: Apple can block the Apple ID for unusual volume. The Mac and the server both read all messages.
- Without a Mac: iPhone users get SMS and MMS from the user's number (green bubbles). No RCS from this app (§16).

**Signal**
- `signal-cli` links as a secondary device, the same way Signal Desktop does. It sends and receives messages, groups,
  reactions, replies, attachments, edits, deletes and read receipts over JSON-RPC.
- Adapter `signal.rs`: map each Signal chat and group to a room. Calls: show "Signal call, open Signal" (signal-cli does
  not carry calls).
- Risks: unofficial client; Signal can change the protocol; the linked device must connect at least every few weeks or
  Signal unlinks it. The server stores plaintext.
- The UI says plainly: "Signal messages are end-to-end encrypted until they reach your server. Your server can read them."

**GroupMe**
- Public API v3 (`api.groupme.com/v3`): groups, direct messages, likes, attachments, members. A user token reads groups,
  members and Topics; the push channel gives new messages.
- Adapter `groupme.rs`: each group and Topic is a room; likes map to a heart reaction; `@all` maps to `@room`.
- Risk: low; the API is old but works. GroupMe has no E2EE, so nothing is lost.

**Google Messages and RCS** (details in §16)
- SMS and MMS: through the ported number on Telnyx. Group texts: §9.
- Business RCS (RBM) through Telnyx: verified brand sender, rich cards, replies in the thread. Not person to person.
- Personal RCS: only a bridge to Google Messages for Web (`mautrix-gmessages` behind C6c). It needs a phone with Google
  Messages as the default SMS app and the number on its SIM.

**Cheogram**
- Its phone number role moves to Telnyx (decided). Its XMPP role stays: the user's XMPP contacts on this server reach the
  user in the app; XMPP apps (including Cheogram itself) can still sign in to this server.
- C3b federation brings contacts on other XMPP servers.

### 13.4 The RCS conflict

Personal RCS needs the number on a SIM in a phone with Google Messages (§16). The user's number is moving to Telnyx,
so that number cannot have personal RCS. The choices:

| Choice | Result |
|---|---|
| A. Number on Telnyx (decided) | SMS, MMS, calls and voicemail in the app. Others see SMS, not RCS. Business RCS possible for a brand sender |
| B. Number stays on a SIM, Google Messages plus the bridge | Personal RCS through the bridge; SMS and MMS through the bridge too; no Telnyx calls on that number |
| C. Both: Telnyx number for the user, a second SIM line with RCS | Two numbers; people must know which to use |

The plan builds A. B and C stay open (§19.3).

## 14. Equivalent features

Each feature of each app and what replaces it in MR Moqui. "Built" means it exists today. A § number points to the plan
that adds it. "New" means this plan adds it (§17).

### 14.1 Messaging basics (all five apps)

| Feature | Equivalent in MR Moqui | Status |
|---|---|---|
| Direct and group chats | Direct, private and public rooms | Built |
| Replies, quotes | Reply, threads | Built |
| Edit, unsend | Edit (24 h), redact | Built |
| Reactions, tapbacks, likes | Reactions; a tapback or a like maps to a reaction across bridges | Built; picker in §4 |
| Mentions, @all | Mentions, `@room` | Built |
| Typing, read receipts (with an off switch) | Typing, read receipts, `CHAT_READ_RECEIPTS` | Built |
| Photos, video, files, voice notes | Uploads; voice notes in §5.1 | Partly built |
| Search, media gallery | Server search (plain rooms); Details tabs | Built; tabs in §5 |
| Pin, mute, archive | Pin, mute, archive | Built |
| Multi-device: phone, desktop, web | Android, desktop app, web | Built |
| Notifications with reply | §7 | Plan |
| Voice and video calls | §8 | Built; plan for seamless |
| Polls | Polls | Built |

### 14.2 Google Messages

| Feature | Equivalent | Status |
|---|---|---|
| SMS, MMS, group MMS | Telnyx SMS, MMS; group texts | Built; group texts planned |
| RCS chat | Business RCS; personal RCS by bridge (§13.4) | §16 |
| Default SMS app | `sim-sms` build | §10 |
| Scheduled send | Scheduled send: a send time on a message the server holds and sends, as mail's due drafts are (`send#DueDrafts`). A chat event has no send time today, and the only outbox is the client's | New |
| Spam protection, block, report | Block a number: the caller lists that call handling has (`CallerList`, Settings, Call handling), used for texts too. Block a person in chat: new. Spam filter for outside numbers (Telnyx number reputation and a local list) | New; the caller lists are built |
| One-time codes: copy, delete after 24 h | Detect codes in SMS; Copy button in the notification; optional delete | New |
| Message reminders | "Remind me" on a message: a notification later | New |
| Smart replies, writing help | Suggested replies from `moqui-llm`, plain rooms only (never encrypted rooms) | New |
| Photo quality choice | Send original or compressed | New |
| Messages for web | The web client and the desktop app | Built |

### 14.3 iMessage and FaceTime

| Feature | Equivalent | Status |
|---|---|---|
| Blue-bubble chat with iPhone users | BlueBubbles bridge (§13.3), else SMS | New |
| Tapbacks | Reactions | Built |
| Edit and unsend within minutes | Edit, redact | Built |
| Send later | Scheduled send | New |
| Message effects, stickers, GIFs | Sticker packs; GIF search through a provider the server proxies; effects: no | New (stickers, GIFs) |
| Share location, live location | A location message with a map, and live location for a set time. The map needs the Maps app, which is a design only ([Maps](Maps.md)) | New, after Maps |
| Check In (tell a contact you arrived) | Later, on live location | Later |
| FaceTime audio and video, group FaceTime | WebRTC calls | Built; §8 |
| Handoff of a call between devices | "Move to this device" | §8 |
| Shared photo albums in a chat | Media tab; save to a shared drive folder | Media tab planned |
| Contact names and photos | Phone book import | §9 |

### 14.4 Signal

| Feature | Equivalent | Status |
|---|---|---|
| E2EE by default | Encrypted direct and private rooms by default | §6.5 |
| Safety numbers | Emoji and QR verification | Built |
| Disappearing messages | A per-room timer from 30 s to 4 weeks. The clients delete at the time, the server deletes by an exact job. Today retention is whole days by a daily job | New |
| View-once media | Media that opens once, then the clients delete it and the server deletes the file | New |
| Note to self | A room with only the user | New (check if a one-person room works) |
| Usernames, phone number hidden | Usernames; a user's phone number is not in chat's data and is never shown to other MR Moqui users. An outside number shows to the members of its own room | Built |
| Linked devices list | Apps and devices | Built |
| Registration lock, PIN | Device keys, password, recovery key for key backup | Built |
| Screen security (no screenshots) | Android `FLAG_SECURE` option, per app or per encrypted room | New |
| Group links with admin approval | Room invite links and QR codes, with optional approval | New |
| Stories | Later, if wanted | Later |
| Sealed sender | Not possible: the server sees who sends. The UI says so (§6) | Not planned |
| Proxy for censored networks | TURN on 443 and the web socket on 443 | Built; live check in §8 |

### 14.5 GroupMe

| Feature | Equivalent | Status |
|---|---|---|
| Large groups | Public and private rooms; test with 5,000 members | Built; load test new |
| Join by SMS without the app | SMS members of a room: a phone number added to a room gets each message as a text with the sender's name, and its replies post to the room | New |
| Group invite links and QR | Invite links | New |
| Likes | Heart reaction | Built |
| Topics (subgroups) | Threads, or linked sub-rooms | Built (threads) |
| Events with RSVP | An event message with Going / Maybe / No, and a calendar entry | New |
| Polls | Polls | Built |
| Gallery | Media tab | §5 |
| Mute, @all | Mute, `@room` | Built |
| Directory and join requests | Browse rooms; join request for private rooms | Browse built; requests new |

### 14.6 Cheogram (XMPP)

| Feature | Equivalent | Status |
|---|---|---|
| Phone number: SMS, MMS, calls, voicemail | Telnyx number | Built; §9 |
| Calls in the Android dialer | Android `PhoneAccount` | §9 |
| XMPP chat with OMEMO | This server's XMPP for XMPP contacts; Moqui chat with Megolm in the app | Built (C3a) |
| Contacts on other XMPP servers | C3b federation | Not built |
| Group chats (MUC) | Rooms are MUC rooms | Built |
| Ad-hoc commands (XEP-0050) | Not needed after the number moves | Not planned |
| Stickers, reactions, replies, threads | Stickers new; the rest built | Partly |
| Push without Google (UnifiedPush) | UnifiedPush option | §7.4 |

### 14.7 Android Phone app

The app can become the Android default phone app (the `ROLE_DIALER` role). It then handles every call on the phone:
the SIM's carrier calls and the server's calls (Moqui calls and the Telnyx number).

| Feature | Equivalent | Status |
|---|---|---|
| Dialer keypad, `tel:` links | Dialer screen (exists on the web; the desktop app can be the `tel:` handler), `tel:` and `DIAL` intents on Android | Dialer built; Android intents new |
| In-call screen for SIM calls | An `InCallService`: the same call screen as Moqui calls (§8) for carrier calls | New |
| Pick the line per call | SIM line, the Telnyx number (through `place#PhoneCall`), or a Moqui call to a user. Default by contact, last used | New |
| Incoming carrier calls | Ring screen over the lock screen (exists for Moqui calls), with the caller's contact name | Ring built; carrier calls new |
| Call log | One history: SIM calls (Android `CallLog`), Telnyx calls and Moqui calls (server call history), merged and by person | New |
| Voicemail | Server voicemail in the number's room (built). The carrier's voicemail: dial-in only (visual voicemail of carriers is closed) | Server voicemail built |
| Spam and unknown callers | Android `CallScreeningService`: block numbers on the user's caller lists (§14.2; the server already rejects by them), silence unknown callers as an option | New |
| Hold, mute, speaker, Bluetooth, merge, swap | Android Telecom for SIM calls; the server for Telnyx and Moqui calls | Partly built |
| Emergency calls | Always on the SIM and the carrier, through Android, never through the server. The app must not block or delay them | Required |
| Calls from the Contacts app and the car | A managed Android `PhoneAccount` for the Telnyx number and Moqui calls, so other apps and cars can call through them | §9 |

**Build:** a cargo feature `sim-dialer`, like `sim-sms`, with its Android parts in an optional plugin crate. The default
APK has no `CALL_PHONE`, `READ_CALL_LOG` or `ANSWER_PHONE_CALLS` permissions. Google Play allows call log permissions only
for the default phone app, so the call log is in this build only. `sim-sms` and `sim-dialer` can ship in one "phone" APK.

**Done when:** with the role given, a carrier call in and out, a Telnyx call in and out, and a Moqui call all use the
app's screens and appear in one call log; an emergency number goes to the carrier at once.

### 14.8 Android Contacts app

| Feature | Equivalent | Status |
|---|---|---|
| Contact list and details | A Contacts screen in the app: name, photo, numbers, emails, and each network the person is on (§11 unified contacts) | New |
| Contacts synced to the server | The server's CardDAV contact books (Mantle's `ContactDavServices`, [Sync](../docs/interfaces/Sync.md)): personal books read and write, shared books read only | Built on a server with the mr-mantle add-on. The framework alone keeps no contacts: there the Contacts screen shows the phone's own contacts and MR Moqui users only |
| Contacts in other Android apps | A built-in Android sync adapter: an Android account "MR Moqui" that syncs the CardDAV books into Android's contact store, both ways. The phone app, the keyboard and the car see them | New |
| Existing phone contacts | Import once from the phone's other accounts into the personal book, with duplicates merged | New |
| Caller name and photo | From the synced contacts on the phone, and on the server from the `call-context` provider that an app implements, for SIM, Telnyx and Moqui calls and for texts | New |
| Favorites, groups | Favorites (starred), contact groups as CardDAV categories | New |
| Share a contact | A vCard message in chat; save a received vCard | New |
| Network per person | Link a contact to an MR Moqui user, an XMPP address, a Signal or GroupMe identity, a phone number (§11) | New |

**Build:** the contact sync is in the default APK (`READ_CONTACTS` and `WRITE_CONTACTS`, asked on first use, with a
reason). It also works today with an outside CardDAV app, such as DAVx5, and an app password.

**Done when:** a contact made in the app shows in Android's contact list and on the server; a contact changed on the
server shows on the phone within a minute; a call and a text from that contact show its name and photo.

## 15. Moving history in

| From | How | Notes |
|---|---|---|
| Phone SMS and MMS | The `sim-sms` build reads the phone's messages once and uploads them to the number's rooms | Needs the default SMS role during import |
| Signal | `signal-cli` backfill when linking (Signal offers "transfer message history") | Only what Signal transfers |
| iMessage | BlueBubbles backfill (pages of 1,000) | From the Mac's history |
| GroupMe | The API's message history per group | Full history |
| Cheogram | The XMPP archive (MAM) of the old account, if its server keeps it; or an export | Best effort |
| Google Messages | Through the phone's SMS store (`sim-sms` import); RCS history only through the bridge | RCS history mostly lost |

Each import is idempotent (an outside message id per network), so it can run again.

# Part 3: RCS

## 16. RCS

Scope: the mobile app (`crates/moqui-mobile`) and the server chat bridges (`crates/moqui-channels`,
`crates/moqui-core/src/service/chat_bridge.rs`).

### 16.1 Summary

1. A third-party Android app cannot send or receive RCS on the user's own number. Android has no public RCS API. Only Google Messages can do it.
2. The server can send and receive RCS as a verified **business** sender (RBM) through Telnyx. People can reply in a 1:1 thread. Brand vetting and carrier approval are necessary.
3. RCS end-to-end encryption (MLS, Universal Profile 3.0) is live only in Google Messages and Apple Messages. Business RCS is never end-to-end encrypted.
4. A bridge to Google Messages for Web (the mautrix-gmessages approach) gives personal RCS on the user's number. It is unofficial, AGPL, and the bridge reads all plaintext.
5. Recommendation: build a Telnyx RCS business adapter first. Add a personal RCS bridge only as an opt-in, labelled experimental feature.

### 16.2 Findings

The five research findings behind this section are in [Appendix A](#a-rcs-research-findings): personal RCS in a
third-party app, business RCS through a provider, RCS end-to-end encryption, open-source RCS clients, and bridging
Google Messages for Web.

### 16.3 Options

| Option | How it works | Who can use it | E2EE? | Cost / approval | Effort | Risks | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Native RCS in the app | App talks RCS to carrier/Jibe | Nobody (no API) | n/a | Not possible | n/a | n/a | Do not build |
| 2. Hand-off to Google Messages | App opens the thread in Google Messages by intent | Any user with Google Messages | Yes, in Google Messages | Free | Low | Two apps; the thread is not in MR Moqui | Build as fallback |
| 3. Telnyx RCS business agent | Server adapter on Telnyx RBM API | Operators with a verified brand | No | $100 vetting, $500 T-Mobile, per-message fees | Medium | Approval time; A2P only; not the user's number | **Build first** |
| 4. Other RBM providers (Twilio, Sinch, Vonage) | Same as 3 | Same as 3 | No | Similar | Medium each | Same as 3 | Later, if needed |
| 5. Google Messages Web bridge (sidecar) | Go `libgm` sidecar + Rust adapter | One user per paired phone | No (plaintext at bridge) | Free | Medium-high | ToS, AGPL, breakage, phone online, no `sim-sms` | Optional, experimental, off by default |
| 6. Open-source IMS client | Own RCS stack | Rooted/privileged only | Possible in theory | n/a | Very high | Carriers reject it | Do not build |
| 7. EU DMA / GrapheneOS / future API | Wait for an official path | Unknown | Unknown | Unknown | Unknown | May never cover chat apps | Watch |

### 16.4 Phased plan

#### RCS phase 0: Safe `sim-sms` with no RCS loss

1. Detect Google Messages and its RCS state when the user enables `sim-sms` (`crates/moqui-mobile`, Android plugin).
2. Show a warning before the app takes the SMS role. Explain that RCS stops.
3. Give two buttons: "Turn off RCS in Google Messages" and "Open disable-chat page".
4. Add a per-thread "Open in Google Messages" action (Option 2). Use `sms:` / `smsto:` intents with the Google Messages package.
5. Label every SIM SMS/MMS thread with the SMS label of §6.2, and "Your carrier can read these messages." in the
   message info.

#### RCS phase 1: Telnyx RCS business adapter (server)

1. Add `crates/moqui-channels/src/telnyx_rcs.rs` next to `telnyx.rs`. Name the adapter `rcs`.
2. Add a chat bridge type for RCS (for example `CbtRcs`; `ChatEntities.xml` has none). Keep `configJson` encrypted, like the SMS adapter.
3. Config keys: `apiKey`, `publicKey`, `agentId`, `messagingProfileId`, `smsFallbackFrom`, `mmsFallback`, `apiBaseUrl`.
4. Reuse the Ed25519 webhook check (`webhook::signature_valid` in `crates/moqui-channels/src/webhook.rs`) and the replay cache of the bridge layer (`replay_take` in `chat_bridge_web.rs`). Move code that only `telnyx.rs` has and both need into a small `telnyx_common` module.
5. Map `deliver()` to `POST /v2/messages/rcs`. Send attachments as `rich_card` media by `file_url`.
6. Map inbound `text`, `user_file`, `location` and `suggestion_response` to `ChatBridgeServices.ingest#Event`.
7. Map `message.sent`, `message.finalized` and `message.read` to delivery and read status.
8. Record `data.type` (RCS or SMS) on each sent message. Show the real path in the UI.
9. Set `Capabilities`: `edit=false`, `delete=false`, `reaction=false`, `idempotent=false`, `encryption=Transport`.
10. Add a capability check before send. Fall back to the SMS bridge when the number is not RCS-capable.
11. Add tests with a fake Telnyx server, like the current SMS tests.
12. Write an operator guide in `docs/interfaces/`: brand, agent, test devices, launch, fees.

#### RCS phase 2: Rich features and labels

1. Add suggested replies and actions to the chat model. Show them as chips in the mobile UI.
2. Add rich cards and carousels as a message kind. Render them in the mobile UI.
3. Add typing and read events when Telnyx exposes RBM `IS_TYPING` and `READ`.
4. Show the RCS labels below in thread headers and message info. The `encryption` capability they come from is built
   with the other labels (§6.2, §18 phase 3), not here.

#### RCS phase 3 (optional): Personal RCS bridge, experimental

1. Ask the user to approve the ToS and AGPL risks first.
2. Create a separate AGPL sidecar (Go, on `libgm`). Keep it in its own public repo.
3. Expose a local API: pair, send, receive, media, typing, read, reactions.
4. Add `crates/moqui-channels/src/gmessages.rs` behind a cargo feature `gmessages-bridge`. Default off.
5. Add a pairing screen in the MR Moqui mobile app: Google login, then emoji confirm.
6. Block `sim-sms` and `gmessages-bridge` on the same phone number. Show why.
7. Show health: phone offline, web session taken, login expired.
8. Label all these threads "RCS via bridge - not end-to-end encrypted".

#### RCS phase 4: Watch items

1. Check the EU DMA Article 6(7) eligibility terms (draft due 2027-02-01).
2. Check Android 18 for new Messages APIs.
3. Check GrapheneOS RCS client progress.
4. Check Telnyx docs for RBM typing events and non-US API onboarding.

### 16.5 UI labels for E2EE status

The labels for MR Moqui chat, SMS and other bridges are in §6.2, the one table for them. These rows add the RCS
paths, in the same form.

| Path | Label (thread header) | Detail text (message info) | Icon |
| --- | --- | --- | --- |
| RCS business (Telnyx RBM) | "RCS Business - not end-to-end encrypted" | "Encrypted in transit only. Telnyx, Google, carriers and this server can read it." | Open lock + verified badge |
| RCS business, sent as SMS fallback | "Sent as SMS - not encrypted" | "The phone did not support RCS. This message went as SMS." | Open lock |
| RCS via Google Messages bridge | "RCS via bridge - not end-to-end encrypted" | "Google Messages on your phone decrypts it. The bridge and this server can read it." | Open lock + bridge icon |
| Hand-off to Google Messages | "Opens in Google Messages" | "Google Messages shows its own encryption status." | External link |

Rules:

- Never show a closed lock for any RCS path in this app.
- Do not copy the Google Messages lock state to bridged threads.
- Show the per-message path when it differs from the thread (RCS vs. SMS fallback).

# Part 4: Work and order

## 17. New work for other networks

These come after phases 0 to 9 of §18, in the order at the end of §18.

| # | Work | Section |
|---|---|---|
| 1 | Unified contacts: one person with many networks; pick the network per message | §11 |
| 2 | Block and report; spam filter for outside numbers | §14.2 |
| 3 | Scheduled send, message reminders | §14.2, §14.3 |
| 4 | Disappearing messages (exact timer) and view-once media | §14.4 |
| 5 | Invite links and QR, with approval; join requests | §14.4, §14.5 |
| 6 | SMS members of a room (GroupMe's SMS join) | §14.5 |
| 7 | Stickers, GIF search, location and live location | §14.3 |
| 8 | Event messages with RSVP | §14.5 |
| 9 | Screen security option; one-time code helper; send original photo | §14.2, §14.4 |
| 10 | GroupMe adapter (`groupme.rs`) | §13.3 |
| 11 | Signal adapter on `signal-cli` (`signal.rs`), optional build feature | §13.3 |
| 12 | iMessage adapter on BlueBubbles (`bluebubbles.rs`), optional build feature | §13.3 |
| 13 | Telnyx RCS adapter | §16 |
| 14 | Matrix appservice API (C6c), then `mautrix-gmessages` for personal RCS | §13.2 |
| 15 | History imports | §15 |
| 16 | Default phone app (`sim-dialer`): SIM calls, line choice, one call log, call screening | §14.7 |
| 17 | Contacts: Contacts screen, Android sync adapter on CardDAV, phone import, caller names | §14.8 |

## 18. Implementation phases

### 18.1 Order

| # | Phase | Sections | Size |
|---|---|---|---|
| 0 | Fixes from the UI review: done 2026-11-16 (`82affc535`) | §5.2 paging, §6.5 | Small |
| 1 | Shared helpers and the rest of the phone layout: done 2026-11-17 (`c88df4a3f`) | §2 | Medium |
| 2 | Notifications | §7 | Large |
| 3 | E2EE labels and the call rule: done 2026-11-17 (`fdd1ab597`) | §6.1–§6.4 | Medium |
| 4 | Calls: flow and screen | §8.3 items 1–7 | Large |
| 5 | Calls: network tests, then resilience: done 2026-11-17 (`64c002d20`; the browser suite under sudo and real phones left) | §8.3 items 8–15, §8.4 | Large |
| 6 | Message menu, reactions, Info: done 2026-11-17 (`acc6d1b66`; a real phone's feel left) | §4 | Medium |
| 7 | Details tabs and services, voice notes | §5 | Medium |
| 8 | Desktop gaps | §3 | Medium |
| 9 | The user's phone number | §9 | Medium, plus the user's steps |
| 10+ | Work that needs a design first, then the other networks | §6.5, §10–§17 | — |

- Phase 0 is first because two of its fixes are under later work: the paging rule (§5) and device verification (§6).
- Labels (3) come before calls (4), since the call screen shows the label and the call rule changes how a call starts.
- Calls are two phases, so the tests of §8.4 exist before the resilience work and order it.
- After phases 2 and 4: a new APK and a test on a real phone, with the checks listed in §18.4. Again after phase 7.

### 18.2 Rules for every phase

- **Commits and tests.** A phase is one or more commits. Each commit runs the specs of what it touched
  (`./moq test <component> --spec …`, the Rust tests of a changed crate). `./moq test-framework` runs once at the end
  of the phase: all of this is framework work (`crates/`, `runtime/components/base`).
- **Which suite.** A rule or a fixed bug is a regression spec. A phone-width check, a two-user walk and a full
  scenario are acceptance specs (`test/acceptance/client/`), run only when added or changed. A screen is rendered at
  most once per mode in the regression suite. The budgets stay (basic-comms: server 9 s, client 9 s, raised from 7 s
  on 2026-11-16 with the user's agreement for the emoji verification check).
- **Existing harness.** `basic-comms/test/client/ShellSync.spec.js` with `ShellSync.page.js` already loads the bundled
  screens with a fake `moquiDesktop` host; shell checks build on it. The client test kit sets a viewport
  (`newPage({viewport})`) and runs two users, and has no touch emulation.
- **Mail stays working.** A phase that changes shared code (the shell, the palette, back handling, notifications)
  runs the mail specs too (§1, Mail). Mail keeps its own rules where chat has a similar word (COM-UI-022, 024, 029):
  - Mail rights (owner, manage, read, send) are not room roles. Leaving a shared mailbox is not removing the account.
  - A mail reply, reply all and forward are not a chat reply and forward: they keep Reply-To, To and Cc, the quote and
    the message references.
  - A folder move is not a label. An attachment on a message, its copy on a forward, an uploaded file and a copy in
    Files are four different things.
  - An automatic reply is not chat Do not disturb, and neither holds back the other.
  - An account removed with its mail kept cannot be restored; it is not a trash.
  - A sent unsubscribe request is not a confirmed unsubscribe, and an invitation shown is not an answer sent.
- **Terminal.** `Chat.xml` has a terminal twin (`Chat.tui.json`). Each phase says what the terminal shows for its new
  parts, or that it shows nothing.
- **Text.** Every new UI text goes through `t()`.
- **Data model.** New entities and fields need no migration before the first release; a clean load is enough.
- **Failure tolerance.** A worker that handles independent rows (pushes, devices) logs a failing one and goes on.
- **Docs.** The phase updates the guides it changes: [Comms](../docs/tools/Comms.md),
  [Mobile-Client](../docs/ui/Mobile-Client.md), [Desktop-Client](../docs/ui/Desktop-Client.md), the
  `docs/interfaces/` page, and the BasicComms README. A new host member goes in the JS reference and
  `moqui-host.d.ts`.

### 18.3 The phases

#### Phase 0: fixes from the UI review

Done 2026-11-16 in `82affc535` ([review](ui-review-basic-comms.md), COM-R01). Chat defects that the review found,
fixed before new work is built on them. Emoji verification stalled because nothing called `Sas.accept()` on the device
that did not start the comparison; My devices overflowed because its Quasar `column` wraps.

| Work | Finding | Files |
|---|---|---|
| `find#RoomFiles` returns every file of a message at a page end; `find#RoomThreads` does not skip threads with equal dates (§5.2 paging rule) | COM-UI-010 | `ChatServices.xml` |
| Emoji verification completes: find why no emoji show after Compare emoji (the receiving side's accept step is the candidate), with states, a timeout and a retry | COM-UI-012 (P1) | `WebrootCrypto.js`, `Devices.qvue` |
| My devices fits a 390 px phone | COM-UI-014 | `Devices.qvue` |
| The Send button is icon-only in the `stacked` Chat | COM-UI-002 | `Chat.qvue:580` |
| One time and day format in the room, the lists and the shell (§2.2 step 1, the helpers) | COM-UI-007 | `WebrootChat.js`, `Chat.qvue`, `ChatPanel.qvue`, `Root.qvue` |
| The last owner's Leave names the successor; the succession's event reads as automatic, not as the leaving owner's act | COM-UI-009 | `Chat.qvue`, `ChatServices.xml` |
| The search pane of an encrypted room says it is not searched | COM-UI-011 | `Chat.qvue` |
| Home list: the phone-room icon; `setNotification` returns `archivedEventSeq`; remove the two bindings nothing uses (`open-room`, `create-from` on the root) | — | `ChatPanel.qvue`, `Chat.xml`, `Chat.qvue` |

Tests: a regression spec for each paging boundary, and for the time format with a user zone that is not the
browser's. The verification fix extends the client spec `CryptoFlow`.

The mail defects of the review (COM-UI-016, 017, 018, 020, 021, 023, 025–028) are not part of this plan; the review's
own pass fixed them (`d34cc6645` for COM-UI-020, then `5f4787d0f`).

#### Phase 1: shared helpers and the rest of the phone layout

Done 2026-11-17 in `c88df4a3f`. Work: §2.2 steps 2 to 7. The helpers of step 1 were built in phase 0, and so were the
Send button (step 5) and the home list's icon and `setNotification` (step 6).

| Work | Files |
|---|---|
| `moqui.back` (`push`, `track`, `pop`, `base`) and its callers: Esc (`closeTopmost`, Chat's `onEscape`), the shell's Back and `goBack` (then its history, `historyBack`), Android's back (`MainActivity.kt`, before Tauri's own back: the WebView's history, then leaving) | `WebrootVue.qvt.js`, `Chat.qvue`, `ChatPanel.qvue`, `Root.qvue`, `gen/android/…/MainActivity.kt` |
| The back arrows name where they go: "Back to chats", "Back to the room" | `Chat.qvue`, `ChatPanel.qvue` |
| Swipe to reply (64 px, the room's timeline; it cancels the long press) | `Chat.qvue` |
| The pickers: a typed number (`searchUsers` with `numbers=true`), the user's numbers (`textNumbers`), the number's room (`textRoom`, `open#PairRoom`); a name with two or more people makes a private room; `moqui.chat.pickKind`, `startChat` | `Chat.xml`, `WebrootChat.js`, `Chat.qvue`, `ChatPanel.qvue` |
| Attach with bubbles: the camera, the gallery or a file | `Chat.qvue` |
| Bundled screens regenerated | `bundled-screens.js` |

- Terminal: no change.
- Tests. Regression: `moqui.back`'s rules (`WebrootVue.qvt.shell`), the picker's rules (`WebrootChat`), Back and the
  swipe in the bundled shell (`ShellSync`), and Chat's number transitions (`BasicCommsTexts`). Acceptance: `PhoneChat`
  (§2.3 at 390×844, offline, in the bundled shell).
- Left for a real phone (§2.3, §18.4): the back gesture on a device, the composer above the keyboard, the long press's
  vibration, the camera and gallery choosers.

#### Phase 2: notifications

Server first, then the Android app, then the desktop and the check screen.

The server side is done (2026-11-17, `ce99f253c`, `9fe2bde2e`): steps 2a to 2d and the server of 2h. `service/chat_push.rs` and
`ChatPushServices.xml` are the worker; `ChatUserActivity`, `ChatPushJob` and `PushOutbox` are in `ChatEntities.xml`;
the Settings, Chat list "Calls that ring through" keeps `CHAT_DND_ALLOWED`; System, Push Health is basic-comms'
`screen/System/PushHealth.xml`. The rules are regression tests of `chat_push_tests.rs` on the worker's clock.

The apps are done (2026-11-17): steps 2e to 2h ([Mobile-Client, Chat
notifications](../docs/ui/Mobile-Client.md#chat-notifications), [Desktop-Client,
Notifications](../docs/ui/Desktop-Client.md#notifications)). Each account on the phone has a push key pair and auth
secret of its own, sealed by the Keystore (`src/push.rs`), and a push is the account's whose keys open it, so a server
cannot make one that shows as another account's; Rust opens `enc` and Kotlin shows the chat (`ChatNotifications.kt`);
a ring to a keyed device is sealed too. UnifiedPush is in the app (`org.unifiedpush.android:connector`, an instance per
account, its messages opened in Rust with the account's same keys, `UnifiedPushService.kt`; Firebase stays unless
the user switches in the check screen). Left: the battery check finds Android's own optimization
only, not a maker's task killer; a chat push has no sender avatar; the desktop counts a thread reply at the mentions
level only when the event names the thread's starter, which the server's events do not yet; UnifiedPush with a real
distributor on a phone is not tried yet.

| Step | Work | Files |
|---|---|---|
| 2a (done) | Seed `ChatMention` in the base data. `ChatUserActivity`, written by `send#Message` and `react#Message`, removed by redaction and retention (§5.2). The feed service comes in phase 7 | `ChatEntities.xml`, `ChatServices.xml`, the base setup data |
| 2b (done) | `ChatPushJob` written by `append#Event`; the worker; `PushOutbox` with merge, retries, TTL, collapse and priority; the ring sent after the commit | new `crates/moqui-core/src/service/chat_push.rs`, `fcm.rs`, `web_push.rs`, `Calls.js` |
| 2c (done) | Rules 1 to 4 in the worker: level, mute as Mentions, read marker, `clear` on a read, DND hold and summary | `chat_push.rs`, `ChatServices.xml` (`mark#Read`) |
| 2d (done) | The device's push key at `register#PushDevice`; the encrypted payload; `ack#Push`; the System health screen | `NotificationServices.xml`, `web_push.rs`, `notify/push_crypto.rs`, a System screen |
| 2e (done) | Android: decrypt and show in Kotlin, channels, `MessagingStyle`, Reply and Mark read through the Rust outbox, `clear`, the permission and battery prompts, token retry | `PushService.kt`, `CallManager.kt`, a new notifications class, `Native.kt`, `crates/moqui-mobile/src` |
| 2f (done) | Desktop and mobile `notify` with tag and link; missed messages on start; the browser payload | `crates/moqui-desktop/src/bridge.rs`, `crates/moqui-mobile/src/bridge.js`, `MoquiLib.js` |
| 2g (done) | The check screen in the shell with its host command; "Send a test notification" | `Root.qvue`, `bridge.js`, `commands.rs` |
| 2h (done) | UnifiedPush: the app registers a distributor's endpoint where the phone has no Google Play; the worker sends the same encrypted payload to it (§7.4) | `web_push.rs` or a new `unified_push.rs`, `PushService.kt`, `NotificationServices.xml` |

- Reuse: the bridge delivery worker's pattern (`chat_bridge.rs`: lease, backoff, sweep, a round at a given time);
  `push_crypto::encrypt`; `WebPushSubscription`'s key fields; the Rust outbox kinds `chat-send` and `chat-read` with
  `FlushWorker.kt`; `KeyVault.kt`.
- Terminal: no change.
- Done when: §7.6. The server rules are regression specs; the phone cases are checks on real phones.

#### Phase 3: E2EE labels and the call rule

Done 2026-11-17 in `fdd1ab597` ([review](ui-review-basic-comms.md) COM-UI-001, 004, 013).

| Work | Files |
|---|---|
| `moqui.chat.e2eeLabel` (state, icon, banner, hint, Verify, one line), `trustOf`, `leftOutText`, `undecryptableText`; `ROOM_ICONS.CrtPrivate` is people | `WebrootChat.js` |
| The lock (with its check) in the header and the rail and the home list for E2EE only, the open lock otherwise; the banner under the header (closed per room) and at the room's start; the composer's Text message and network name; the bridges' encryption and the calls' setting (on and fixed in an encrypted room) in Room settings | `Chat.qvue`, `ChatPanel.qvue` |
| `Capabilities.encryption` (`None`, `Transport`, `BridgeDecrypted`, `EndToEnd`); `get#BridgeCapabilities`, the room's `bridges` and `find#Rooms`' `bridgeNetworks`; Telnyx SMS says `None` | `chat_bridge.rs`, `telnyx.rs`, `ChatServices.xml` |
| Rule 4: `roomRequiresE2ee` for an encrypted room; `set#RoomCallEncryption` refuses to turn it off there | `Calls.js`, `CallServices.xml` |
| COM-UI-013: the devices a share left out (`moqui.crypto.leftOut`), a room's device trust (`roomTrust`, asked only of a device already open), the withheld code in `decryptRoom` | `WebrootCrypto.js`, `Chat.qvue` |
| The terminal's one line | `ChatTui.xml` |
| Bundled screens regenerated | `bundled-screens.js` |

- SMS is `None`, not `Transport` (§6.3: "Never"): Telnyx is reached over TLS, but the provider, the carriers and the
  phone network read every text.
- Tests. Regression: the label's rules, the trust and the texts (`WebrootChat`); rule 4 (`CallEncryption`: an encrypted
  room's call, a phone and a guest refused, the setting fixed, an encrypted direct room, a plain direct room's choice);
  the terminal's line (`BasicCommsChat`, `BasicCommsEncrypted`, `BasicCommsBridges`); the bridges' encryption
  (`BasicCommsBridges`, `chat_bridge_tests`, moqui-channels). Acceptance: `ChatLabels` (each room state at 390×844 and
  1280×800 in the bundled shell, offline) and `EncryptedChatFlow`'s verified-only feature (who was left out, and
  `m.unverified` to the reader).
- Review (`1de64af65`): verifying another person (Members' Verify, the banner's and the "Not readable by" line's, over the
  to-device SAS/QR flow, in the shared `m-chat-verification` card My devices uses too; a request from another person in
  the open room); rule 3 (the checks, `verifiedUserProblem` with Verify or Send anyway, `onTrust` refreshes); the label in
  the members pane and Room settings; no lock in New room's Private, the not-available state, Mail and Remove the PIN; the
  "Not readable by" note kept per account; a members-only record room plain, SMS with another bridge, an inbound-only
  bridge; the withheld code when the sync fails; `requireE2ee` on an encrypted room's call. Tests: `WebrootChat`;
  acceptance `EncryptedChatFlow` (john.doe verifies b by emoji and the label has its check; b's new device stops the next
  send until Send anyway) and `ChatLabels` (the members pane, Verify from the banner).
- Second review (`b33d3213a`): Send anyway is kept in the browser per account and room (by device key or master key),
  never as the SDK's trust, so "only verified devices" stays strict; the check before a send is the one gate and then
  shares with all devices, so Verify then send works; a person verified by emoji only is covered; the sender's check
  needs a verified device (`verifiedSender`); edits and reactions ask too; incoming requests named, cancelled on close.
  Acceptance `EncryptedChatFlow` covers Verify then send and Send anyway under only verified devices.
- Left: the call screen's label (phase 4, from `e2eeLabel`'s call states); the label in a notification (phase 2's
  payload); Mail's line beside a person's chats (phase 8, the `mail` state); a request from another person when no
  encrypted room is open shows only on My devices.

Planned:
- The label state function in `moqui.chat`; the lock and the room type icons; the banners and composer hints of §6.2
  in the header, room info, room list and home list.
- `Capabilities.encryption` on the adapter trait, `get#BridgeCapabilities` and the Telnyx SMS adapter.
- Rule 4: `roomRequiresE2ee` for an encrypted room; the room's call setting shown on and fixed there.
- The sender's warning and the recipient's text for a message a device cannot read (COM-UI-013).
- Files: `WebrootChat.js`, `Chat.qvue`, `ChatPanel.qvue`, `chat_bridge.rs`, `crates/moqui-channels/src/telnyx.rs`,
  `Calls.js`, `CallServices.xml`.
- Terminal: the room's label as one line of text.
- Done when: §6 "Labels done when".

#### Phase 4: calls, flow and screen

- §8.3 items 1 to 7: the voice and video buttons; the call screen on the phone and as a side panel; voice to video;
  picture-in-picture; audio routing through Telecom; move to this device; call waiting.
- Files: `Chat.qvue`, `Calls/Call.qvue`, `WebrootCall.js`, `Calls.js`, `CallManager.kt`,
  `CallConnectionService.kt`, `MainActivity.kt`.
- Terminal: `Call.tui.json` keeps its controls; say there which of the new ones it has.
- Done when: the rows of §8.2 from Start to Leave the app, Move the call, Missed call and Second call, on real
  phones. The existing acceptance specs `CallFlow` and `EncryptedCallFlow` are extended for voice-only start and the
  upgrade.
- Status (2026-11-17): the web part and the Android part are built; §8.2 on real phones is the user's (§18.4).
  - Android part: every call in Telecom (a joined one placed as an outgoing self-managed call); audio routing through
    `CallEndpoint` and the call screen's audio button (`call_audio`, `call_audio_route`); the proximity wake lock;
    picture-in-picture for a video call; the foreground service's microphone and camera types; the page's `held` as
    Telecom's hold and Telecom's hold to the page (`hostHold`); call waiting through Telecom with Hold and answer on the
    phone's Answer; Move to and from the app through the page's move. Still `android.telecom`: the move to
    `androidx.core.telecom` is phase 9's, with the managed account (§9). The native call screen stays the page's
    `m-call-compact`. Tests: JVM `CallRulesTest` (routes, the ear, picture-in-picture, the service's types), instrumented
    `CallTelecomTest` (a joined call in Telecom, hold and resume, the routes), Rust `tell_script`, the page's
    `WebrootCall` (the host's audio, picture-in-picture, hold), acceptance `CallFlow` passes.
  - Built: §8.3 items 1, 2, 3, 4 (desktop), 6 and 7 (the status table there). `Chat.qvue`'s two buttons;
    `WebrootCall.js`'s side panel, `m-call-compact`, `addVideo`/`dropVideo` and the prompt, `openPip`, `moveHere`,
    `hold`/`resume` and `declineWithText`; `Calls.js` `move#Call` (finished on the new leg's `connected`) and
    `hold#OwnLeg`, a leg's `live` and `movedFromLegId` in the call's view, a direct call going on while its person has
    another leg; a socket keeps its four newest bindings, the oldest released (`notws/rtc.rs`: a page whose calls ended
    was refused its fifth).
  - Review fixes: a held leg is kept without its socket for 30 minutes and shown held again after a reload; Swap
    checks the held leg first and a refused hold keeps the call (and answers nothing); the voice-to-video prompt only
    in a call of two, for the other person's own change; the held banner inside the phone's call; one pending move per
    user; a move that never bound keeps no direct call going; the page releases a call's binding when done (`release`)
    and never loses its held one to eviction; joining another call leaves the first; the mobile host hears `held`, not
    `left`; no call button in a room with nobody here to ring; a phone caller gets no quick text, and the page says so.
  - Terminal: `CallTui` shows a person on hold; its comment says the rest is the browser's.
  - Tests. Regression: `Calls` (the move, a move left, the hold), `BasicCommsCalls` (both refused to a stranger through
    either mount, the terminal's On hold), `WebrootCall` (webroot, offline: the panel or the screen, the prompt, the
    move's offer and push, the quick text sealed in an encrypted room), `notws::rtc` (the bindings). Acceptance:
    `CallFlow` (a voice-only start and the upgrade and downgrade, Move to this device, call waiting with swap and a
    quick text), `EncryptedCallFlow` (an encrypted voice-only start and the upgrade).
  - Left: on the direct path a move goes through the media server first (a short gap); the picture-in-picture window
    in the desktop client's CEF is not checked; §8.2 on real phones (§18.4).

#### Phase 5: calls, network

1. The in-memory cases in `sfu/testnet.rs`, then the browser suite (§8.4). Record the first results.
2. The resilience items of §8.3 in the order the results give: RED and DTX, ICE restart, the 60 s keep, keepalives,
   the quality icon. Check the built items (FEC, RTX, TURN on 443).
3. The direct path for one-to-one calls (§8.3 item 15) with "Always relay my calls", then the browser suite on both
   paths.
- Files: `crates/moqui-rtc/src/sfu/`, `WebrootCall.js`, `Calls.js`, `MainActivity.kt`, a new acceptance suite.
- Done when: §8.5.
- Status (2026-11-17): built.
  1. In memory: `sfu/testnet.rs` shapes each link (loss, bursts, jitter, capacity, delay, a cut) and moves a client
     (a NAT's new port, a new interface, a rejoin); `sfu/network_tests.rs` holds the profiles as regression tests
     (under a second) and prints them all (`network_profiles`). Results in §8.4. The browser suite is
     `basic-comms/test/acceptance/client/CallNetwork.spec.js` with `scripts/call-network.sh` (namespaces, the server
     and the shaper, `scripts/call-network-control.py`); it skips without root and has not run yet.
  2. In the results' order: RED and the 120 ms audio hold, DTX; ICE restart (disconnected, failed, a network change,
     nothing coming in) before a join, with Android's `NetworkWatch` in `MainActivity.kt` (not built on a device
     here); the 60 s keep; the socket's ping every 10 s in a call; the quality icon (`m-call-network`). FEC, NACK and
     RTX are negotiated; TURN's order is the browser's ICE.
  3. The direct path with Always relay my calls (Settings, Calls) and SFrame on both paths; acceptance `CallDirect`
     passes.
  - Terminal: `Call.tui.json` (CallTui) shows none of it: no media, so no quality, Reconnecting or path; Settings, Calls'
    twin says Always relay is set in the browser.
  - Left: the browser suite's first run (root) and §8.5 on real phones; the estimate's reaction to a sudden drop (EDGE,
    the bandwidth step: 2.3 s and a 1.8 s freeze against 2 s and 1 s); video under 200 ms of jitter; TURN over TLS on
    443 live; a recorded cellular trace in the suite; the direct path's profiles with a page per namespace (the suite's
    pages share one Chromium).

#### Phase 6: message menu, reactions, Info

Done 2026-11-17 in `acc6d1b66` ([review](ui-review-basic-comms.md) COM-UI-008, 034, 035). No server transition was
needed: `timeline?eventSeq=` (get#Timeline, which checks the room) gives the one message with its delivery and the
members' read markers.

| Work | Files |
|---|---|
| One action list (`messageActions`, `chat-message-menu`) for the sheet, the hover toolbar's More and a desktop row's right click; who reacted and who voted by a long press (`pressStart`, `showPeople`); the Info dialog | `Chat.qvue` |
| `messageInfo`, `messageInfoLines` | `WebrootChat.js` |
| `moqui.emoji`, `m-emoji-picker`; its data file, license and notice | `WebrootEmoji.js`, `WebrootVue.qvt.css`, `libs/refresh.mjs`, `libs/emoji/`, `MANIFEST.json`, `NOTICES.md` |
| The script in the pages and the shell, the precompiled client components, the staged library | `qapps.xml`, `index.html`, `libs.txt`, `qvt.rs`, `WebrootVue.qvt.render.js` |
| Info per message in text | `ChatTui.xml`, `Chat.tui.json` |
| Bundled screens regenerated | `bundled-screens.js` |

- Tests. Regression: the Info rules (`WebrootChat`: read markers at or after, a member with receipts off not listed,
  the viewer's receipts off, "and N more", each network's delivery, a thread, never "not read"); the picker
  (`WebrootEmoji`: search, recent, groups); the one list in the sheet, More and the right click, and Info read again
  (`ShellSync`); the terminal's Info (`BasicCommsBridges`). Acceptance: `PhoneReactions` (react, who reacted, take it
  back, the picker and Info by made touch events at 390×844 in the bundled shell, offline).

- One action list for the sheet and the hover menu; right-click on a desktop row.
- `m-emoji-picker` in `webroot/js` with its data file; recent emoji in `storage`.
- Who reacted and who voted, by touch. Info, with the rules of §4.
- Files: `Chat.qvue`, a new `webroot/js` component (added to `client_components_are_precompiled`, the shell's
  `index.html` and `qapps.xml`), `libs/`.
- Terminal: Info as text; the picker is not in the terminal.
- Done when: §4.

#### Phase 7: details tabs and services, voice notes

- `ChatEventLink`; `find#RoomLinks`, `find#RoomReactions`, `find#ChatActivity`; the include and leave-out type lists
  of `find#RoomFiles`; the transitions `links`, `reactions` and `activity`.
- The tabbed details view; the viewer's swipe; the client-side sort for an encrypted room.
- Voice notes: record in the composer, send as an audio attachment, play in the Voice tab and in the message.
- Files: `ChatEntities.xml`, `ChatServices.xml`, `Chat.xml`, `Chat.qvue`.
- Terminal: Links and Activity as lists; no media grid.
- Done when: §5.

#### Phase 8: desktop gaps

- The palette source: people, phone rooms, rooms without the prefix in Comms. The palette in the desktop and mobile
  shell (shared with the [Shared/Gallery review](ui-review-shared-gallery.md), which owns the shell's keys).
- The Activity and Threads views on `find#ChatActivity`; folding; the Phone and Bridged groups.
- The mail line on a person's row (§3), from a new read-only service that finds each person's latest mail thread
  among the user's accounts.
- Files: `ChatScreenServices.xml` (`search#PaletteRooms`), `Root.qvue`, `Chat.qvue`.
- Done when: §3.

#### Phase 9: the user's phone number

- The user's steps: the port order and 10DLC in the Telnyx portal; the live tests on the real number.
- A managed `PhoneAccount` with an outgoing path, on the current Telecom library; names from the phone's contacts and
  from `call-context`; group texts.
- Files: `CallManager.kt`, `CallConnectionService.kt`, the Android manifest, `Sms.js`,
  `crates/moqui-channels/src/telnyx.rs`.
- Done when: §9.2.

#### Phase 10 and later

Each of these needs its design first:

| Work | Design needed |
|---|---|
| E2EE by default | §6.5: first sign-in on a new phone, and the Android check. What `createDirect` makes is decided |
| `sim-sms` | §10: MMS, a synced SIM room and its sends, SIM threads beside server rooms |
| `sim-dialer` and the contacts sync | §14.7, §14.8; what a server without mr-mantle offers |
| Unified contacts and the per-message network choice | §11; mail stays in the Inbox, with a line on a person's row (§3, §19.1) |

Then the other networks and the rest of §17, in this order:
1. The number on Telnyx, block and spam, contacts sync (the user stops using Google Messages and Cheogram).
2. Default phone app and the merged call log (the user stops using the Phone app).
3. GroupMe adapter (official API, low risk).
4. Signal adapter.
5. iMessage adapter (needs a Mac).
6. Disappearing messages, invite links, scheduled send, SMS members.
7. RCS: Telnyx business adapter, then C6c and the personal RCS bridge if §13.4 picks B or C.
8. The rest of §17.

**Done when** for each network: the user sends and gets text, a photo and a reaction with a real person on that network,
and the room shows the right network and E2EE label.

### 18.4 Checks on a real phone

These cannot be shown by a browser at phone width. They are collected here for each APK.

| After phase | Check |
|---|---|
| 1 | The keyboard does not cover the composer. A long press vibrates. Back closes a sheet, a pane and the room in order |
| 2 | The soak cases of §7.6. Reply and Mark read from a notification. A notification after a reboot |
| 4 | The rows of §8.2 between two phones and with the desktop app. A voice call on the earpiece with the screen off at the ear; a video call on the speaker; a Bluetooth and a wired headset taking the call at once and giving it back; the audio button's choices. Home during a video call: picture-in-picture with the video only; Home during a voice call: the call goes on with its microphone (and a video call's camera) for minutes, screen off. A second call while in one: it rings, Answer holds the first, Swap, and a car's or watch's hold. Move a call from the desktop to the phone and back with no gap. The lock screen. An encrypted call on the phone's WebView |
| 5 | Wi-Fi to mobile data during a call |
| 7 | A voice note recorded and played |

## 19. Decisions and open questions

### 19.1 Decided

| Date | Decision |
|---|---|
| 2026-11-16 | The user's number moves to Telnyx (§13.4, choice A). Cheogram's phone number role ends; its XMPP role stays |
| 2026-11-16 | The layout thresholds stay as built: 1024 px for the stacked layout, 640 px for bubbles, 900 px for a pane over the timeline (§1) |
| 2026-11-16 | Room rows keep the built swipes: right pins, left archives. Mark read is in the long-press sheet (§2.1) |
| 2026-11-16 | Every call in an encrypted room is E2EE, direct rooms included. No fallback to a plain call (§6.1 rule 4) |
| 2026-11-16 | Mute keeps its built meaning: ordinary messages are silent, a mention still notifies. The level Nothing is fully silent (§7.2 rule 1) |
| 2026-11-16 | A phone notification for an encrypted room shows the room and sender only; the text shows when the app opens. Decryption in the background is a later design (§6.5, §7.4) |
| 2026-11-16 | Mod+K stays the shell palette's key. Chat adds to its sources and adds no key listener of its own (§3) |
| 2026-11-16 | No chat component moves to a plain `.js` file. Helpers go to `moqui.chat`; a new shared component is an `m-` component (§2.2) |
| 2026-11-16 | E2EE by default: a new direct chat or private group is encrypted when every person in it has a crypto device, else plain with the plain banner. The plain direct rooms that exist stay; the picker opens the encrypted room when a person has both (§6.1 rule 5, §6.5) |
| 2026-11-16 | Search in an encrypted room: none at first, said in the pane before a query. Later, in memory over the messages the device has decrypted; nothing decrypted is written to disk for search (§6.5) |
| 2026-11-16 | A phone notification for an encrypted room keeps the room and sender only, with no later design for its text. Revisit only from daily use (§6.5, §7.4) |
| 2026-11-16 | A reaction to my message shows in Activity, with no push (§7.2 rule 4) |
| 2026-11-16 | SIM texts stay on the phone by default, with a per-user opt-in switch to sync them to the server as rooms, both in the first `sim-sms` version (§10) |
| 2026-11-16 | One-to-one calls go peer to peer first, between two users of this server in a direct room, with "Always relay my calls" and the media server as fallback; a third person moves the call to the media server (§8.3 item 15, phase 5) |
| 2026-11-16 | UnifiedPush is in the first release, in phase 2 beside FCM (§7.4, step 2h) |
| 2026-11-16 | Mail stays in the Inbox. A person's latest mail thread shows as a line on their direct chat's row and opens in the Inbox; no mail-only rows (§3, phase 8). If mail ever joins as rooms, the rules of §18.2 "Mail stays working" come with it |
| 2026-11-16 | During Do not disturb a call rings from a person on the user's allowed list (Chat settings, beside the hours), or as the same person's second call within 3 minutes (§7.2 rule 4) |

### 19.2 Open: the app

None. The questions asked here are decided in §19.1.

### 19.3 Open: other networks

- §13.4: number on Telnyx only (A), or keep a SIM line for personal RCS (B or C)?
- iMessage: is there a Mac that can stay on? Without one, iPhone users get SMS.
- Signal: accept that the server can read bridged Signal messages? The alternative is to keep the Signal app for those
  chats.
- Sidecars (signal-cli, BlueBubbles, mautrix): on the same server, or on a separate machine?
- Stories: wanted?
- After the port, does the phone keep a SIM line (for carrier calls and emergency calls), or only data?

### 19.4 Open: RCS

1. Do you accept that RCS from the server comes from a business agent name, not your own number?
2. Will you pay the Telnyx brand vetting ($100) and T-Mobile launch ($500) fees?
3. Which agent use case fits: Transactional or Multi-use?
4. Which countries do you need? Telnyx API onboarding is US only today.
5. Do you want RCS phase 3 (personal RCS bridge)? It has ToS risk and AGPL code.
6. If yes, do you accept a separate AGPL sidecar repo?
7. If yes, is it acceptable that the server stores bridged RCS as plaintext?
8. For `sim-sms` users, should the app warn and turn off RCS, or should it keep Google Messages as default and use only the hand-off?
9. Is a Twilio or Sinch adapter needed, or is Telnyx enough?

The prices, dates and platform rules in §16 and Appendix A come from the sources in Appendix B as read on
2026-11-16. They were not checked again in the revision. Check the source before a decision that depends on one.

# Appendix

## A. RCS research findings

Research for §16, 2026-11-16. Not checked again in the revision (§19.4).

### Finding 1: Personal RCS in a third-party default SMS app

| Item | Finding |
| --- | --- |
| Public Android RCS API | None. Google shelved planned RCS APIs in 2019. They never came back. |
| `SipDelegateManager` / IMS single registration (Android 12+) | AOSP says: "third party apps aren't allowed to access RCS single registration APIs". The app must be a **privileged** (preinstalled) app **and** the default SMS app. |
| `ImsRcsManager` | Gives registration state and capability exchange only. Some methods need carrier privileges. It does not send chat messages. |
| Carrier privileges | Not enough. The design doc says single registration is not available to downloadable apps with carrier privileges. |
| Google Jibe | US carriers (AT&T, T-Mobile, Verizon) run RCS on Google Jibe. Jibe provisioning needs DroidGuard (a Play Services internal API). Google blocks RCS on rooted phones through Play Integrity. |
| Hidden Google Messages API | XDA found one in 2021. It was restricted to Samsung apps. |
| Samsung | Samsung Messages was discontinued in the US on 2026-07-06. Google Messages is now the only Android RCS app on US carriers. |
| Android 16/17 | No new public API. Android 17 marks E2EE RCS rows as "restricted" in the telephony database. Only the default SMS app can read them. |
| EU DMA | Article 7 messaging interop does not cover Google Messages. The 2026-07-16 Article 6(7) decision makes Google expose read/write of SMS, MMS and RCS (via its Messages app) to "Qualified Services". Due in Android 18, by 2027-08-01. Scope is mainly AI services. It is not an RCS stack for third-party apps. |
| GrapheneOS | Plans its own RCS client with MLS "in the longer term". It still needs sandboxed Google Play for activation. |

**Important side effect for `sim-sms`:** Google Messages must be the default SMS app to use RCS. If the MR Moqui mobile app becomes the default, the number can stay registered for RCS. Then RCS messages from other people may not arrive. The user must turn off RCS chats in Google Messages first, or use https://messages.google.com/disable-chat.

### Finding 2: Business RCS (RBM) through a provider

| Item | Finding |
| --- | --- |
| Model | A2P and P2A only. One business agent talks to one person. No P2P. No group chats with a business. Users cannot react with emoji to business messages. |
| Can a person reply? | Yes, in the same 1:1 thread. Replies come to the agent webhook. |
| Sender identity | A verified agent (brand name, logo, check mark). Not a phone number. It is not the user's own number. |
| Agent use cases (Google) | OTP, Transactional, Promotional, Multi-use. Fixed at launch. |
| Billing (Google) | Per carrier rate card. Basic (text up to 160 chars), Single (rich), Conversational (24 h session). |
| Onboarding | Brand verification, then per-carrier launch approval. Third-party estimates: weeks to months. SMS fallback still needs A2P 10DLC in the US. |
| Telnyx availability | Yes. US agents via API. Other countries via a portal form. |
| Telnyx costs (US) | $100 brand vetting fee. $500 T-Mobile launch fee. $0.0065 per Rich Text segment, $0.016 per Rich Media message, plus carrier pass-through fees. No platform fee. |
| Twilio | GA since 2025-08-26. $0.0083 Rich, $0.022 Rich Media, plus carrier fees. Onboarding fee figures differ by source. |
| Others | Sinch, Vonage, AWS and others resell RBM with similar rules. |

**Telnyx RCS API shape:**

| Function | Telnyx API |
| --- | --- |
| Send | `POST /v2/messages/rcs` with `agent_id`, `to`, `messaging_profile_id`, `agent_message.content_message` (`text` or `rich_card`). |
| Rich content | `standalone_card`, `carousel_card` (2-10 cards), media by `file_url`. |
| Suggestions | `reply` (text + `postback_data`), `open_url_action`, `dial_action`, `view_location_action`. |
| Fallback | `sms_fallback` (text only) or `mms_fallback` (with `media_urls`). Response `data.type` tells RCS or SMS. |
| Inbound | `message.received` webhook to the URL on the RCS agent. `body` holds `text`, `user_file` (file + thumbnail), `location` or `suggestion_response`. |
| Status | `message.sent`, `message.finalized`, `message.read` to the messaging profile URL. |
| Read receipts | Inbound: `message.read`. Not all devices send it. |
| Typing | Google RBM supports user `IS_TYPING` and agent `IS_TYPING`/`READ` events. Telnyx docs do not show them yet. |
| Signature | `telnyx-signature-ed25519` + `telnyx-timestamp`. Same as the existing SMS adapter. |
| Capability check | RCS Capabilities endpoint (is a number RCS-capable). |
| Agent management | `/v2/rcs/brands`, `/v2/rcs/agents`, test devices, launch, carrier approvals. |

### Finding 3: RCS end-to-end encryption

| Item | Finding |
| --- | --- |
| Standard | GSMA Universal Profile 3.0 (2025-03-14) defines MLS (RFC 9420) for RCS. |
| Rollout | Apple and Google started cross-platform E2EE RCS in beta on 2026-05-11. Needs iOS 26.5 and current Google Messages, plus supported carriers. On by default. A lock icon shows. |
| Android to Android | Google Messages had its own E2EE since 2020. Google says "only Google Messages supports end-to-end encrypted RCS" on Android. |
| Third-party reach | None. MLS keys live in Google Messages or Apple Messages. No API exposes them. |
| Business RCS | Google: "Verified RCS chats between businesses and users aren't end-to-end encrypted." Provider, carrier and Google can see content. |
| Bridge (Finding 5) | The phone decrypts, then sends plaintext to the bridge over the web-pairing channel. The bridge sees plaintext. |

### Finding 4: Open-source RCS clients

| Project | Status | Practical? |
| --- | --- | --- |
| `Hirohumi/rust-rcs-client` (Apache-2.0) | Rust RCS client library. Works mainly on Chinese carriers. CPM standalone, HTTP file transfer, chatbots. | No for US/EU. It needs carrier auto-configuration and IMS access. Jibe needs DroidGuard. |
| `android-rcs/rcsjta` | Old GSMA Java RCS stack. Inactive. | No. Old profile. Same access problems. |
| AOSP Messaging | No RCS. | No. |
| GrapheneOS plan | Future RCS + MLS client. Still needs sandboxed Google Play. | Watch only. |
| Rooted phone + privileged app | Could get `PERFORM_IMS_SINGLE_REGISTRATION`. | No. Carrier and Jibe servers reject non-certified clients. Play Integrity blocks root. |

### Finding 5: Bridging Google Messages for Web

**How mautrix-gmessages works:**

- It is a Go Matrix puppeting bridge. Its protocol library is `pkg/libgm`.
- It acts like a Google Messages for Web session (messages.google.com).
- Login: Google account cookies, then the user taps a matching emoji in Google Messages on the phone. QR pairing seems disabled by Google.
- Messages go from the phone, through Google relay servers, to the bridge. Payloads use keys from pairing (`libgm/crypto`: AES-CTR, AES-GCM).
- It carries SMS, MMS and RCS (including E2EE RCS chats, after the phone decrypts them).

**Limits and risks:**

| Risk | Detail |
| --- | --- |
| License | AGPL-3.0, with exceptions only for Beeper and Element. Any port or linked use of `libgm` is AGPL. |
| ToS | Not an official Google interface. Google can change or block it at any time. Account action is possible. |
| Phone online | The phone must stay online. All messages go through Google Messages on the phone. |
| One web session | Only one Messages for Web session is active. The user's own browser session pauses the bridge. |
| Login fragility | Needs browser cookies. Device Bound Session Credentials in Chrome break it. |
| Conflict with `sim-sms` | Google Messages must stay the default SMS app. So the same phone cannot use `sim-sms` and this bridge at the same time. |
| E2EE | Breaks E2EE at the bridge. The bridge and the MR Moqui server store plaintext. |

**Could an MR Moqui `ChatBridgeAdapter` do the same?**

| Approach | Effort | License | Maintenance |
| --- | --- | --- | --- |
| A. Run mautrix-gmessages + a Matrix homeserver, then bridge Matrix to MR Moqui | High (two bridges) | AGPL stays in its own process | Upstream maintains the protocol |
| B. Small Go sidecar on `libgm` with a local JSON/WebSocket API, plus a Rust adapter | Medium | Sidecar is AGPL. Publish its source. Keep it a separate process and repo. Get a legal check. | Track `libgm` releases |
| C. Port `libgm` to Rust | High | A translation is a derivative work, so AGPL. A clean-room port is slow. | We must follow every Google protocol change |

Option B is the most practical if the user wants personal RCS.

## B. Sources

Android and personal RCS:

- AOSP, IMS single registration: https://source.android.com/docs/core/connect/ims-single-registration
- AOSP design PDF: https://source.android.com/static/docs/core/connect/ims_single_registration_v1_1_1.pdf
- ImsRcsManager reference: https://developer.android.com/reference/android/telephony/ims/ImsRcsManager
- SipDelegateManager source: https://android.googlesource.com/platform/frameworks/base/+/refs/heads/master/telephony/java/android/telephony/ims/SipDelegateManager.java
- MakeUseOf, 2026-11-01, "Google Messages is the only Android app allowed to send RCS": https://www.makeuseof.com/google-messages-is-the-only-android-app-allowed-to-send-rcs-and-that-should-worry-you/
- How-To Geek, SMS app article: https://www.howtogeek.com/the-sms-app-is-dead-why-google-messages-is-now-the-only-way-to-text-on-android/
- XDA, hidden RCS API (Samsung only): https://www.xda-developers.com/google-messages-rcs-api-third-party-apps/
- Android Central, Samsung Messages clone: https://www.androidcentral.com/phones/samsung-galaxy/this-app-claims-to-brings-back-samsung-messages-complete-with-rcs
- SyncTech, restricted RCS rows: https://www.synctech.com.au/faqs/why-are-some-messages-missing-from-the-backup/
- Google Messages RCS FAQ: https://support.google.com/messages/answer/9487020?hl=en
- Google RCS deregistration: https://messages.google.com/disable-chat
- 9to5Google, deregistration form: https://9to5google.com/2020/06/19/google-messages-disable-rcs-form/
- EU DMA Article 6(7) dossier: https://research.eutechreg.com/dossiers/google-android-interoperability-dma/
- EU DMA case summary: https://ec.europa.eu/competition/digital_markets_act/cases/202619/DMA_100220_2145.pdf
- Element, DMA scope: https://element.io/blog/the-eu-digital-markets-act-is-here/

Business RCS:

- Telnyx, send RCS: https://developers.telnyx.com/docs/messaging/messages/send-an-rcs-message
- Telnyx, RCS webhooks: https://developers.telnyx.com/docs/messaging/messages/receiving-rcs-webhooks
- Telnyx, RCS getting started: https://developers.telnyx.com/docs/messaging/messages/rcs-getting-started
- Telnyx, RCS API onboarding guide: https://support.telnyx.com/en/articles/16624885-rcs-api-onboarding-guide
- Telnyx, agent management API: https://telnyx.com/release-notes/rcs-agent-management-api-release
- Telnyx pricing: https://telnyx.com/pricing/messaging
- Google RBM, agent use cases: https://developers.google.com/business-communications/rcs-business-messaging/guides/learn/agent-use-cases
- Google RBM, user events: https://developers.google.com/business-communications/rcs-business-messaging/guides/build/events
- Google RBM, agent events: https://developers.google.com/business-communications/rcs-business-messaging/guides/build/events/send-events
- Google Messages, RCS chats with businesses: https://support.google.com/messages/answer/7513219?hl=en
- Twilio RCS GA: https://www.twilio.com/en-us/blog/products/launches/rcs-generally-available
- Twilio RCS page: https://www.twilio.com/en-us/messaging/channels/rcs
- Sinch, RCS pricing: https://sinch.com/blog/rcs-pricing-explained/
- Vonage, RBM use cases: https://api.support.vonage.com/hc/en-us/articles/15331569271836-RBM-Supported-Use-Cases
- SimplyRCS, provider price comparison (vendor source): https://simplyrcs.ai/blog/how-much-does-rcs-cost/

RCS E2EE:

- GSMA, UP 3.0 E2EE: https://www.gsma.com/newsroom/article/rcs-encryption-a-leap-towards-secure-and-interoperable-messaging/
- Apple Newsroom, 2026-05-11: https://www.apple.com/newsroom/2026/05/end-to-end-encrypted-rcs-messaging-begins-rolling-out-today-in-beta/
- Google Messages, E2EE help: https://support.google.com/messages/answer/10262381
- 9to5Google, UP 3.0: https://9to5google.com/2025/03/14/rcs-end-to-end-encryption-update/

Open-source clients:

- rust-rcs-client: https://github.com/Hirohumi/rust-rcs-client
- rcsjta: https://github.com/android-rcs/rcsjta
- GrapheneOS Messaging issue #44: https://github.com/GrapheneOS/Messaging/issues/44
- heise, GrapheneOS RCS plan: https://www.heise.de/en/news/GrapheneOS-plans-its-own-messenger-solution-with-RCS-and-E2EE-11445618.html

Bridges:

- mautrix-gmessages: https://github.com/mautrix/gmessages
- License exceptions: https://github.com/mautrix/gmessages/blob/main/LICENSE.exceptions
- Authentication docs: https://docs.mau.fi/bridges/go/gmessages/authentication.html
- libgm crypto: https://github.com/mautrix/gmessages/tree/main/pkg/libgm/crypto
- Go package: https://pkg.go.dev/go.mau.fi/mautrix-gmessages
