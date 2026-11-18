# Comms Messaging Plan

Plan, 2026-11-16; revised the same day against the source and the
[BasicComms UI review](ui-review-basic-comms.md) (findings `COM-UI-…`). One plan for the Comms chat as a messaging
app, for one app for all messaging, and for RCS. Phases 0 to 9 are built (2026-11-17), each reviewed by a separate
agent and fixed; what waits for real phones is in §18.4, and the user's own steps (the port, 10DLC) in §9. Phase 10 and
later (§18.3) need a design first. The build order is in §18.

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
| §3: desktop gaps | Built (phase 8): the palette in the app's shell, its rooms first in Comms, people and phone rooms; Activity, Threads, the groups and the mail line | 8, done |
| §4: reactions and the message menu | Built (phase 6): one action list, the emoji picker, who reacted and voted by touch, Info; a real phone's feel left | 6, done |
| §5: details tabs and new services | Built (phase 7): the tabs, the list services with one paging rule, voice notes; a voice note on a real phone left | 7, done |
| §6: E2EE policy and labels | Built (phase 3): the labels, the call rule, verifying people and the new-device warning. E2EE by default and first sign-in on a new device: designed (§6.5) | 3, done; 12 designed |
| §7: notifications | Built (phase 2): the push worker and its rules, encrypted payloads per device for FCM, Web Push and UnifiedPush, the Android app's notifications, the desktop's, the check screen; the soak tests on real phones left | 2, done |
| §8: voice and video calls | Built (phases 4 and 5): voice and video buttons, the call screen, voice to video, picture-in-picture, move, call waiting, Telecom on Android, the network tests and resilience, the direct path; EDGE and the bandwidth step not met, the browser network suite and real phones left | 4, 5, done |
| §9: the user's own phone number | Built (phase 9): group texts, the phone's dialer and Contacts through the app, contacts' names; §9.2 waits for the user's port, 10DLC and live tests | 9, built |
| §10: the phone's SIM texts (`sim-sms`) | Phone only with an opt-in sync, decided; MMS and the synced room need a design | 10+ |
| §11–§15: other networks and app equivalents | Plan; each network needs an adapter or a bridge | 10+ |
| §16: RCS | Research done; business RCS through the SMS provider modules, Twilio and Sinch first, designed as phase 10 (§18.3) | 10 |

# Part 1: The app

## 1. Where we start

**Built** (BasicComms `Chat.xml` and `Chat.qvue`, `Panel/ChatPanel.qvue`, the mobile app in `crates/moqui-mobile`):

| Area | What exists |
|---|---|
| Messages | Send, edit (24 hours by default: `chat@edit-window-minutes`, 0 for no limit), redact, reply, threads with a thread pane, forward, pins, polls, mentions, drafts, link previews (text only, three a message) |
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
| Phone number | SMS and MMS through a provider: Telnyx, Twilio or Sinch ([SMS](../docs/interfaces/SMS.md)); SIP trunks, `place#PhoneCall`, the Dialer, voicemail ([SIP](../docs/interfaces/SIP.md)). One room per outside number holds its texts, calls and voicemail |
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
| Quick switcher | Mod+K is the shell palette (`WebrootVue.qvt.js:10010`), and BasicComms gives it a Rooms source with the `#` prefix (`BasicComms.xml:34`, `search#PaletteRooms`). Do not add a second Ctrl+K listener (COM-UI-006). Add: people the user has no room with (choosing one starts the direct chat), phone-number rooms, and rooms without the prefix while Comms is the current app. Bring the palette to the desktop and mobile shell, which has none (`Root.qvue`) | Built (phase 8): the current app's prefixed sources lead (`palette.leadGroups`), Enter waits for their answer; people are a group of their own (People to message) that never leads, and a person's `dm` link opens an unsaved message, the room made by its first send; `Root.qvue` has the palette and the keymap |
| Activity view | A sidebar entry: mentions, reactions to my messages, and replies to my messages and in my threads, newest first. New service `find#ChatActivity` (§5) | Built (phase 8): the Activity entry beside the filters, more as it scrolls, an encrypted room's text decrypted in the page |
| Threads view | A sidebar entry for all threads I am in. `find#RoomThreads` with Mine works in one room only (`ChatServices.xml:2196`); across rooms the view reads the thread replies of `find#ChatActivity` | Built (phase 8): `find#ChatThreads`, the threads I started or replied in across rooms (mine with only my reply, and those where I wrote last), the latest reply first |
| Sidebar sections | Fold the groups. Add groups: Phone (rooms of type `CrtExternal`) and Bridged. Today the groups are Pinned and Recent, and a phone room shows only under All | Built (phase 8): Pinned, Recent, Phone, Bridged, each folding (per user, in the browser); a Phone filter, and All and Phone on the phone's home |
| Mail on a person's row | Where the user has a direct chat with a person, that person's latest mail thread shows on the row as a second line ("Email: subject") and opens in the Inbox, with "Email is not end-to-end encrypted" (§6.1 rule 7). No mail-only rows; mail stays in the Inbox (decided, §19). A user of this server matches by the mail address of their account; an outside person's line comes with unified contacts (§11.1). Also on the phone home | Built (phase 8): `find#DirectChatMail`, by the person's user account address only, whole addresses in To and Cc, the last 180 days, one query for the list (the latest 300 matching messages), read once a page for Chat and the panel (`moqui.chat.mailLines`). Designed as phase 22: the line on each room of a person (a phone room, a GroupMe direct conversation), by the addresses the user linked and, for an outside person, the contacts hook's |

**Done when:** in Comms, Mod+K, two letters and Enter open a room, on the web and in the desktop app. The Activity
view lists a mention and a reaction from another user (regression service spec for the feed; the views in an
acceptance client spec). Met in phase 8: acceptance `ChatPalette` (the web) and `ChatDesktop` (the app's shell).

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

Phase 15 adds "Remind me…" to the action list, after Mark unread from here, and "Send later…" to the composer's Send
(a long press on a phone, an arrow and a right click on a desktop) (§18.3).
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
| People | Members | Built (phase 7) |
| Media | `files` with `image/` and `video/` together. Grid of thumbnails. Full-screen viewer with swipe | Built (phase 7) |
| Files | `files`, all types except image, video and audio | Built (phase 7) |
| Voice | `files` with `audio/`. Inline player. Voice notes are recorded in the composer (`MediaRecorder`) and sent as an audio attachment | Built (phase 7); a real phone's recording left (§18.4) |
| Links | New `links` transition (§5.2) | Built (phase 7) |
| Reactions | New `reactions` transition (§5.2) | Built (phase 7) |
| Pinned | Pins | Built (phase 7) |

- `find#RoomFiles` took one `contentTypePrefix`. It has a list of prefixes to include and a list to leave out
  (`contentTypePrefixList`, `excludeContentTypePrefixList`; phase 7).
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

Status (phase 7): built. `ChatEventLink` (every link of a message, from `extract#LinkUrls` with `listed`, at most 50,
written by `send#Message`, replaced by `edit#Message`, removed by `clear#MessageContent`); `find#RoomLinks`,
`find#RoomReactions`, `find#ChatActivity`, and `get#MessageSummaries` that the last two and an encrypted room's
`find#RoomFiles` share for the ciphertext. Regression: `BasicCommsDetails` (each service's paging across a message with
more items than the limit, equal dates, a stranger, an encrypted room; the links written, replaced and removed; the
terminal's two lists) and `chat_link`'s listed links. Acceptance: `ChatDetails`.

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
line is on a person's mail line beside their chats (phase 8, the `mail` state of the label). Rule 5 is
phase 12 (§6.5).

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
| Encrypted, this device not set up (§6.5) | Lock | "Set up this device to read messages sent before it was added." with "Set up", in place of the banner above until done | — |
| Plain private room or plain direct chat | Open lock | "Not end-to-end encrypted. The server can read these messages." | — |
| Public channel | Open lock, no banner | — | — |
| SMS or MMS | Open lock | "Not end-to-end encrypted. Texts go through the phone network and the SMS provider." | "Text message" |
| Bridged network | Open lock | "Not end-to-end encrypted on this server. The bridge reads messages to pass them on." | The network's name |
| Phone call | Open lock | "Phone calls are not end-to-end encrypted." | — |
| Encrypted call | Lock | "This call is end-to-end encrypted." | — |

- The room type has its own icon in the room list (person, people, `#`, record, SMS), separate from the lock. Today a
  private room's type icon is a lock (`ROOM_ICONS.CrtPrivate`, `Chat.qvue:234`); it becomes a people icon.
- The label state is one function in `moqui.chat` (§2.2), from the room's `encrypted` flag, its type, its bridges and
  the devices' trust. The header, the room info, the room list, the call screen and a notification all use it. A
  direct chat not made yet (a dm link's unsaved first message) shows the label of the room its first send makes (§6.5).
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

### 6.5 E2EE by default

Decided (§19): `createDirect` and a new private group make an encrypted room when every person in it has a crypto
device, else a plain room with the plain banner; the plain direct rooms that exist stay, and when a person has both the
picker opens the encrypted one; no search in an encrypted room at first; a phone notification for an encrypted room
shows the room and sender only. Designed 2026-11-17 as phase 12 (§18.3), with the user's two answers of the same day
(§19.1): Chat makes the browser's device, and a plain direct chat that exists keeps opening, with a switch.

- **Rooms today.** An encrypted direct room is a room of its own beside the plain direct room of the same people
  (`directKey` with an `e2ee:` prefix, `create#Room`), and encryption cannot be turned on or off after the room is made.
  That stays.
- **Who has a crypto device.** A user whose device is not revoked, has uploaded its identity keys
  (`ChatDevice.curve25519Key`) and was seen in the last 90 days (`lastSeenDate`, set by each key upload and sync;
  property `chat@device-active-days`). A device that has not synced for that long would never read what is sent to it.
  A device Chat made by itself (`ChatDevice.madeAutomatically`, below) counts only once it was seen on a later day than
  it was made, or its user verified it (a cross-signing signature over it), so one private window does not make its user
  one with a device.
- **Devices are made when Chat opens (decided, §19.1).** Today a browser makes its device only when it first needs
  one, so few people would have one and the default would seldom apply. From phase 12 the Chat screen (on the web, and
  the bundled Chat of the desktop and Android apps) makes this browser's device once its room list shows, in idle time
  (`moqui.crypto.ensureDevice({automatic: true})`): the 7.8 MB module loads once per browser and is cached. Not the
  tray's chat panel, not a desktop app window (`elsewhere`), not a browser without WebAssembly or IndexedDB (logged; Chat
  works, its rooms plain). Such a device is marked `madeAutomatically`. So they do not pile up: a user's 50 devices count
  only those seen in `chat@device-active-days`, and the daily `clean#ChatToDevice` revokes each automatic device never
  verified and not seen for `chat@device-auto-expire-days` (30), logged, one at a time.
- **The rule, on the server.** `ChatScreenServices.create#ChatRoom`, which Chat's `createDirect` and `createRoom`
  transitions call. When `encrypted` is not given:
  - a direct room (one person or several): the encrypted direct room of exactly these people, when there is one, is
    answered whatever their devices now; else the plain one, when there is one, whatever their devices (decided,
    §19.1); else a new room, encrypted when the user and every member have a device (`check#DevicesReady`), else plain;
  - a private room: encrypted when every member has a device;
  - a public room: never.

  An explicit `encrypted` (New room's toggle once the user changed it) is kept as today. `create#Room` itself keeps
  `encrypted` false by default, so XMPP's direct chats, a call to a person (`Calls.js`), agents and the apps' own rooms
  do not change. The out-parameters gain `encrypted`, which the transitions answer. Two makes of the same people's direct
  room at once wait for each other (the people's `UserAccount` rows locked in one order); a twin made anyway is left, and
  the oldest is answered, as `directRoomFor` picks it.
- **What the client learns.** `get#EncryptionDefault` (`ChatKeyServices`; Chat's transition `encryptionDefault`)
  answers one bit for a set of people: whether a new room of the user and them would be encrypted. No device IDs,
  counts, names or dates. A user refused by an app's chat-target provider (`get#RefusedUsers`), unknown or disabled
  counts as one with no device, after the same device query, so its time tells nothing more. At most 50 people an ask and
  600 an hour a user (then `rateLimited` and false), counted atomically (a cache slot per person, `putIfAbsent`), shared
  by the cluster with a distributed cache factory and per node without one. The bit is what the room made would show
  anyway; the ask only lets the screen show it before the room exists.
- **The picker.** `moqui.chat.directRoomFor` answers the encrypted direct room of these people first, then the plain
  one, as `createDirect` does. Chat's dm link (the pickers, the palette's people, a person's Message) with no room yet
  opens the unsaved first message, which shows the label of the room its first send makes (`encryptionDefault`); that
  message is sent by the room's own send once `createDirect` made the room, so it is sealed in an encrypted room.
  `startChat` sends no `encrypted` for a named group (the server decides). New room: the Encrypted toggle follows
  `encryptionDefault` for the members picked until the user changes it; untouched, nothing is sent and the server
  decides.
- **Start an encrypted chat.** A plain direct room's header menu (Room) and its start offer "Start an encrypted chat"
  (one click at a time) when `encryptionDefault` says true for its people: `createDirect` with `encrypted` true makes or
  opens the encrypted direct room of the same people, and the picker opens that one from then on. The encrypted room's
  start then says "Earlier messages with {name} are in a chat that is not end-to-end encrypted", with Open. The plain
  room stays as it is.
- **First sign-in on a new device.** A new browser, phone or desktop app makes its device when Chat opens. Messages sent
  before that were encrypted for the user's other devices only. Until this device is set up:
  - the encrypted room shows "Set up this device to read messages sent before it was added", with Set up, in the
    banner's place (§6.2); each message it cannot read says "Sent before this device was set up", with Set up, in
    place of today's backup text (the `m.unverified` text stays). Sending and the messages sent after the device was
    made work at once: the members' browsers hear of the new device at their next sync (`device_lists.changed`);
  - set up means this device can read what was sent before it: it holds the key of the user's current key backup, or,
    when the server has no key backup to receive, it was verified with another of the user's devices (here, or by its
    cross-signing), which forwards the keys it holds. Verified while a backup exists is not yet set up: the card says
    "Verified. Getting your keys from the other device…", and after two minutes "Your other device did not send the
    keys", with Try again (the secrets asked again) and Use your recovery key; the bar and each message's Set up stay. It
    is needed only when the user has a key backup or another device (`moqui.chat.setupNeeded` on
    `moqui.crypto.setupStatus()`). A user's first device needs none; its encrypted rooms
    offer "Set up key backup" once (closed per account), which opens My devices.

  Set up opens `m-chat-device-setup` (`WebrootChat.js`; Chat, and My devices at its top), with two ways:
  - **Recovery key** (when there is a backup): the key typed or pasted, then Restore (`restoreBackup`); a wrong key is
    said plainly (`badRecoveryKey`, `wrongRecoveryKey`) and nothing is reset.
  - **Another device**: the user's other devices in use (seen in `chat@device-active-days`), by name and when last seen,
    the last seen first, each with Verify; the emoji (or QR) comparison
    runs in the card (`m-chat-verification`). The other device shows the request in Chat, in any room or the rail
    (new: today a request from one's own device shows on My devices only), naming the new device and when it was
    added, and on My devices. When it is done, this
    device asks its verified devices for the secrets it lacks (`OlmMachine.requestMissingSecretsIfNeeded`): the backup
    key (`m.megolm_backup.v1`) and the cross-signing keys. The SDK answers only a verified device of the same user. A
    backup key is used only when it opens the server's current backup (its public key); then the backup is restored as
    with the recovery key, and this device backs up to it. With no backup, the verified device forwards the room keys it
    holds when asked (the SDK's room key requests), while it is open.
  - Not now closes the card, kept per account and device in this browser; the messages keep their Set up.

  When done, the room decrypts again (`onRoomKeys`) and the bar goes. On a phone the card is a full-screen dialog
  (under 600 px wide), on a desktop 480 px.
- **My devices in the apps.** The desktop and Android apps have no My devices today (Desktop-Client, Known gaps), so
  their device can neither back up nor verify. Phase 12 bundles Settings/Devices (`Devices.qvue`) in the shell, with
  Root's route, the account menu's "My devices (encryption)" and a link to it shown in the shell (`SHELL_SCREENS`,
  `client/windows.rs`).
- **Device verification.** COM-UI-012 (emoji verification stalled) was fixed in phase 0 (`82affc535`): the device
  that did not start the comparison accepts it, a sync every 5 s while one is open, the states, a timeout and Try
  again; `CryptoFlow` checks it through `moqui.crypto`. COM-UI-014 (My devices 907 px wide on a 390 px phone) was fixed
  there too and checked by a probe page. Phase 12 retested both through the screen with a one-off client spec, not kept
  (two browser contexts of one user on one origin: Verify, Accept, Compare emoji, They match on My devices, then They
  differ; My devices at 390 × 844 with a long device name, light and dark) and found nothing open; QR, two origins, the
  keyboard and the TUI are not retested (the review's rows).
- **Search.** Built in phase 0 (COM-UI-011): the search pane of an encrypted room says so before a query, its field
  disabled. Later, search in memory over the messages the device has decrypted; nothing decrypted is written to disk
  for search. The offline store is plain SQLite and relies on the OS disk encryption
  ([Desktop-Client](../docs/ui/Desktop-Client.md)).
- **Message text in a phone notification.** Built (phase 2, §7.4): the room and the sender only. The crypto store is
  the WebView's, with one owner, so native code cannot decrypt a push. No hidden WebView and no crypto store in Rust is
  planned for it; revisit only if daily use asks for it.
- **Android.** The crypto device runs in the Android WebView (the shell page, as on the desktop), its store passphrase
  sealed by the Android Keystore. The check on a real phone is in §18.4 (phase 12), with
  [Exit checks, MD6](Mobile-Desktop-Exit-Checks.md#md6-the-android-app).

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
9. **Reminders** (phase 15). A reminder the user set pushes to each of their devices at its time, as a `notification`
   push with its own tag and collapse key. It is not dropped by a read marker or closed by a room's clear, it is held
   during do not disturb, and rule 5 applies to its text.
10. **One-time codes** (phase 21). A text with a code found in it carries `otp {code, label}` in its push only where
   the push carries the snippet (rule 5). The web shows a Copy action where the browser shows actions, and the Android
   app (its next APK) a Copy code action, with the lock screen's version hiding the code.

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
to the server's SMS and SIP provider (Twilio, Sinch or Telnyx). The server already does texts, MMS, calls and voicemail
for a number (§1): texts through the provider's messaging API, calls on standard SIP through its trunk.

### 9.1 Gaps

| Item | Today | Work |
|---|---|---|
| Port the number | — | A port order with the provider (a portal step by the user, not code). Then add the number in System, Voice: Numbers with the messaging bridge, the trunk and the user's routes |
| 10DLC | Required for texts from a US local number | The user registers the brand and campaign in the provider's portal ([SMS](../docs/interfaces/SMS.md)) |
| Providers | Telnyx (adapter `sms`), Twilio (`sms-twilio`) and Sinch (`sms-sinch`) built; none tested live with the provider. Sinch has no group texts on its Conversation API; Twilio's group texts use its Conversations API (US/CA) | Others test each provider live first (below) |
| Checked live | Calls, SMS and MMS are tested with fakes only, for every provider | Test on the real number with the chosen provider: text, MMS, a group text, call in, call out, voicemail. Only the user starts a test that calls or texts a real number |
| Push for messages | The app shows only the call ring | §7 |
| Phone rooms in the list | Rooms of type `CrtExternal` with an SMS icon in Chat; no icon on the phone home (`ChatPanel.qvue:153`); listed only under All | The icon on the home (§2.2); a Phone group in the sidebar and a phone filter on the phone layout (§3) |
| Start a text | The Text action on a call in Call history (`Calls/History.xml:303`) | A phone number in the new-chat picker (§2.2) |
| Group texts | Built (phase 9) | Group MMS: one room for a set of outside numbers |
| Android dialer and contacts | Built (phase 9): a managed `tel` calling account; the user turns it on in Android's settings | A managed account for the `tel` scheme with an outgoing path, so the dialer and contacts can call through the app with `place#PhoneCall` |
| Phone book | Built (phase 9): the phone's contacts (asked at the first search) and the server's `call-context` | Read Android contacts (asked on first use), on the phone only. Show names on phone rooms. Search contacts in the new-chat picker. On the server, names come from `call-context` where an app gives one (§14.8) |

Status (phase 9, 2026-11-17):
- **Group texts** ([SMS, Group texts](../docs/interfaces/SMS.md#group-texts)): a group MMS is one room for its set of
  outside numbers (sorted, `<ours>/<a>,<b>,...`), whoever of them writes first; a reply goes out as one group text to
  them all, each number checked and counted as a single text is, each recipient's receipt kept; new group rooms are
  limited a day for one sender and one of our numbers. The most recipients is the bridge's (`Capabilities`, the
  provider's `group_max`): Telnyx 8 (`/v2/messages/group_mms`), Twilio 9 (a Conversation of the room's numbers, at most 10
  participants with ours, US/CA +1 long codes), Sinch none (a group room on a Sinch bridge refuses to send and says why).
- **Providers** ([SMS](../docs/interfaces/SMS.md)): the SMS adapter is one provider-neutral core
  (`crates/moqui-channels/src/sms.rs`, the `SmsProvider` trait: the send rules and limits, refusals, the never-resend
  rule, the webhook's checks, `receive#Sms` and `note#DeliveryReceipt`) with a module per provider in
  `crates/moqui-channels/src/providers/`. Telnyx (adapter `sms`), Twilio (`sms-twilio`: 1:1 texts on the Messages API,
  group texts through the Conversations API) and Sinch (`sms-sinch`: the Conversation API's SMS and MMS channels, no
  group texts) are built; none is tested live with the provider, and the user has others test them first. A webhook
  signed with a secret (Twilio, Sinch) is verified on the core node, never on an edge node. Calls stay on SIP through
  the provider's trunk; the voice profiles `VppTwilio` and `VppSinch` are data only, not verified
  ([SIP](../docs/interfaces/SIP.md)).
- **The phone's dialer and Contacts** ([Mobile-Client, The phone's dialer and contacts](../docs/ui/Mobile-Client.md#the-phones-dialer-and-contacts)):
  the app registers a managed `tel` calling account while an account signed in on the phone has a number to call
  from. Android lists it under Calling accounts, where only the user turns it on: the app says so in its notification
  check, which opens that screen. A number dialed with it is handed to the app's Dialer and filled in; the user presses
  Call, which places it through `place#PhoneCall` (caller ID, fraud limits, emergency path); Telecom's own call is
  cancelled. Nothing is placed by itself, for cost and fraud safety: any app may place a call with the account and so
  choose the number. When several accounts can call and the current one cannot, the user picks which. Numbers Android
  treats as emergency numbers stay on the SIM.
- **Names**: phone rooms and the new-chat picker take names from the phone's contacts (read on the phone, asked at the
  first search, never sent to the server) and then from the server's `call-context` providers (`find#PhoneNames`, only
  for the user's own phone rooms, limited an hour), which is asked for every number so it cannot tell which the phone's
  contacts have.
- **The Telecom library.** The managed account and the self-managed calls stay on `android.telecom`.
  `androidx.core.telecom` (Core-Telecom) has no managed or call-provider account: its `CallsManager` makes only
  self-managed calls, so the dialer's path cannot be built on it. Moving the self-managed calls to Core-Telecom alone
  would be a rewrite of the calls' Telecom part (`CallConnectionService`, `CallAudio`, the hold), with nothing a user
  sees. Android deprecates the self-managed `ConnectionService` at API 36 ([Mobile-Desktop](Mobile-Desktop.md)), but it
  still works there; that move is later work, its own phase, when a target SDK needs it.

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
  and a room on the user's number at the provider.

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

### 11.1 One person across networks

Designed 2026-11-17 as phase 22 (§18.3).
- **A person** is one user's grouping of addresses: a user of this server (`user:`), an outside number (`tel:`, its SMS
  and RCS rooms), a mail address (`email:`) and a person on a bridge's network (`ext:`, GroupMe now). Each user's people
  are their own: made from their own rooms, their own merges and splits, and the app's contacts they may see (the
  contacts hook, Mantle's contact books and directory when mr-mantle is there). Nothing is shared or shown to anyone else.
- **Rooms stay separate**, one per network, each with its own E2EE label. There is no merged timeline.
- **Pick the network per message.** A person's room has a network button in its composer, which lists the person's ways
  (their rooms and the ones that can be started), the best first. Choosing one opens that room and moves the draft there.
- **Best path first**: Moqui chat when the person is a user of this server (their encrypted direct room first, as §6.5
  has it); else the network they last wrote on; else SMS from the user's number; else mail in the Inbox. A blocked
  number (phase 11) is listed dimmed and never the best; a person the user blocked has no way.
- **Mail stays in the Inbox** (§19.1): a person's latest mail thread is a line on their rooms and opens in the Inbox.
- **Privacy**: a user's phone number is never learned through chat (§14.4); the hook is asked only about the addresses
  of the user's own rooms, and an address the user types is matched only against what they are shown and joins a user
  of this server only when already shown, so the contacts cannot be probed by typing.

## 12. The protocols MR Moqui keeps

These are built ([Future-Plans, Comms](Future-Plans.md#comms)). The app must keep them working and show them in the one
inbox.

| Protocol | Who it reaches | E2EE | Notes |
|---|---|---|---|
| Moqui chat (web, desktop, Android) | MR Moqui users | Yes in encrypted rooms (Megolm) | The home network |
| WebRTC calls | MR Moqui users and guests by link | Yes (SFrame) | §8 |
| XMPP (C3a) | People on XMPP apps (Conversations, Cheogram, Monal, Dino, Gajim) with an account on this server | OMEMO between XMPP apps | Federation to other XMPP servers is C3b, not built |
| SIP | Desk phones, soft phones, the phone network through a trunk | No | Calls, IVR, voicemail |
| SMS and MMS | Any phone number, through the SMS provider (Telnyx, Twilio or Sinch) | No | One room per outside number with its texts, calls and voicemail |
| Mail | Any mail address | No (TLS only in transit) | The Comms Inbox; a person's mail can show beside their chats |
| Matrix (C6a client API, C6b federation; planned) | Matrix users | Megolm possible | The appservice API (C6c) is also the way to host Matrix bridges (§13.2) |

## 13. Reaching each network

### 13.1 Summary

| App | How the app reaches its people | Official? | E2EE through MR Moqui | Needs |
|---|---|---|---|---|
| Google Messages: SMS, MMS | The user's number, ported to an SMS and SIP provider: Twilio, Sinch or Telnyx (decided) | Yes | No | Built; live checks (§9) |
| Google Messages: RCS | Business RCS through the same provider (Twilio and Sinch first); personal RCS only through a Google Messages for Web bridge | Business: yes. Personal: no | No | §16; see §13.4 |
| iMessage | A Mac the user owns runs BlueBubbles Server; an adapter talks to its API | No | No | An always-on Mac signed in to the Apple ID |
| Signal | A linked device (like Signal Desktop) through `signal-cli` | No (Signal allows linked devices; the client is third party) | No, the bridge decrypts | A Signal account on a phone to link from |
| GroupMe | GroupMe's public API v3 with the user's access token | Yes | GroupMe has no E2EE | The user's token (`request_secret`) |
| Cheogram | Replaced: the number moves to an SMS and SIP provider (decided). XMPP contacts reach the user on this server's XMPP | — | OMEMO only between XMPP apps | C3b federation for contacts on other XMPP servers |

### 13.2 How bridges plug in

All networks go through the chat bridge layer ([Chat-Bridges](../docs/interfaces/Chat-Bridges.md)): a
`ChatBridgeAdapter` (the trait is in `crates/moqui-core/src/service/chat_bridge.rs`; the adapters are in
`crates/moqui-channels`, which has one today, SMS: a neutral core with a module per provider), one room per outside
conversation, the bridge chip and the
E2EE label from the adapter's `encryption` capability (§6.2). A new network also needs its bridge type (`Cbt…`) where
`ChatEntities.xml` has none.

| Way | Use for | Why |
|---|---|---|
| A Rust adapter on a documented API | GroupMe (REST), BlueBubbles (REST and its event socket), business RCS in the SMS provider modules (§16) | Small, no extra process, same as the SMS adapter |
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

**GroupMe** (designed as phase 14, §18.3)
- Public API v3 (`api.groupme.com/v3`): groups, direct messages, likes, attachments, members. A user token reads groups,
  members and Topics; the push channel gives new messages.
- Adapter `groupme` (`crates/moqui-channels/src/groupme/`), bridge type `CbtGroupMe`, encryption `None`:
  - each group, each Topic and each direct conversation is a private room of the bridge's owner;
  - a like is a heart reaction both ways, and `@all` is `@room` both ways;
  - pictures go out through GroupMe's image service.
- New messages come through GroupMe's push channel: one outbound HTTPS connection from the core node, so no webhook and
  no edge node. A catch-up through the messages API after each reconnect is the polling fallback.
- Risk: low; the API is old but works. GroupMe has no E2EE, so nothing is lost. Direct messages, Topics and the push
  event types beyond `line.create` are documented only by the community (phase 14, "Not confirmed").

**Google Messages and RCS** (details in §16)
- SMS and MMS: through the ported number on the SMS provider. Group texts: §9. Text members of a room (GroupMe's SMS
  join): phase 18.
- Business RCS (RBM) through the same provider (Twilio and Sinch first): verified brand sender, rich cards, replies in
  the thread. Not person to person.
- Personal RCS: only a bridge to Google Messages for Web (`mautrix-gmessages` behind C6c). It needs a phone with Google
  Messages as the default SMS app and the number on its SIM.

**Cheogram**
- Its phone number role moves to the SMS and SIP provider (decided). Its XMPP role stays: the user's XMPP contacts on this server reach the
  user in the app; XMPP apps (including Cheogram itself) can still sign in to this server.
- C3b federation brings contacts on other XMPP servers.

### 13.4 The RCS conflict

Personal RCS needs the number on a SIM in a phone with Google Messages (§16). The user's number is moving to an SMS and
SIP provider (Twilio, Sinch or Telnyx), so that number cannot have personal RCS. The choices:

| Choice | Result |
|---|---|
| A. Number on the provider (decided) | SMS, MMS, calls and voicemail in the app. Others see SMS, not RCS. Business RCS possible for a brand sender, through the same provider |
| B. Number stays on a SIM, Google Messages plus the bridge | Personal RCS through the bridge; SMS and MMS through the bridge too; no provider calls on that number |
| C. Both: the provider's number for the user, a second SIM line with RCS | Two numbers; people must know which to use |

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
| SMS, MMS, group MMS | SMS and MMS through the provider; group texts | Built; group texts in phase 9 (none on Sinch, §9) |
| RCS chat | Business RCS; personal RCS by bridge (§13.4) | §16; business RCS phase 10, its cards, chips, read and labels phase 13 |
| Default SMS app | `sim-sms` build | §10 |
| Scheduled send | Send later on the composer's Send (a long press on a phone, an arrow on a desktop): the server holds the message (`ChatScheduledMessage`) and sends it at its time as the sender, through the normal path, so bridges and their send checks apply then, as mail's due drafts are sent (`send#DueDrafts`). The room lists its scheduled messages: edit, send now, cancel. In an encrypted room, which the server cannot encrypt for, the sender's own device holds it and sends it, the server keeping a placeholder with no text | Designed: phase 15 (§18.3) |
| Spam protection, block, report | Block a number: its calls and texts, on the user's Blocked list (a `CallerList`, Settings, Call handling); its texts kept out of sight in "Spam & blocked". Block a person in chat: their messages hidden in shared rooms, their direct rooms and invites refused. Spam filter for outside numbers: local rules (first contact, links, keywords, numbers, reports here), on by default; the provider's number lookup optional and off. Not spam and Report spam | Designed: phase 11 (§18.3); the caller lists are built |
| One-time codes: copy, delete after 24 h | Codes found in incoming texts (a keyword near 4 to 8 digits, with care for numbers, prices and dates); a Copy chip on the message and a Copy button in the notification (web push where the browser shows actions; Android in its next APK); an optional delete after 24 h through the disappearing-message timer, with Keep | Built: phase 21 (§18.3) |
| Message reminders | "Remind me" on a message (in 20 minutes, in 1 hour, tomorrow, a time picked): a push through the chat push worker and an Activity row then; pending reminders listed at the top of Activity and cancelled there. A reminder keeps the room and the message's number only, never its text | Designed: phase 15 (§18.3) |
| Smart replies, writing help | Suggested replies from `moqui-llm`, plain rooms only (never encrypted rooms) | New |
| Photo quality choice | Compressed (2048 px JPEG, metadata dropped) by default, or Original, per send; a room that goes out by text fits the photo under its MMS limit. Today every photo goes at full size | Built: phase 21 (§18.3) |
| Messages for web | The web client and the desktop app | Built |

### 14.3 iMessage and FaceTime

| Feature | Equivalent | Status |
|---|---|---|
| Blue-bubble chat with iPhone users | BlueBubbles bridge (§13.3), else SMS | New |
| Tapbacks | Reactions | Built |
| Edit and unsend within minutes | Edit, redact | Built |
| Send later | Scheduled send (§14.2); in an encrypted room by the sender's own device | Designed: phase 15 (§18.3) |
| Message effects, stickers, GIFs | Sticker packs, local: the server's (an administrator's) and each user's own; PNG, WebP or GIF, animated too; a picker beside the emoji picker; a sticker shows large, and goes to bridges as its picture or as text. GIF search waits for a provider; effects: no | Stickers built: phase 20 (§18.3); GIFs later |
| Share location, live location | A location message with no map: coordinates or an address, a label, "Open in maps" (`geo:` in the Android app, OpenStreetMap on the web) and Copy; the device's position or a typed place, no geocoding. Bridges get the text with the link; Sinch RCS its location message. Live location waits for the Maps app ([Maps](Maps.md)) | Location built: phase 20 (§18.3); live location after Maps |
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
| Disappearing messages | A per-room timer from 30 s to 4 weeks, from when a message is sent, set by a direct room's member or another room's moderator; a change is an event in the room. The clients delete at the time, the server by a due-time index and a worker that wakes at the next due time; the room's feed tells the other devices. Refused in bridged, phone and record rooms | Designed: phase 16 (§18.3) |
| View-once media | One photo or video that each recipient opens once (a 2-minute window), with `FLAG_SECURE` on Android while open; the sender sees Opened; the server deletes the file when all have opened, or after 14 days | Designed: phase 16 (§18.3) |
| Note to self | A room with only the user | New (check if a one-person room works) |
| Usernames, phone number hidden | Usernames; a user's phone number is not in chat's data and is never shown to other MR Moqui users. An outside number shows to the members of its own room. Unified contacts keep it: a person who is a user shows only the numbers of the viewer's own rooms, and chat never links a number to a user from its own or voice's data (phase 22) | Built |
| Linked devices list | Apps and devices | Built |
| Registration lock, PIN | Device keys, password, recovery key for key backup | Built |
| Screen security (no screenshots) | Android `FLAG_SECURE`: a user setting (off, in encrypted rooms, the whole app) and a per-room switch in an encrypted room, through phase 16's reasons (`app:`, `room:`) | Built: phase 21 (§18.3) |
| Group links with admin approval | Room invite links and QR codes (`/join/<token>`) in private and public rooms, made by a room's moderators (any member of a public room), with an expiry, a use limit and optional approval; revoked at once. Join requests in the moderators' queue | Designed: phase 17 (§18.3) |
| Stories | Later, if wanted | Later |
| Sealed sender | Not possible: the server sees who sends. The UI says so (§6) | Not planned |
| Proxy for censored networks | TURN on 443 and the web socket on 443 | Built; live check in §8 |

### 14.5 GroupMe

| Feature | Equivalent | Status |
|---|---|---|
| Large groups | Public and private rooms; test with 5,000 members | Built; load test new |
| Join by SMS without the app | Text members of a private room: a number a moderator adds (with the person's consent) gets each message as a text, "Ann: …", from one of our numbers marked for room texts, and its replies post to the room as that number. One room per outside number on each of our numbers; STOP leaves. Never in an encrypted room | Designed: phase 18 (§18.3) |
| Group invite links and QR | Invite links and their QR codes; a deep link into the Android app (`org.moqui.mobile:/join`) | Designed: phase 17 (§18.3) |
| Likes | Heart reaction; across the GroupMe bridge a like is a heart both ways | Built; the bridge in phase 14 |
| Topics (subgroups) | Threads, or linked sub-rooms. A bridged Topic is a room of its own, "Group › Topic" | Built (threads); the bridge in phase 14 |
| Events with RSVP | An event message (title, start and end in a time zone, place, details) with Going / Maybe / No, the counts and who answered, a reminder before it, a calendar entry through an app's `chat-calendar` provider or an `.ics` file (the phone's calendar app on Android); changes and a cancellation post to the room; bridges get the text. Not in encrypted rooms, as polls | Built: phase 19 (§18.3) |
| Polls | Polls | Built |
| Gallery | Media tab | §5 |
| Mute, @all | Mute, `@room`; across the GroupMe bridge `@all` and `@room` map both ways | Built; the bridge in phase 14 |
| Directory and join requests | Browse rooms; a private room its owner lists in Browse takes join requests ("Ask to join"), as a link with approval does | Browse built; requests designed: phase 17 (§18.3) |

### 14.6 Cheogram (XMPP)

| Feature | Equivalent | Status |
|---|---|---|
| Phone number: SMS, MMS, calls, voicemail | The number on the SMS and SIP provider | Built; §9 |
| Calls in the Android dialer | Android `PhoneAccount` | §9 |
| XMPP chat with OMEMO | This server's XMPP for XMPP contacts; Moqui chat with Megolm in the app | Built (C3a) |
| Contacts on other XMPP servers | C3b federation | Not built |
| Group chats (MUC) | Rooms are MUC rooms | Built |
| Ad-hoc commands (XEP-0050) | Not needed after the number moves | Not planned |
| Stickers, reactions, replies, threads | Built (stickers: phase 20) | Built |
| Push without Google (UnifiedPush) | UnifiedPush option | §7.4 |

### 14.7 Android Phone app

The app can become the Android default phone app (the `ROLE_DIALER` role). It then handles every call on the phone:
the SIM's carrier calls and the server's calls (Moqui calls and the user's number on the SIP provider).

| Feature | Equivalent | Status |
|---|---|---|
| Dialer keypad, `tel:` links | Dialer screen (exists on the web; the desktop app can be the `tel:` handler), `tel:` and `DIAL` intents on Android | Dialer built; Android intents new |
| In-call screen for SIM calls | An `InCallService`: the same call screen as Moqui calls (§8) for carrier calls | New |
| Pick the line per call | SIM line, the provider's number (through `place#PhoneCall`), or a Moqui call to a user. Default by contact, last used | New |
| Incoming carrier calls | Ring screen over the lock screen (exists for Moqui calls), with the caller's contact name | Ring built; carrier calls new |
| Call log | One history: SIM calls (Android `CallLog`), the provider number's calls and Moqui calls (server call history), merged and by person | New |
| Voicemail | Server voicemail in the number's room (built). The carrier's voicemail: dial-in only (visual voicemail of carriers is closed) | Server voicemail built |
| Spam and unknown callers | Android `CallScreeningService`: block the numbers on the user's Blocked list (§14.2, phase 11; the server already refuses their calls), silence unknown callers as an option | New |
| Hold, mute, speaker, Bluetooth, merge, swap | Android Telecom for SIM calls; the server for the provider number's calls and Moqui calls | Partly built |
| Emergency calls | Always on the SIM and the carrier, through Android, never through the server. The app must not block or delay them | Required |
| Calls from the Contacts app and the car | A managed Android `PhoneAccount` for the provider's number and Moqui calls, so other apps and cars can call through them | §9 |

**Build:** a cargo feature `sim-dialer`, like `sim-sms`, with its Android parts in an optional plugin crate. The default
APK has no `CALL_PHONE`, `READ_CALL_LOG` or `ANSWER_PHONE_CALLS` permissions. Google Play allows call log permissions only
for the default phone app, so the call log is in this build only. `sim-sms` and `sim-dialer` can ship in one "phone" APK.

**Done when:** with the role given, a carrier call in and out, a call in and out on the provider's number, and a Moqui call all use the
app's screens and appear in one call log; an emergency number goes to the carrier at once.

### 14.8 Android Contacts app

| Feature | Equivalent | Status |
|---|---|---|
| Contact list and details | A Contacts screen in the app: name, photo, numbers, emails, and each network the person is on (§11 unified contacts) | New |
| Contacts synced to the server | The server's CardDAV contact books (Mantle's `ContactDavServices`, [Sync](../docs/interfaces/Sync.md)): personal books read and write, shared books read only | Built on a server with the mr-mantle add-on. The framework alone keeps no contacts: there the Contacts screen shows the phone's own contacts and MR Moqui users only |
| Contacts in other Android apps | A built-in Android sync adapter: an Android account "MR Moqui" that syncs the CardDAV books into Android's contact store, both ways. The phone app, the keyboard and the car see them | New |
| Existing phone contacts | Import once from the phone's other accounts into the personal book, with duplicates merged | New |
| Caller name and photo | From the synced contacts on the phone, and on the server from the `call-context` provider that an app implements, for SIM, provider and Moqui calls and for texts | New |
| Favorites, groups | Favorites (starred), contact groups as CardDAV categories | New |
| Share a contact | A vCard message in chat; save a received vCard | New |
| Network per person | Link a contact to an MR Moqui user, an XMPP address, a Signal or GroupMe identity, a phone number (§11) | Designed: phase 22 (§11.1), chat's side: users, numbers, mail addresses and GroupMe people, grouped per user, with the app's contacts through a hook. XMPP addresses of other servers and Signal identities come with C3b and their adapter (`ext:`) |

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
| GroupMe | The API's message history per group. Phase 14 catches a room up from its last message only | Full history |
| Cheogram | The XMPP archive (MAM) of the old account, if its server keeps it; or an export | Best effort |
| Google Messages | Through the phone's SMS store (`sim-sms` import); RCS history only through the bridge | RCS history mostly lost |

Each import is idempotent (an outside message id per network), so it can run again.

# Part 3: RCS

## 16. RCS

Scope: the mobile app (`crates/moqui-mobile`) and the server chat bridges (`crates/moqui-channels`,
`crates/moqui-core/src/service/chat_bridge.rs`).

### 16.1 Summary

1. A third-party Android app cannot send or receive RCS on the user's own number. Android has no public RCS API. Only Google Messages can do it.
2. The server can send and receive RCS as a verified **business** sender through the SMS provider it already uses. Twilio, Sinch and Telnyx each resell Google's RCS Business Messaging (RBM), with a fallback to SMS or MMS. People can reply in a 1:1 thread. Brand vetting and carrier approval are necessary.
3. RCS end-to-end encryption (MLS, Universal Profile 3.0) is live only in Google Messages and Apple Messages. Business RCS is never end-to-end encrypted.
4. A bridge to Google Messages for Web (the mautrix-gmessages approach) gives personal RCS on the user's number. It is unofficial, AGPL, and the bridge reads all plaintext.
5. Recommendation: build business RCS into the SMS provider modules, on the neutral SMS core, Twilio and Sinch first; Telnyx RCS is optional. Add a personal RCS bridge only as an opt-in, labelled experimental feature.

### 16.2 Findings

The five research findings behind this section are in [Appendix A](#a-rcs-research-findings): personal RCS in a
third-party app, business RCS through a provider, RCS end-to-end encryption, open-source RCS clients, and bridging
Google Messages for Web.

### 16.3 Options

| Option | How it works | Who can use it | E2EE? | Cost / approval | Effort | Risks | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Native RCS in the app | App talks RCS to carrier/Jibe | Nobody (no API) | n/a | Not possible | n/a | n/a | Do not build |
| 2. Hand-off to Google Messages | App opens the thread in Google Messages by intent | Any user with Google Messages | Yes, in Google Messages | Free | Low | Two apps; the thread is not in MR Moqui | Build as fallback |
| 3. Business RCS through the SMS providers | The provider's RBM sender, in the same module as its SMS: Twilio and Sinch, Telnyx optional | Operators with a verified brand | No | Brand vetting, carrier launch, per-message fees (Appendix A) | Medium for the first, small for each next | Approval time; A2P only; not the user's number | **Build first** (Twilio and Sinch) |
| 4. Other RBM providers (Vonage, AWS) | A provider module of its own, as 3 | Same as 3 | No | Similar | Medium each | Same as 3 | Later, if needed |
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

#### How each provider carries RCS

Business RCS goes through the same provider modules as SMS (§9, Providers), each on Google's RBM model that the
provider resells. The facts and their sources are in Appendix A, Finding 2.

| | Twilio | Sinch | Telnyx (optional) |
| --- | --- | --- | --- |
| Sender | An RCS sender in a Messaging Service, beside the service's numbers | The Conversation API's `RCS` channel on the app | An RCS agent (`agent_id`) on a messaging profile |
| Send | The Messages API with `MessagingServiceSid` and an E.164 `To` | `messages:send` with an `RCS` channel identity, as SMS and MMS are sent | `POST /v2/messages/rcs` |
| Fallback | Automatic: SMS or MMS from the service's numbers when the phone has no RCS | `channel_priority_order` `["RCS","SMS"]` (MMS for a file); `SWITCHING_CHANNEL` in the delivery report | `sms_fallback` or `mms_fallback`; the answer's `data.type` says which went |
| No fallback | A `To` of `rcs:+1…` | `RCS` alone in the order | No fallback fields |
| Rich content | `ContentSid` (a Content Template: cards, carousels, suggestions) | The Conversation API's rich message kinds (cards, carousels, choices) | `rich_card`, `carousel_card`, suggestions |
| Reports | Status callbacks as for SMS, with `read` | `MESSAGE_DELIVERY`: `DELIVERED`, `READ`, `FAILED`, `SWITCHING_CHANNEL` | `message.sent`, `message.finalized`, `message.read` |
| The path it went | The callback's `From`: `rcs:<SenderId>` when it went as RCS, else the fallback number (Twilio documents no other field); `FallbackFrom` names the fallback number | The report's `channel_identity.channel` (`RCS`, `SMS`, `MMS`; that a report after a switch names the new channel is not documented) | The answer's `data.type` |
| Signature | `X-Twilio-Signature`, checked on the core node | `x-sinch-webhook-signature`, checked on the core node | Ed25519, as its SMS |

#### RCS phase 1: business RCS in the provider modules (server)

1. RCS is not a module of its own, and the `telnyx_common` module planned before is not needed: the neutral core
   (`crates/moqui-channels/src/sms.rs`) keeps the send rules and limits, the refusals, the never-resend rule, the
   webhook's checks (POST, the replay cache, the provider's `verify`, on the core node for a secret-signed provider)
   and the `receive#Sms` and `note#DeliveryReceipt` calls. Each provider module gets its RCS part: `providers/twilio.rs` and `providers/sinch.rs`
   first, `providers/telnyx.rs` optional. The `SmsProvider` trait gains what RCS needs (the channel of a send, rich
   content, the path a message went).
2. Add a chat bridge type for RCS (for example `CbtRcs`; `ChatEntities.xml` has none), with adapter names
   `rcs-twilio`, `rcs-sinch` and `rcs-telnyx`, chosen by `ChatBridge.adapterName` as the SMS ones are. Its own bridge,
   since its sender is the brand agent, not a number: the other person's phone shows it in a thread apart from the
   number's texts. Keep `configJson` encrypted, like the SMS adapters.
3. Config keys: the provider's SMS keys, plus its RCS sender: Twilio's `messagingServiceSid` with the RCS sender in it
   and `fallbackFrom`, the bridge's home number; Sinch's `appId` with the RCS channel set up, and `smsSender` (the home
   number) and `mmsSender` for the fallback; `smsFallback` (true; false sends RCS only); Telnyx's `agentId`,
   `messagingProfileId`, `smsFallbackFrom`, `mmsFallback`. The bridge's numbers are numbers whose `messagingBridgeId`
   is the RCS bridge (phase 10, §18.3).
4. Sinch's inbound handling must accept the `RCS` channel: today `providers/sinch.rs` leaves a message and an opt-out
   on any channel but SMS and MMS. An RCS identity is a phone number, so the RCS bridge keys a room by the other
   person's number, as the SMS bridge does.
5. Send through the table above: the provider's fallback by default; no fallback where the room or the message asks for
   RCS only. Phase 10 builds the bridge-wide switch (`smsFallback`); RCS only per room and per message is later
   work: a stored room field and a composer option. A file goes as the provider's media (Twilio `MediaUrl`, Sinch `media_message`, Telnyx `file_url`).
6. Map inbound text, files, location and suggestion replies to `ChatBridgeServices.ingest#Event`.
7. Map each provider's reports to delivery and read status. A report never moves a delivery backwards (Twilio's come in
   any order).
8. Record the path (RCS, SMS or MMS) of each sent message, from the provider's answer or report. Show the real path in
   the UI.
9. Set `Capabilities`: `edit=false`, `delete=false`, `reaction=false`, `idempotent=false`, `encryption=Transport`, no
   group texts (RBM has no groups).
10. The provider's fallback is the capability check. Where the provider has a capability lookup (Telnyx's RCS
    Capabilities endpoint), use it only to show the path before a send.
11. A reply to a fallback text reaches the fallback number: the RCS room when that number is the RCS bridge's own,
    else the number's SMS room (decided 2026-11-17; a same-provider SMS bridge's number only, phase 10).
12. Add tests with each provider's fake server in `crates/moqui-channels/src/tests.rs`, like the SMS tests.
13. Write an operator guide in `docs/interfaces/`: per provider, the brand, the agent or sender, test devices, launch
    and fees, each marked not yet tested with the provider until it is.

#### RCS phase 2: Rich features and labels

Designed 2026-11-17: phase 13 (§18.3) has the data model, the providers' forms and the tests.

1. Add suggested replies and actions to the chat model. Show them as chips in the mobile UI, then desktop and web
   (`content.rich`, `reply#Suggestion`).
2. Add rich cards and carousels as a message kind, sent in each provider's form (the table above): Twilio a Content
   resource made for the send (or an app's own `ContentSid`), Sinch its card, carousel and choice messages. Every other
   bridge and client gets the fallback text with links. Authors: apps and services (`send#Rich`), and the composer's
   Card for a card with buttons.
3. Add typing and read events where the provider passes RBM's `IS_TYPING` and `READ` (Twilio's `read` status and
   Sinch's `READ` report give read). Typing goes out only: both providers document the indicator out, neither one in.
4. Show the RCS labels below in thread headers and message info, in place of the SMS banner. The `encryption`
   capability they come from is built with the other labels (§6.2, §18 phase 3), not here.

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
4. Check each provider's docs for RBM typing events and the countries it onboards (Telnyx: the US by API).

### 16.5 UI labels for E2EE status

The labels for MR Moqui chat, SMS and other bridges are in §6.2, the one table for them. These rows add the RCS
paths, in the same form. The business rows are phase 13 (§18.3): the `rcs` state of `moqui.chat.e2eeLabel`, its
banner naming the provider, and Info's "Encryption" line by each message's path.

| Path | Label (thread header) | Detail text (message info) | Icon |
| --- | --- | --- | --- |
| RCS business (the provider's RBM) | "RCS Business - not end-to-end encrypted" | "Encrypted in transit only. The provider, Google, carriers and this server can read it." (The provider by name: Twilio, Sinch or Telnyx.) | Open lock + verified badge |
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
| 1 | Unified contacts: one person with many networks; pick the network per message: designed, phase 22 (§18.3) | §11, §11.1 |
| 2 | Block and report; spam filter for outside numbers | §14.2; designed as phase 11 (§18.3) |
| 3 | Scheduled send, message reminders | §14.2, §14.3; designed as phase 15 (§18.3) |
| 4 | Disappearing messages (exact timer) and view-once media: designed, phase 16 (§18.3) | §14.4 |
| 5 | Invite links and QR, with approval; join requests: designed, phase 17 (§18.3) | §14.4, §14.5 |
| 6 | SMS members of a room (GroupMe's SMS join): designed, phase 18 (§18.3) | §14.5 |
| 7 | Stickers, GIF search, location and live location: sticker packs and location built, phase 20 (§18.3); GIF search and live location later | §14.3 |
| 8 | Event messages with RSVP: built, phase 19 (§18.3) | §14.5 |
| 9 | Screen security option; one-time code helper; send original photo: built, phase 21 (§18.3) | §14.2, §14.4 |
| 10 | GroupMe adapter (`groupme`): designed, phase 14 (§18.3) | §13.3 |
| 11 | Signal adapter on `signal-cli` (`signal.rs`), optional build feature | §13.3 |
| 12 | iMessage adapter on BlueBubbles (`bluebubbles.rs`), optional build feature | §13.3 |
| 13 | Business RCS in the SMS provider modules, Twilio and Sinch first: phase 10; its rich features and labels: designed, phase 13 (§18.3) | §16 |
| 14 | Matrix appservice API (C6c), then `mautrix-gmessages` for personal RCS | §13.2 |
| 15 | History imports | §15 |
| 16 | Default phone app (`sim-dialer`): SIM calls, line choice, one call log, call screening | §14.7 |
| 17 | Contacts: Contacts screen, Android sync adapter on CardDAV, phone import, caller names | §14.8 |
| 18 | Encrypted polls and events together: answers as encrypted messages, counted on the devices (needs its design) | §14.1, §14.5 |

## 18. Implementation phases

### 18.1 Order

| # | Phase | Sections | Size |
|---|---|---|---|
| 0 | Fixes from the UI review: done 2026-11-16 (`82affc535`) | §5.2 paging, §6.5 | Small |
| 1 | Shared helpers and the rest of the phone layout: done 2026-11-17 (`c88df4a3f`) | §2 | Medium |
| 2 | Notifications: done 2026-11-17 (`93476615a`; UnifiedPush with a real distributor and the soak tests on real phones left) | §7 | Large |
| 3 | E2EE labels and the call rule: done 2026-11-17 (`fdd1ab597`) | §6.1–§6.4 | Medium |
| 4 | Calls: flow and screen: done 2026-11-17 (web `20ce6c8d1`, Android `adc40de76`; the real-phone checks of §18.4 left) | §8.3 items 1–7 | Large |
| 5 | Calls: network tests, then resilience: done 2026-11-17 (`64c002d20`; the browser suite under sudo and real phones left) | §8.3 items 8–15, §8.4 | Large |
| 6 | Message menu, reactions, Info: done 2026-11-17 (`acc6d1b66`; a real phone's feel left) | §4 | Medium |
| 7 | Details tabs and services, voice notes: done 2026-11-17 (`533f3c0f2`; a voice note on a real phone left) | §5 | Medium |
| 8 | Desktop gaps: done 2026-11-17 (`4e451b601`) | §3 | Medium |
| 9 | The user's phone number: built 2026-11-17 (`adc40de76`; the port, 10DLC and the live tests are the user's steps) | §9 | Medium, plus the user's steps |
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
  most once per mode in the regression suite. The budgets stay (basic-comms: server 9 s, client 12 s, raised with the
  user's agreement on 2026-11-16 for the emoji verification check and on 2026-11-17 after phases 1 to 9).
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

- SMS is `None`, not `Transport` (§6.3: "Never"): the provider is reached over TLS, but the provider, the carriers and
  the phone network read every text.
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
    phone's Answer; Move to and from the app through the page's move. Still `android.telecom` (phase 9 kept it: §9.1
    says why). The native call screen stays the page's
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
  - Android second review fixes: the ongoing notification's Hang up and person are the call in use, not the held one;
    Telecom's first mute report skipped on Android 14+ too (the page's own mute cannot reach Telecom: no
    `Connection.setMuted`); every new document ends the page's calls (`PageLoadEvent::Started`), and a `joined` on its
    way from a page that went places nothing; the screen at the ear follows the app's front and back; no
    `CAPABILITY_HOLD` on the call in use while another is held; picture-in-picture stays while a joined call remains;
    `call_audio` on the main thread; the chosen route asked again after a Swap; a second call active for Telecom once
    the first is held; the call screen's mount token; a hold that never settles gives up; a Hang up before the join
    clears the kept answer. Another account's ring answered mid-call drops the call without telling its server
    (documented, Mobile-Client "During a call").

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

Done 2026-11-17 in `533f3c0f2`.

| Work | Files |
|---|---|
| `ChatEventLink`, written by `send#Message` (with a forward's links), replaced by `edit#Message`, removed by `clear#MessageContent` (redaction, retention); `extract#LinkUrls` with `listed` (any port, our own links, at most 50; the same text as a preview, so the hash finds it) | `ChatEntities.xml`, `ChatServices.xml`, `chat_link.rs` |
| `find#RoomLinks`, `find#RoomReactions`, `find#ChatActivity` (one cursor of date, eventSeq, kind and room), `get#MessageSummaries`; `find#RoomFiles`' lists of types (an encrypted room's files with their message's ciphertext) | `ChatServices.xml` |
| The transitions `links`, `reactions`, `activity`; `files` takes `contentTypePrefixes` and `excludeContentTypePrefixes`; `attachment` serves a sound or video inline | `Chat.xml` |
| The details view: tabs People, Media (grid; the viewer, full screen, with swipe, arrows and Left and Right), Files, Voice (player), Links, Reactions (All, To my messages), Pinned, each loading more as it scrolls and read again on a push; an encrypted room's files decrypted and sorted here, its links from the messages opened here | `Chat.qvue` |
| Voice notes: the microphone in place of Send with no text, the time, Cancel and Send (ten minutes at most), sent through the upload (encrypted in an encrypted room); played in the message and in Voice; a decrypted sound or video keeps its type (`blobType`) | `Chat.qvue`, `WebrootChat.js`, `WebrootVue.qvt.css` |
| The terminal: a room's Links (`chatView` links) and Activity (`chatView` activity), each with More | `ChatTui.xml`, `Chat.tui.json` |
| Bundled screens regenerated | `bundled-screens.js` |

- Tests. Regression: `BasicCommsDetails` (the links written, replaced and gone; each service's paging, equal dates, a
  stranger, an encrypted room; the terminal's lists), `chat_link` (the listed links), `WebrootChat` (`blobType`),
  `ShellSync` (the microphone in place of Send). Acceptance: `ChatDetails` (each tab's items for a room with photos, a
  video, a PDF, a voice note, two links and three reactions, the viewer's swipe, and a voice note recorded and sent,
  at 390×844 and 1280×800 in the bundled shell, offline).
- Left: a voice note recorded and played on a real phone (§18.4); the Activity view (phase 8 builds it on
  `find#ChatActivity`).
- Review fixes: the microphone is asked for once and dropped when the composer went or the recording was cancelled
  meanwhile (no recorder on a closed room); a voice note goes alone (typed text stays; Enter waits while it records);
  sounds and videos are served with their type (`content_type_for_name`: webm, ogg, opus, m4a, mp3, mp4, wav, webp);
  an encrypted edit keeps the message's files in its ciphertext (`moqui.chat.keptFiles`); the tabs' latest request wins
  and a push merges at the top; `find#RoomReactions` says there is more only when another message passes;
  `find#ChatActivity`'s cursor has the row's id and refuses a bad one; `find#RoomLinks` searches the room's links joined
  to their previews (`ChatEventLinkPreview`). Tests: `BasicCommsDetails` (the listed links end to end, the attachment's
  type, the cursors), `WebrootChat` (an edit's files), `moqui-lib` `resource` (the media types).

Planned:
- `ChatEventLink`; `find#RoomLinks`, `find#RoomReactions`, `find#ChatActivity`; the include and leave-out type lists
  of `find#RoomFiles`; the transitions `links`, `reactions` and `activity`.
- The tabbed details view; the viewer's swipe; the client-side sort for an encrypted room.
- Voice notes: record in the composer, send as an audio attachment, play in the Voice tab and in the message.
- Files: `ChatEntities.xml`, `ChatServices.xml`, `Chat.xml`, `Chat.qvue`.
- Terminal: Links and Activity as lists; no media grid.
- Done when: §5.

#### Phase 8: desktop gaps

Done 2026-11-17.

| Work | Files |
|---|---|
| The palette's Chat rooms: rooms by name, a phone room by its digits (its number or its bridge's, of all the user's rooms); a private room's icon is people, not a lock. People to message (`search#PalettePeople`): the people with no direct chat, a group that never leads; Chat's `dm` opens an unsaved message (`moqui.chat.directRoomFor`), the direct chat made by its first send | `ChatScreenServices.xml`, `BasicComms.xml`, `Chat.qvue`, `WebrootChat.js` |
| The current app's prefixed sources lead with no prefix (`palette.leadGroups`, the root's `paletteLead`); Enter typed with the text waits for their answer; each opening starts empty; Ctrl+Enter takes the root's `openNewTab` (the shell's app window); offline, one note and no error per search | `WebrootVue.qvt.js` |
| The palette in the desktop and mobile shell: Search, Mod+K through `moqui.keymap` (so Chat's keys work there too), the comms app's rooms first | `Root.qvue` |
| Activity and Threads (`find#ChatThreads`, the `myThreads` transition) beside the filters, more as they scroll, what is new merged at the top on a push that concerns them, an encrypted room's text decrypted in the page; the groups Pinned, Recent, Phone and Bridged, folding (per user); the Phone filter | `Chat.qvue`, `ChatScreenServices.xml`, `Chat.xml` |
| The mail line: `find#DirectChatMail` (a person's latest thread by their user account's address, a whole address of To or Cc, the last 180 days, in mail the user may read, Spam, Trash and drafts left out, one query), the `mailLines` transition read once a page (`moqui.chat.mailLines`), the line, a link Tab and Enter reach, with the `mail` label on the rail and the phone's home; the home's All and Phone | `ChatScreenServices.xml`, `Chat.xml`, `Chat.qvue`, `ChatPanel.qvue`, `WebrootChat.js` |
| The terminal's Threads (`chatView` threads: `find#ChatThreads`, its own empty text) | `ChatTui.xml` |
| Bundled screens regenerated | `bundled-screens.js` |

- Tests. Regression: `BasicCommsDesktop` (the palette source's rules: a room by name, a phone room by its digits, a
  person with no direct chat and not one with, a stranger finds no private room; the Threads feed: thread replies with
  the root for the starter and a writer, never the replier's own, a stranger none; the mail line: the latest thread by
  address in mail the user may read, a shared account's too, Trash, drafts, a look-alike address and an account not the
  user's left out, no line for a person with no direct chat, a stranger nothing), `WebrootVue.qvt.shell` (the lead
  groups). Acceptance: `ChatDesktop` (the app's shell, offline: the palette's two letters and Enter before a screen of
  the same letters, a person started from it, Activity's mention and reaction from another user, Threads, the groups and
  folding, the mail line at 1280 × 800 and on the phone's home, the Phone filter) and `ChatPalette` (the web, on the
  suite's server).
- Review (2026-11-17): the mail line by the person's account address only (an owned or shared box is not theirs), whole
  addresses, 180 days, read once a page; People to message as a group of its own and the unsaved `dm` message; Threads
  from `find#ChatThreads` (no `ChatUserActivity` kind added, so nothing new notifies); a push merges at the top; the mail
  line a link; phone rooms by their bridge's number; Ctrl+Enter and offline in the shell. Regression: `BasicCommsDesktop`
  (the alias left out, People to message apart, `find#ChatThreads`), `WebrootChat` (`directRoomFor`, `mailLines`).
- Left: a message with no reply is no thread; the mail line reads the latest 300 matching messages of 180 days, so a
  person whose mail is older has none; the shell's palette has no Recent or Saved finds and does not read the `KEYMAP`
  preference.

Planned:

- The palette source: people, phone rooms, rooms without the prefix in Comms. The palette in the desktop and mobile
  shell (shared with the [Shared/Gallery review](ui-review-shared-gallery.md), which owns the shell's keys).
- The Activity and Threads views on `find#ChatActivity`; folding; the Phone and Bridged groups.
- The mail line on a person's row (§3), from a new read-only service that finds each person's latest mail thread
  among the user's accounts.
- Files: `ChatScreenServices.xml` (`search#PaletteRooms`), `Root.qvue`, `Chat.qvue`.
- Done when: §3.

#### Phase 9: the user's phone number

- The user's steps: the port order and 10DLC in the SMS and SIP provider's portal; the live tests on the real number.
- A managed `PhoneAccount` with an outgoing path, on the current Telecom library; names from the phone's contacts and
  from `call-context`; group texts.
- Files: `CallManager.kt`, `CallConnectionService.kt`, the Android manifest, `Sms.js`, the SMS adapter (then
  `crates/moqui-channels/src/telnyx.rs`; now `sms.rs` and `providers/`).
- Done when: §9.2.
- Status (2026-11-17): built (§9.1's status); §9.2 waits for the user's port, 10DLC and the live tests (§18.4).
  - Group texts: `Sms.js` (`numberSet`, `groupKey`, a group room in `ensure#PairRoom`, `open#PairRoom` and `receive#Sms`'s
    `recipientList`), the adapter's group send and the webhook's recipients (Telnyx's `to` and `cc`; Twilio's group
    texts, added later, through its Conversations API); Text a number takes several numbers, and the new-chat pickers start a group text for
    several (`WebrootChat.js` `pickKind`, `startChat`). A group room on a bridge without group texts (Sinch) refuses to
    send and says why.
  - Providers: built, none tested live with the provider (§9.1, Providers). Telnyx was the only provider in phase 9;
    the SMS adapter then became a neutral core with a module per provider, and Twilio and Sinch were added on it.
  - Names: `find#PhoneNames` (the apps' `call-context` providers, as the user) and Chat's `phoneNames`;
    `moqui.chat.nameRooms` and `contactOptions` (the phone's contacts first), `m-drop-down`'s `options-extra`.
  - Android: `DialAccount.kt` (the managed account, the handing over, the check's state), `PhoneBook.kt` (the
    contacts' search and names, the permission's question), `src/dial.rs` (the one-shot dial link, the accounts that
    call), the shell's commands `call_dial_account`, `call_dial_choice`, `call_dial_from`, `contacts_search`,
    `contact_names`; the shell tells the app whether the account calls (`Root.qvue`), and its Dialer fills a handed
    number in (`Dialer.qvue`: the user presses Call; the account picked when several can; `*67` hides the number); the
    notification check's Calling accounts and Contacts rows.
  - Review fixes: nothing a link brings is called by itself; a group text counts once a recipient, sends no messaging
    profile to the group endpoint, keeps each recipient's receipt (failed only when all failed), keeps its sender,
    leaves our numbers out, gives a short code its own room and limits new group rooms a day; `find#PhoneNames` only for
    the user's phone rooms, 600 numbers an hour; the server asked for every number; the names again when contacts are
    allowed later; the pick of an account bound to the account and two minutes; the dial accounts pruned at start.
    A framework fix: a conf's `service-file` outside a component's `service` directory is found by name (a component's
    `test/MoquiConf.xml` names its test services), for the BasicCommsTexts `call-context` provider.
  - Tests. Regression: `moqui-channels` (a group text's room and its one group MMS; receipts per recipient; a STOP from
    one member; the sender kept, ours left out, a short code, the day's new groups), `BasicCommsTexts` (a group opened
    in any order and our number left out; names from the component's own test provider for the user's rooms only),
    `WebrootChat` (the pick of several numbers, the names asked of the server for every number and again when contacts
    are allowed, the contacts' options, each recipient's receipt in Info), `ShellSync` (the check's rows, a handed
    number filled in and placed only on Call, the account's pick, `*67`), moqui-mobile `dial` and `commands` (the link,
    the number, the accounts, the pick, the contacts' arguments), JVM `CallRulesTest` (the account's state, a contact's
    number, the question once);
    instrumented `DialAccountTest` (registered and taken away, a number handed over by a nonce with Telecom's call
    cancelled, a number refused, a contact's name and search). On the emulator: the system's CALL intent through the
    account reaches the app's Dialer path, cold, and a made-up dial link is refused.
  - Left: the user's steps (the port, 10DLC) and §18.4's checks; group MMS between real phones; the account's on or off
    is not read (Telecom asks `READ_PHONE_NUMBERS`, which the app does not ask), so the check says "on" only once a
    number came through it.

#### Phase 10: business RCS (R1, §16 RCS phase 1)

Designed 2026-11-17. Built against fake servers only, so **not yet tested with the provider** (Twilio and Sinch). Telnyx
RCS (optional), rich cards, suggestion chips, read and typing events, and the RCS header labels are out (R2, §16 RCS
phase 2).

**The model.** An RCS bridge is a chat bridge of the new type `CbtRcs` (`enumCode` `rcs`), adapter `rcs-twilio` or
`rcs-sinch`. Its sender is the brand's agent, and its rooms are keyed by a number of ours, as an SMS bridge's are:
- **Its number.** A number's `messagingBridgeId` may name an RCS bridge (`Voice.js` takes `CbtRcs` as well as
  `CbtSms`; System, Voice, Numbers lists both). In phase 10 an RCS bridge has exactly one number, its home number:
  `store#PhoneNumber` refuses a second (`rcsOneNumber`) and one that is not the home number (`notHomeNumber`), and
  `store#Bridge` refuses to make a bridge RCS with several numbers, the system's, or a home number not its number
  (review 2026-11-17: with several, the agent's one thread would send the replies and a STOP to the home number's
  pair, and another pair would stay sendable). Several numbers per RCS bridge are later work: per-person routing of
  the agent's thread and the opt-out across the bridge's numbers. That number is:
  - the room's `ours`;
  - the fallback sender of its texts;
  - the pair of its opt-out;
  - what its grants, limits and members come from.

  So `check#SmsSend`, `ensure#PairRoom`, `receive#Sms`, the members and the calls all work unchanged. A user texts
  "through RCS" by being granted that number. Its room's texts go RCS first, from the brand agent's name (§19.4 Q1).
- **The system's number stays on SMS** (decided 2026-11-17). The system's own texts (sign-in codes) go from
  `VoicePolicy.smsSystemNumber`: `Voice.js` refuses to give that number to an RCS bridge (`store#PhoneNumber`, reason
  `systemNumber`) and to make a number of an RCS bridge the system's (`update#VoicePolicy`).
- **The room.** `ours/theirs` on the RCS bridge, `CrtExternal`, with the topic "RCS and texts with … on …". The other
  person's RCS replies and their SMS replies to the fallback number come into this one room. On their phone these are
  two threads: the agent's and the number's.
- **The bridge's home number.** An RCS message that comes in names the agent, not a number. Its `ours` is the bridge's
  home number: Twilio's `fallbackFrom` and Sinch's `smsSender`, a number whose `messagingBridgeId` is that bridge.
- **The reply to a fallback text (§19.1).** A text the RCS bridge takes for one of our numbers:
  - when the number's bridge is this RCS bridge: its room here;
  - when the number's bridge is an enabled SMS bridge of the same provider (`rcs-twilio` with `sms-twilio`,
    `rcs-sinch` with `sms-sinch`) and the same account (the payload's `AccountSid`, or `project_id/app_id`, is both
    bridges'): that number's SMS room, through its own bridge (`receive#Sms` with that bridge's id). The adapter
    decides this in Rust (`SmsAdapter::handover`, `SmsProvider::account_of`), where the bridges' configurations are
    readable, and only inside the RCS bridge's verified webhook;
  - otherwise (another provider's, another account's, a disabled bridge, outside the webhook): left before anything
    is kept, logged (`otherProvider`, `otherAccount`, `bridgeDisabled`, `notInWebhook`).

**The data model.**
- `ChatEntities.xml`: Enumeration `CbtRcs` (RCS, `rcs`).
- `VoiceEntities.xml`, beside `receiptStatus` on `ChatBridgeDelivery`: `deliveryPath`, the way a sent message went:
  `rcs`, `sms` or `mms`. Empty until known, and always empty on an SMS bridge.
- No other entity changes, and no migration.

**The trait** (`crates/moqui-channels/src/sms.rs`). R2 adds to it without changing it:
- `SmsProvider::channel()`: `Channel::Sms` by default, or `Channel::Rcs`. Its effects:
  - The capabilities' `encryption` is `Transport` for RCS. It stays `None` for SMS.
  - For RCS a sent message's path is recorded.
- `Outgoing.fallback`: whether an RCS send may go as SMS or MMS. It comes from the bridge's `smsFallback`, true by
  default. R2 adds `Outgoing.rich`: cards, carousels and suggestions.
- `Path`: `Rcs`, `Sms`, `Mms`, or `Fallback`. `Fallback` means "SMS or MMS, as its files say", for a report that does
  not say which.
- `Receipt.path`, and `SmsEvent::Path { remote_event_id, path }` for a report that only gives the way (Sinch's
  `SWITCHING_CHANNEL`).
- `Inbound.location` (latitude, longitude, label, address) and `Inbound.postback` (the payload of a tapped suggestion,
  kept for R2).

R2 adds `ReceiptStatus::Read` and a typing event.

**Recording the path** (`note#DeliveryPath`):
- The core records it after an accepted send when the send cannot fall back (`rcs`), by `chatBridgeDeliveryId`.
- Each report records it by `remoteEventId`.
- `Fallback` becomes `sms` or `mms` by the message's files.
- A path never moves from `sms` or `mms` back to `rcs`, and the same path again changes nothing.
- A change pushes `push#DeliveryState`.

**The providers.**

| | `rcs-twilio` (`providers/twilio.rs`, `providers/twilio/rcs.rs`) | `rcs-sinch` (`providers/sinch.rs`, `providers/sinch/rcs.rs`) |
| --- | --- | --- |
| Config, besides the SMS keys | `messagingServiceSid` with the RCS sender in its pool (required); `fallbackFrom`, the home number (required); `smsFallback` | `smsSender`, the home number (required); `mmsSender` as for SMS; `smsFallback`. One Conversation API app for each RCS bridge, with its RCS and SMS (and MMS) channels |
| Send | Messages API: `MessagingServiceSid`, `To` E.164, `FallbackFrom` set to the room's number, `Body`, `MediaUrl`, `StatusCallback`. No fallback: `To` `rcs:+1…` | `messages:send`: identities `RCS` and `SMS` (or `MMS` for a file), `channel_priority_order` `["RCS","SMS"]` (or `"MMS"`), `SMS_SENDER` or `MMS_SENDER` set to the room's number. No fallback: `RCS` alone |
| Path | The status callback's `From`: `rcs:<SenderId>` is `rcs`, a number is `Fallback` | The report's `channel_identity.channel`: `RCS`, `SMS` or `MMS`. `SWITCHING_CHANNEL` is `Fallback` |
| In | From `rcs:+1…` (prefix taken off, E.164 checked), To `rcs:<SenderId>`, which is the home number; a number `To` as SMS. `Latitude`, `Longitude`, `Label`, `Address`; `ButtonPayload` as the postback | The `RCS` channel too: `text_message`, `media_message`, `location_message`, `choice_response_message` (its `postback_data`). `sender_id` is not read on RCS (it is not a number), so ours is `smsSender`. An `RCS` opt-out notice is a STOP |
| Group texts | none (`group_max` 0: a group room refuses) | none |

`sms-sinch` still leaves RCS messages, and `sms-twilio` sends no RCS fields.

**Services and screens.**
- `ChatBridgeServices.note#DeliveryPath` (new, force-new, internal) and `get#EventDeliveries` (each destination's
  `path`).
- `Sms.js` (small, beside D1's work in `receive`):
  - `receive` takes `metadata` (the postback and the location) into the item;
  - a location with no words becomes the text "Location: {label}" with an OpenStreetMap link built from the coordinates
    (§19.1, Location);
  - the RCS room's topic in `ensurePairRoom`.
- The hand-over of a reply to a fallback text is the adapter's (`sms.rs`), not `Sms.js`'s.
- `Voice.js` (store number: the system's number and one number, the home number), `Numbers.xml` (Texts through: SMS
  and RCS bridges), `store#Bridge` (the network and the adapter agree; an RCS bridge's one number).
- `WebrootChat.js`:
  - `messageInfo` and `messageInfoLines`: a line "Sent as" (RCS; SMS, the phone took no RCS; MMS);
  - `moqui.chat.pathName(destination)`, SMS or MMS when the message went as the fallback, for `Chat.qvue`
    `deliveryText`'s one string "Delivered to {network} as {path}".
- `ChatTui.xml`: Info's "Sent as" line.
- The thread header keeps the SMS banner in an RCS room until R2's RCS labels (decided 2026-11-17).
- Phone and desktop: the same caption and Info line. Nothing depends on the width.

**Failure tolerance.**
- A path that is not kept is logged, and the send stays accepted. A report whose path is not kept still answers 200;
  its receipt is the part that is retried.
- An RCS message with no home number configured is left (`unknownNumber`), logged with the bridge's name.
- A malformed `rcs:` address is a 400 for that one event.

**Security and abuse.**
- Every RCS send goes through `check#SmsSend`: the grants, the opt-out, the day's limits. RCS is billed as texts are.
- Signatures are checked as for the provider's SMS, on the core node.
- The reroute is only to a same-provider SMS bridge's number, never to another provider's.
- Coordinates are numbers within range. The link is built here, never taken from the provider. Labels, addresses and
  postbacks are cut to 200, 200 and 1024 characters.
- The agent id is never taken as a number.
- `configJson` stays encrypted.

**Left out.**
- Telnyx RCS.
- The capability lookup (step 10: only Telnyx has one).
- RCS-only per room or per message (decided 2026-11-17: later work, a stored room field and a composer option). R1
  has it only bridge-wide, as `smsFallback` false.
- Rich content, suggestion chips, read and typing (R2).
- The RCS header labels (R2, §16.5).
- A `rcsSenderId` check on inbound (the signature covers it).

**Tests** (regression):
- `crates/moqui-channels/src/tests/rcs.rs`, on the SMS bed with a fake Twilio and a fake Sinch:
  - each send's fields, with and without fallback, and a file;
  - the path from the answer and from the reports: RCS, the fallback's SMS and MMS, never back;
  - RCS, location and suggestion replies in, into the home number's room;
  - a fallback reply to the RCS room and to a same-provider, same-account SMS bridge's room (Twilio and Sinch); left
    for another provider's, another account's (the payload's or the bridge's), a disabled bridge's (no opt-out kept)
    and outside the webhook; the webhook mark put back after a failure;
  - one number per RCS bridge, its home number, never the system's; `store#Bridge`'s network and adapter agreeing;
    a short-code sender;
  - the RCS opt-out;
  - the capabilities and a group room refused;
  - the config checks;
  - `sms-sinch` still leaving RCS.
- `WebrootChat.spec.js`: `pathName` and Info's "Sent as" line.

**Docs:**
- New: `docs/interfaces/RCS.md`, the operator guide, per provider:
  - the brand, agent or sender, the test devices, the launch and its fees, the webhooks, the bridge's keys;
  - marked **not yet tested with the provider**, with the facts not confirmed in the providers' docs listed.
- Changed: `SMS.md` and `Chat-Bridges.md` (the adapters), `docs/tools/Comms.md` (Texts), the crate's notes and
  `providers/mod.rs`.

**Status (2026-11-17): built**, against fake servers only. Not yet tested with the provider.
- Built as designed, with three details:
  - The hand-over of a fallback reply runs only inside the RCS bridge's verified webhook. The adapter hands its
    webhook mark to the SMS bridge for that one text (`sms::with_mark`, put back after, also on an error), because
    `ingest#Event` takes a transport-verified item only for the bridge whose webhook runs.
  - A place's coordinates reach `Sms.js` as text: a JSON number is a decimal in the service scripts.
  - The system number stays on an SMS bridge, refused in `Voice.js` `storeNumber` and `updatePolicy` (reason
    `systemNumber`).
- Left:
  - the live tests with Twilio and Sinch, and the facts not confirmed (`docs/interfaces/RCS.md`, "Not confirmed");
  - RCS only per room and per message;
  - several numbers per RCS bridge (per-person routing and opt-out across them);
  - R2.

#### Phase 11: block and spam

Designed 2026-11-17 (§14.2, §17 #2); built 2026-11-18 (the status at the end of this entry). A user blocks an outside number (its calls and texts) or a person in
chat, and a spam filter keeps texts from outside numbers that look like spam in one place, "Spam & blocked", with no
notification.

**A blocked number's texts are kept out of sight, not dropped.** Why:
- A number of ours may reach several users (a ring group, caller-ID grants), and a block is one user's. The others still
  get the text, so it is kept, and only the blocking user's place in the room is out of sight.
- Spam needs the same thing: a text the filter took must come back on "Not spam". One mechanism serves both.
- STOP, START and HELP are applied as now (a carrier's rule), and the apps' filing and opt-outs see every text.
- An unblock brings back what came in meanwhile, so a block made by mistake loses nothing. A call from a blocked number
  is still refused, as a reject rule refuses one today; nothing is kept of a refused call.

**Out of sight.** A member's `filterEnumId` (`CmfSpam` or `CmfBlocked`) takes the room out of their sight:
- `find#Rooms` leaves it out, unless its filter is `spam`, which lists only those rooms. It answers `filteredCount`, so the
  rail can show the folder. This covers every client: the rail, the Comms home, the palette and the XMPP room list.
- It counts nothing (`get#UnreadCounts`), and its level is `none` in `levelMap`.
- `append#Event` leaves the member out of the socket push. The push worker (`chat_push.rs` `recipients`) skips them.
- A new member who is filtered from the start gets no `joined` push; the change is still in their feed.
- The member opens it from the folder: everything shows, with a banner in place of the composer. Spam: "Not spam" and
  "Report spam". Blocked: "Unblock". A spam room's links are shown as text, not links.

**Data model**

| Entity, field | What |
|---|---|
| `ChatRoomMember.filterEnumId`, `filterDate` | `ChatMemberFilter`: `CmfSpam`, `CmfBlocked`; empty: in sight (`ChatEntities.xml`) |
| `moqui.chat.ChatUserBlock` (`userId`, `blockedUserId` pk; `blockedDate`; index on `blockedUserId`) | A person a user blocked in chat; at most 500 a user |
| `ChatMemberUnread` alias `senderUserId` (filter only) | The unread count less the blocked senders' messages |
| `CallerList.listTypeEnumId` | `CallerListType`: `CltBlocked`; empty: a list of the user's own. One Blocked list a user, made on the first block, named by its type, never renamed or deleted, not offered to a rule (`VoiceEntities.xml`) |
| `CallHandlingSetting.spamFilter` | N: the user's phone rooms are never put in Spam; on when empty |
| `VoicePolicy`: `smsSpamFilter`, `smsSpamThreshold`, `smsSpamLookup`, `smsSpamLookupDays`, `smsSpamLookupMaxPerDay` | The filter (on when empty; N turns it off for everyone); its threshold (100); the provider's lookup (off unless Y); its cache (30 days) and its limit a day (100, counted in `SmsSendCount` as `lookup`) |
| `moqui.call.SmsSpamRule` (`smsSpamRuleId`, `ruleTypeEnumId`, `pattern`, `weight`, `description`, `disabled`, `hitCount`, `notSpamCount`, `spamCount`, `lastHitDate`) | The deployment's rules, seeded in `VoiceSetupData.xml`, kept in System, Voice, Spam. `SmsSpamRuleType`: `SsrFirstContact`, `SsrLink`, `SsrLinkHost`, `SsrKeyword`, `SsrNumber`, `SsrReported`, `SsrLineType`. The counts show the admin which rules misfire |
| `moqui.call.SmsSpamNumber` (`phoneNumber` pk; `lastScore`, `lastReasonsJson`, `lastJudgedDate`; `lookupJson`, `lookupDate`, `lookupBridgeId`) | What this deployment knows of an outside number: the last verdict's rules (the banner's reasons), the provider's answer `{lineType, carrierName, callerName}` |
| `moqui.call.SmsSpamVote` (`phoneNumber`, `userId` pk; `voteEnumId` `SsvSpam` or `SsvNotSpam`; `voteDate`) | One vote of a user for a number. A number is reported when it has more spam votes than not-spam votes, and trusted the other way |

**The filter.** A text is judged when all of these hold:
- the filter is on (`smsSpamFilter`);
- the room has no message from a local sender (once we replied, it is a conversation);
- the number is not trusted;
- the number is not known.

A number is known when:
- it is on a member's caller list (not the Blocked list);
- it is a user's mobile number (`CallHandlingSetting`);
- or an app's `call-context` provider names it, for a new conversation only, asked as the room's first member (as
  `file#ExternalConversation` is).

The score is the sum of the weights of the rules that match. The text is spam at the threshold or above. The seeded
rules:

| Type | Pattern | Weight |
|---|---|---|
| First contact | The first text of a new conversation | 40 |
| Link | Any `http(s)://` or bare-domain link | 30 |
| Link host | `bit.ly`, `tinyurl.com`, `t.co`, `goo.gl`, `is.gd`, `ow.ly`, `cutt.ly`, `rb.gy`, `tiny.cc`, `shorturl.at` | 40 each |
| Keyword | gift card; you have won; claim your prize; verify your account; account suspended; unusual activity; confirm your identity; final notice; unpaid toll; delivery attempt failed; package could not be delivered; loan approved | 40 each |
| Number | None seeded; an admin adds a number or a prefix (`+1900*`) | — |
| Reported | The number is reported here | 100 |
| Line type | `nonFixedVoip`, only with the lookup on | 40 |

So:
- a first text from an unknown number with a shortened link is spam (110);
- a first text with a link alone is not (70);
- a number reported by a user here is spam for everyone else's new texts from it.

Patterns are plain text. A keyword matches on word boundaries, ignoring case; a host matches exactly or with a leading
`*.`. There are no regular expressions, so a text from outside cannot make the match slow. Only the first 2,000
characters of a text are read.

**Not spam and Report spam** (`SmsSpam.js`, a current member of the external room):
- Not spam:
  - clears `CmfSpam` for every member of the room;
  - votes not-spam for the number, which makes it trusted when it outvotes the spam votes;
  - counts `notSpamCount` on the rules of the last verdict.
  The room is not judged again, and its next text is filed by the apps as usual.
- Report spam in a pair room:
  - puts the number on the user's Blocked list, so its calls are refused too;
  - votes spam, and counts `spamCount` on the rules;
  - puts the room in Spam for the other members whose filter is on (it is reported now).
- Report spam in a group text: the room is blocked for the user (a room-level `CmfBlocked`), and each of its numbers gets
  a spam vote, with no caller-list entries.
- No provider's report API is used; the votes stay on this server.

**Block a number** (`CallHandling.js` `blockNumber`, `unblockNumber`; the room menu's Block; Settings, Call handling):
- Block adds the number, or a prefix ending in `*`, to the user's Blocked list.
- The user's pair rooms with a number that matches get `CmfBlocked`. Each room is changed on its own, with one feed
  change (`settings`, `filtered`).
- Unblock removes it, and clears `CmfBlocked` on the pair rooms that no other entry still matches.
- Calls: `planUser` refuses a caller on the Blocked list before every rule (reason `declined`, no voicemail).
  `Calls.js` `ring` leaves out each user who blocked the caller (a number on their Blocked list, or a user they blocked
  in chat). So a member of a ring group who blocked a number is not rung, and the others are.
- A spam verdict does not touch calls. A call from a spam room's number rings; its call events in the room notify
  nobody who has the room filtered.

**Group texts.** A group text room is judged as a pair room, by its sender. A block of one number does not filter the
group, because the room belongs to its other numbers too; as on iOS, a blocked number's group texts still show. Block or
Report spam from a group room's menu filters that room for the user, until Unblock in it.

**Block a person in chat** (`ChatBlocks.js`, `ChatBlockServices.xml`; local users only):
- Refused. `get#RefusedUsers` adds the users who blocked the current user, answered as users that do not exist. Through
  it, nothing else changes in `create#Room` or `add#Member`:
  - they cannot start a direct room with me, the existing one included;
  - they cannot add me to a room (the invite);
  - the people search leaves me out for them;
  - a public room's mention of me from them counts nothing.
- Hidden from me:
  - `get#Timeline` marks their events `hidden: 'blocked'`, with no body, files, ciphertext, reactions or previews.
    Paging is unchanged.
  - `search#ChatEvents`, `find#RoomFiles`, `find#RoomLinks` and `find#RoomThreads` leave out their items after the query.
    The paging rule (§5.2) allows this: the cursor, not the count of rows, says whether there is more.
  - `find#Rooms` blanks a last message of theirs.
  - The client shows one quiet line for a run of hidden messages: "Hidden: a message from someone you blocked".
- Silent for me:
  - `append#Event` leaves me out of the socket push of their events (not membership events);
  - `chat_push.rs` skips me for a local sender I blocked;
  - `record#UserActivity` writes no activity row for me;
  - `send#Message` and `edit#Message` count no mention of me;
  - `signal#Typing` leaves me out;
  - the unread counts of `find#Rooms` and `get#UnreadCounts` subtract their messages, with one grouped query, only for a
    user with blocks;
  - their calls do not ring me (`ring`).
- My one-to-one direct rooms with them (the plain one and the encrypted one) get `CmfBlocked`. Opened from the folder,
  they show everything, with Unblock.
- They notice nothing: their messages are sent as usual, and nothing says they are blocked (SEC-COM-01).
- Unblock: in Settings, Chat (Blocked people), from the folder's banner, or from the person's menu.

**The provider's number lookup** (optional, off by default):
- `ChatBridgeAdapter::lookup_number(&self, bridge, number) -> Result<Option<NumberLookup>, String>`, defaulting to
  `Ok(None)`. `NumberLookup {line_type, carrier_name, caller_name}` is in `chat_bridge.rs`.
- The Rust service `ChatBridgeServices.lookup#BridgeNumber` (`chatBridgeId`, `phoneNumber`; out `lineType`,
  `carrierName`, `callerName`, or `reasonCode` `notSupported` or `failed`) calls it.
- `SmsProvider::lookup_request(config, settings, number) -> Option<ApiRequest>` and `lookup_of(body) ->
  Option<NumberLookup>` both default to `None`. `SmsAdapter` implements `lookup_number` with them, through its guarded
  client with a 3 s timeout.
- Twilio is built: Lookup v2, `GET {lookupsBaseUrl}/v2/PhoneNumbers/{number}?Fields=line_type_intelligence`, with Basic
  auth. `lookupsBaseUrl` defaults to `https://lookups.twilio.com`. The line type keeps Twilio's names (`mobile`,
  `landline`, `fixedVoip`, `nonFixedVoip`, `tollFree`, …). It is in `providers/twilio/lookup.rs`, with one delegating
  method in `twilio.rs`. Not yet tested with the provider.
- Telnyx is designed and not built: `GET /v2/number_lookup/{number}?type=carrier`, with `carrier.type`.
- `Sms.js` asks for a lookup only when all of these hold:
  - the lookup is on;
  - the text is a first contact from an unknown number in E.164 (never a short code);
  - the local score is within the Line type rule's weight of the threshold, so the answer can change the verdict;
  - no cached answer is younger than `smsSpamLookupDays`;
  - the day is under `smsSpamLookupMaxPerDay`.
- A failure or a timeout adds nothing and is not cached.

**`receive#Sms`** (`Sms.js`; small changes beside R1's):
1. The keyword (STOP, START, HELP) is applied first, as now.
2. `judge#SmsText` runs in its own transaction and answers `memberFilterMap` (userId to `CmfBlocked` or `CmfSpam`),
   `spam`, `score` and `reasons`:
   - each user of our number with the sender on their Blocked list gets `CmfBlocked`, in a pair room;
   - if the text is spam, each other user with the filter on gets `CmfSpam`.
3. `ensure#PairRoom` takes `memberFilterMap`. It sets the filter on new members and on current members with none, and
   never clears one.
4. `ingest#Event`, as now.
5. When the text is accepted and not a duplicate:
   - a spam verdict is noted (`note#SmsJudged`: the rules' hit counts and the number's last verdict);
   - `file#ExternalConversation` is skipped while the text is spam.

**Services**

| Service | What |
|---|---|
| `org.moqui.call.SmsSpamServices` (`SmsSpam.js`): `judge#SmsText`, `note#SmsJudged` (internal); `mark#NotSpam`, `report#Spam`, `get#SpamInfo` (the banner's reasons); `get#SpamFilter`, `update#SpamFilter`, `store#SmsSpamRule`, `delete#SmsSpamRule` (VoiceAdmin) | The filter |
| `org.moqui.chat.ChatBlockServices` (`ChatBlocks.js`): `block#ChatUser`, `unblock#ChatUser`, `find#BlockedUsers`; `block#Room`, `unblock#Room` (by the room's kind: a pair room's number, a one-to-one direct room's person, a group text's room); `set#MemberFilters` (internal: rooms and a filter for a user, one feed change); the script's `blockedBy` and `blockersOf` for `ChatServices` | Blocks in chat, and the room menu's Block |
| `CallHandlingServices`: `block#PhoneNumber`, `unblock#PhoneNumber`; `get#CallHandling` with `blockedList` and `spamFilter` | Numbers |
| `ChatBridgeServices.lookup#BridgeNumber` (Rust) | The lookup hook |

**Screens**

| Work | Files |
|---|---|
| The rail's chip "Spam & blocked" (its count; shown when `filteredCount` is above 0 or the chip is chosen): the filtered rooms from `rooms?filter=spam`, each labelled Spam or Blocked. A filtered room's banner in place of the composer (the reasons, Not spam, Report spam; or Unblock), and a spam room's links as text. The room menu: Block or Unblock (a number, a person or the conversation), Report spam in a phone room. Block in the member list's menu of each person, beside the moderator's items. The hidden-message line | `Chat.qvue` |
| Transitions `blockRoom`, `unblockRoom`, `notSpam`, `reportSpam`, `blockUser`, `unblockUser`, `spamInfo`; `rooms` with `filter=spam`; `filteredCount` in `chatData` | `Chat.xml`, `ChatScreenServices.xml` |
| Blocked numbers (add a number or a prefix, remove) and "Filter spam texts", above the Caller lists; the Blocked list left out of the caller lists and of a rule's choice | `Settings/CallHandling.qvue`, `CallHandling.xml`, `CallHandlingTui.xml` |
| Blocked people (name, since when, Unblock; Block someone, through the user search) | `Settings/Chat.xml` |
| System, Voice, Spam (VoiceAdmin): the switch, the threshold, the lookup with its days and its limit a day; the rules (type, pattern, weight, on, hits, Not spam, Reported), with add, edit and remove | `System/Voice/Spam.xml` (new), `System/Voice.xml` |
| The terminal: the hidden-message line. Filtered rooms are not listed (`find#Rooms`). There is no Spam view and no banner actions: those are the browser's | `ChatTui.xml`, `Chat.tui.json` |
| Bundled screens regenerated | `bundled-screens.js` |

**On a phone and on a desktop.** The same screens:
- At 390 px the chip row scrolls as it does now, and the banner takes the composer's place.
- Block is in the room's menu, which is the phone's top-right menu.
- The desktop shell's bundled Chat is the same. Offline, its rail keeps the last list it had online, with the filtered
  rooms already left out.
- Neither the Android app's pushes nor the desktop's notifications come from a filtered room or a blocked sender: the
  server sends neither the push nor the socket event.

**Bridges and encryption.**
- SMS is not end-to-end encrypted, and the server already reads texts, so judging them adds no reader. A lookup sends
  only the number, to the provider of our number.
- In an encrypted room, a blocked person's ciphertext is left out of my timeline like any other content. Room keys are
  shared as before: a block changes what I see, not who is in the room.
- A blocked person's messages still go out on the room's bridges.

**Mail and the apps.**
- Mail's own block (Inbox: a sender's address to Spam) stays separate. A chat block blocks no mail, and a mail block
  blocks no chat (§18.2, Mail stays working).
- mr-mantle and mr-boss:
  - `send#SmsMessage`, `check#SmsSend` and the campaigns' sends are unchanged. A user's block never stops the system's
    texts or a campaign's.
  - STOP is applied before the judge, so the `SmsOptOut` EECA (the PcpSms withdrawal) runs for every number.
  - Filing waits while a room is spam.
  - The TextFiling spec's numbers (a known party, and an unknown number with no link) are not spam.

**Failure tolerance.**
- If `judge#SmsText` fails, it is logged and the text goes in unfiltered: an error never hides or loses a text.
- If a `call-context` provider fails, the number counts as known, so the first contact adds nothing.
- If the lookup fails, it adds nothing.
- The rooms a block or an unblock changes are changed each on its own, and one that fails is logged.
- The rule counts and the number's verdict are written after the text is taken, in a transaction of their own, and a
  failure is logged.
- If `blockersOf` fails inside `append#Event` or `ring`, it is logged and everyone gets the push or the ring: the blocks
  are a preference, not a security boundary.

**Security and abuse.**
- Every user service acts on the user's own rows. The spam admin services need `VoiceAdmin`. Not spam and Report spam
  need a current member of an external room.
- A block tells the blocked person nothing: they are refused as "not found", as the other reach refusals are.
- A user has one vote a number. One user's report puts the number in the other users' Spam for new texts only: it is
  never dropped, and their Not spam outvotes it.
- The patterns are plain text, and a text is read up to 2,000 characters.
- The lookup costs money: it is off by default, and has a limit a day, a cache, and a gate (only when the answer can
  change the verdict).
- A spam text's files are fetched as any text's are, with the same bounds. Its links are not linked.
- A user keeps at most 500 entries on the Blocked list and 500 blocked people.

**The spam purge** (decided 2026-11-17). The property `chat@spam-purge-days`, 0 by default (keep). Above 0, the daily
`clean#ChatRetention` deletes the texts older than that in each phone room that every current member has in Spam
(`SmsSpamServices.purge#SpamRooms`, through `clean#RoomRetention` with its `retentionDays`), each room on its own:
one that fails is logged and the rest go on.

**XMPP.** The user's XMPP sessions get the same pushes as the web client, so a blocked person's new messages do not
reach them, and `XmppServices.get#Archive` leaves a blocked sender's messages out of the history (one condition). The
XMPP app's own blocklist (XEP-0191, `XmppBlockedJid`) stays its own.

**Left out (known gaps).**
- A provider's report API; Telnyx's lookup; weights that learn by themselves. The admin sees the counts and sets the
  weights.
- Hiding a blocked number's texts in a group text.
- A blocked person's reactions and poll votes still count in the totals; their read markers still show.
- Judging calls as spam (§14.7 `CallScreeningService`); the SIM's texts (§10).
- The phone's own contacts as known numbers: they stay on the phone, and Not spam fixes one at a time.

**Tests (regression).**
- `framework/test/CallHandling.spec.js`: the Blocked list is made on the first block; it cannot be renamed, deleted or
  named by a rule; a blocked number's call is refused before every rule; a prefix; unblock.
- `framework/test/SmsSpam.spec.js` (new):
  - each rule type and the threshold;
  - a known number, a trusted number, and a room we replied in are not judged;
  - a spam text: not in `find#Rooms`, under `filter=spam`, level `none`, no `joined` push for a new member;
  - STOP from a spam or a blocked number is still applied;
  - a blocked number's text with a teammate who sees it;
  - Not spam and Report spam, with their votes and counts;
  - a group text;
  - a user with the filter off;
  - the lookup off by default, then its gate, its kept answer and its limit a day;
  - the purge;
- `framework/test/ChatBlocks.spec.js` (new):
  - a direct room and an add refused as not found;
  - hidden in the timeline and the links;
  - the unread counts;
  - no activity row and no mention count;
  - the direct room in Spam & blocked;
  - unblock;
  - a stranger cannot read another user's blocks.
- `basic-comms/test/BasicCommsTexts.spec.js`: the transitions `roomBlock`, `blockRoom`, `reportSpam` and `unblockRoom`;
  `rooms?filter=spam` and `filteredCount`; `BasicComms`: System, Voice, Spam rendered with the other Voice screens (qvt and tui).
- moqui-core: `chat_push` (a filtered member and a member who blocked the sender get no push); `chat_bridge`
  (`lookup#BridgeNumber` for an adapter with no lookup answers `notSupported`).
- moqui-channels `tests/twilio.rs`: the lookup's request and its answer on the fake server, and a failure.
- No client spec (the client budget is at 10.3 of 12 s) and no acceptance spec.

**Docs.**
- [SMS](../docs/interfaces/SMS.md): Blocked numbers and spam, and the lookup.
- [SIP, Call handling](../docs/interfaces/SIP.md#call-handling): the Blocked list.
- [Comms](../docs/tools/Comms.md): Block a person; Spam & blocked.
- [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): `lookup_number`.
- The BasicComms README: System, Voice, Spam.

**Review decisions (2026-11-18).** These replace the text above where they differ.
1. *Report spam acts on the reporter.* A pair room's number goes on the reporter's Blocked list and their room is out of
   sight; teammates get it in Spam only in a room nobody here ever replied in. The reported rule weighs 50 for each
   distinct reporter (less one for each Not spam), so one report is not spam for others; an admin's report, or an admin's
   number rule, reaches the threshold alone. The reporter's Unblock withdraws the vote and brings back the teammates' rooms
   the report put in Spam (`SmsSpamVote.filteredJson`, `withdrawReport`). In a group text the room is hidden for the
   reporter alone, and the report votes only against the numbers they pick (none by default). The confirms say exactly
   this.
2. *Ring groups and chat calls.* A user who blocked the caller is left out before any leg: `runRing` dials no mobile and
   offers no leg and writes no `CallUserLog` for them (no missed call), and in order or round robin their turn is skipped.
   In `Calls.js` `start`, a room's call leaves a blocker with no leg and no callee log, and a direct call to one is
   `noCallee`, as to nobody. Spec: `RingGroup`.
3. *Leaks closed.* `get#Timeline`'s reply previews (`replyTo.hidden`), the hidden stub names nobody;
   `find#RoomThreads`' last reply (`lastHidden`); `find#ChatThreads` (myThreads: a blocked root left out, its last reply
   blanked); `find#ChatActivity`; `push#Update` (not to a blocker, nor to a member with the room filtered).
4. *Not spam sticks.* It marks the room never to be judged again (`ChatRoomBridge.notSpamDate`) and trusts the number for
   the voter.
5. *Votes.* One user's Not spam trusts the number for that user's rooms only. For everyone it takes an admin's vote, or
   `TRUST_VOTES` (3) distinct users' that outnumber the reports; the votes are on System, Voice, Spam (`store#SpamVote`:
   trust, report or clear). An admin's `SsrNumber` rule applies to a trusted or known number all the same.
6. *Reject lists.* `known()` leaves out a caller list a reject rule uses.
7. *Client counts.* `moqui.chat.levelOf` gives `none` for a filtered room (the settings push carries `filtered`), and
   `applyCounts` sets a room the server's counts no longer name at nothing.
8. *Privacy.* The Calls picker answers a blocker as `noCallee`. A person block refuses only what the blocked person
   starts: a direct room, and adding the blocker to a private room (`get#RefusedUsers` with `blocks`); an admin who is
   not blocked still adds them, and public and record rooms are never refused. The blocked person still sees the
   blocker in shared rooms and reads their messages there.
9. *Unblock.* The mentions and Activity made during a block do not come back.
10. *Failure tolerance.* `blockedBy` and `blockersOf` never fail (logged: no blockers, everyone pushed and rung);
    `setFilters` and `refilterPhoneRooms` change each room on its own.
11. *Performance.* The day's lookup count is reserved in a transaction of its own, not held through the provider's
    answer, and given back when the lookup fails. `ChatRoomMember` has an index on `filterEnumId`.
12. *Conventions.* `chat@spam-purge-days` is an attribute of the `chat` element in MoquiDefaultConf.xml; System, Voice, Spam renders in
    qvt only in `BasicComms.spec.js` (no tui template of its own); its counts are localized.

**Status (2026-11-18): built** as designed, with the purge and the XMPP filter above, and the review decisions.
- Files: `SmsSpam.js`, `SmsSpamServices.xml`, `ChatBlocks.js`, `ChatBlockServices.xml` (new); `CallHandling.js`,
  `CallHandlingServices.xml`, `Sms.js`, `SmsServices.xml`, `Calls.js` (`ring`), `ChatServices.xml` (one-line filters),
  `ChatBridgeServices.xml`, `XmppServices.xml`, `VoiceEntities.xml`, `ChatEntities.xml`, `VoiceSetupData.xml` (the seeded
  rules); `chat_bridge.rs` (`lookup_number`, `lookup#BridgeNumber`), `chat_push.rs`, moqui-channels `sms.rs` and
  `providers/twilio/lookup.rs` (Twilio Lookup v2, not yet tested with Twilio); `Chat.qvue`, `Chat.xml`, `ChatTui.xml`,
  `ChatScreenServices.xml` (`filteredCount`), `Settings/CallHandling.qvue`, `CallHandling.xml`, `CallHandlingTui.xml`,
  `Settings/Chat.xml`, `System/Voice/Spam.xml`; `bundled-screens.js`.
- Tests. Regression: `framework/test/SmsSpam.spec.js` and `ChatBlocks.spec.js` (new), `CallHandling.spec.js` (the
  Blocked list), `basic-comms` `BasicCommsTexts` (the transitions) and `BasicComms` (System, Voice, Spam in qvt and tui
  with the other Voice screens); moqui-core `chat_push` (`a_filtered_room_and_a_blocked_sender_push_nothing`),
  `chat_bridge` (`a_bridge_with_no_number_lookup_answers_not_supported`); moqui-channels `tests/twilio.rs`
  (`a_twilio_lookup_reads_the_line_type_and_a_failure_is_an_error`); after the review also `RingGroup` (a blocker left out
  of a ring group and of a chat call), `SmsSpam` (the report, the group report, trust, reject lists), `ChatBlocks` (a
  public room takes the blocker, the reply preview) and the webroot client's `WebrootChat` (the filtered level, the counts
  reset, the hidden-line fold). The rail's chip, the banner and the menus are not walked in a browser.

#### Phase 12: E2EE by default and first sign-in on a new device

Designed 2026-11-17 (§6.5, §6.1 rule 5). Chat's new direct rooms and private groups are end-to-end encrypted when
everyone in them has a crypto device; a new device is set up from the key backup or another device.

Status (2026-11-17): built (`97f8937d0` server, `ef3fa69e0` client, `11f1aac12` docs, `d8604a21d`, then the review's
fixes). The retests of COM-UI-012 and 014 found nothing open (the review's rows). The basic-comms client suite measured
10.8 s of its 12 s. Left: §18.4's row on a real phone; the SDK's forwarding of room keys to a verified device with no
backup is not in a spec; `moqui-desktop`'s own tests were not run here (the crate needs `./moq desktop`'s generated
`target/moqui-app`), only rustfmt on its changed files.

| Work | Files |
|---|---|
| `check#DevicesReady` (internal) and `get#EncryptionDefault` (one bit, 50 people an ask, 600 an hour counted atomically, the same query for a refused person), from `ChatDevice` (not revoked, identity keys, seen in `chat@device-active-days`, default 90; an automatic one once seen on a later day or verified); `madeAutomatically`; the 50 devices in use only; the daily revoke of unused automatic devices (`revoke#AutomaticDevice`, `chat@device-auto-expire-days`) | `ChatKeyEntities.xml`, `ChatKeyServices.xml`, `ChatKeys.js`, webroot `ChatKeys.xml`, `MoquiDefaultConf.xml` |
| The default in `create#ChatRoom` (`encrypted` not given: the encrypted direct room, then the plain one, then the rule; out `encrypted`; the people's rows locked, the oldest twin answered); the transitions `createDirect` (`encrypted` true for Start an encrypted chat, false for a page that cannot encrypt), `createRoom` (no `encrypted` unless given) and `encryptionDefault` | `ChatScreenServices.xml`, `Chat.xml` |
| `directRoomFor` encrypted first, the oldest of each kind; `startChat` without `encrypted`; `setupNeeded`, `setupWaiting`; `undecryptableText`'s set-up text; `makeDeviceSoon`; the `m-chat-device-setup` card (waiting, Try again, Use your recovery key). The set-up bar is Chat's own, not an `e2eeLabel` state | `WebrootChat.js` |
| `setupStatus`, `requestSecrets`, the secrets asked after a verification with one's own device (`requestMissingSecretsIfNeeded`, the receive-secret callback, the backup key checked against the current backup and restored with it: `restoreBackup`'s import shared), `ensureDevice({automatic})`. No `setup` in the crypto state: Chat and My devices ask `setupStatus` | `WebrootCrypto.js` |
| Chat: the device made when the rooms show (idle time); the unsaved chat's label from `encryptionDefault` and its first send through the room's send (kept in the composer when not sent); Start an encrypted chat in a plain direct room's header menu and start, one click at a time, and the plain chat's link at the encrypted chat's start; the set-up bar and the message's Set up; an own-device verification request as a dialog anywhere in Chat, naming the device and when it was added; New room's toggle; Start one click at a time | `Chat.qvue` |
| The chat panel's Start one click at a time (its `encrypted` goes through `startChat`) | `Panel/ChatPanel.qvue` |
| A declined call's text: a page that cannot encrypt asks for the plain direct room | `WebrootCall.js` |
| My devices: the set-up card at its top; COM-UI-012 and 014 whatever the retest finds | `Settings/Devices.qvue` |
| My devices in the apps: the route, the account menu's item, the bundle, a link to it shown in the shell | `crates/moqui-desktop/shell/screens/Root.qvue`, `shell/js/shell.js`, `shell/bundled-screens.txt`, `bundled-screens.js`, `src/client/windows.rs` |

- **Phone and desktop.** One card for both: full screen under 600 px, a 480 px dialog above. The bar and the
  message's Set up are in the room on both. On the Android app it is the shell's bundled Chat and My devices.
- **Terminal.** No change: it lists an encrypted room and says to open it in a browser, as today; it makes no direct
  rooms.
- **Bridges, XMPP, calls.** No change: an encrypted room is never bridged and XMPP serves none; `create#Room`'s own
  default stays plain, so XMPP's direct chats and a call to a person (`Calls.js`) keep the plain direct room. A call
  from an encrypted room is encrypted (rule 4).
- **Failure tolerance.**
  - The device at Chat's opening: a failure (no WebAssembly, IndexedDB blocked, a refused upload) is logged and Chat
    goes on; that browser has no device, and the rule then makes plain rooms for its user unless another device of
    theirs counts.
  - `encryptionDefault` that fails or is `rateLimited`: the unsaved chat shows no label until the room is made, and
    New room's toggle stays off unless the user turns it on; the server decides at the create.
  - Secrets not answered in 2 minutes: the card says to open Chat on that device or use the recovery key, with Try
    again. A backed-up key that does not decrypt is counted and skipped, as `restoreBackup` does.
- **Security and abuse.**
  - `get#EncryptionDefault` reveals one bit for a set, the bit the room made would show. A refused, unknown or disabled
    user is answered as one with no device, the limits bound probing, and it names no device.
  - The server only picks the default. The room's label shows what was made, and an encrypted room never becomes plain.
    A server that answered false could only make a plain room, which says so.
  - Secrets go only to a verified device of the same user (the SDK's rule). A gossiped backup key is used only when it
    opens the server's current backup. No recovery key or secret reaches the server.
  - Devices at Chat's opening: marked `madeAutomatically`. A private window makes a device each time it opens Chat: such
    a device counts for the default only once used on a later day or verified, the 50 a user may hold count only those
    in use, and the daily job revokes one never verified after 30 days unseen.
  - Two makes of one direct room at once: the people's rows are locked in one order, the oldest twin is answered, and
    the buttons take one click at a time.
- **Left out.** Turning a room's encryption on or off; search on the device (later, §19.1); dehydrated devices (keys
  received while no device is open); Matrix secret storage (4S); signing in a new device by QR from another; revoking
  stale devices the user made (only automatic ones go by the job); the Dialer's call to a person in the encrypted direct
  room.
- **Tests (regression).**
  - Server: `framework/test/ChatEncryptionDefault.spec.js` (new): `get#EncryptionDefault` true when all have a device,
    false for one without, a revoked or keyless device, one not seen in 90 days, a refused and an unknown user; the
    answer holds the bit only; 51 people refused; past 600 an hour `rateLimited`. `basic-comms/test/BasicCommsEncrypted`
    (a feature): `create#ChatRoom` with no `encrypted`, a direct room, a group direct and a private room encrypted when
    all have devices and plain otherwise; the encrypted direct room answered after a device of theirs went; a plain
    direct room that exists answered though all have devices, and `encrypted` true then making the encrypted one
    beside it; an explicit false kept; a public room never; `create#Room`'s own default plain.
  - Client, offline: `webroot/test/client/WebrootChat` (`directRoomFor` encrypted first, then plain; `startChat` sends
    no `encrypted`; `setupNeeded`; the set-up texts and label) and `basic-comms/test/client/ShellSync` (the bundled Chat
    with a stand-in server and crypto: a dm link with no room shows the label the default gives; a plain direct room
    offers Start an encrypted chat, and the encrypted room links to it; the search pane's notice before a query; the
    set-up bar, the card's Restore with the key and Verify of one's own device, the bar gone when done; My devices
    routed in the shell).
  - Client, with the server: `basic-comms/test/client/CryptoFlow`'s emoji feature goes on: the first device has a key
    backup, and after the comparison the second gets the backup key from it and restores with no recovery key
    (`setupStatus` needed before, not after). The client budget (12 s, 8.2–10.5 s measured) is measured after; an
    overrun goes to the user.
  - No acceptance spec: the screen retests of COM-UI-012 and 014 are by hand, recorded in the review.
- **Docs.** [Chat keys](../docs/interfaces/Chat-Keys.md) (the default, `get#EncryptionDefault`, the device at Chat's
  opening, set-up, the secrets, the API's `setupStatus`), [Comms](../docs/tools/Comms.md),
  [Desktop-Client](../docs/ui/Desktop-Client.md#chat-crypto) (My devices bundled; its Known gaps line),
  [Mobile-Client](../docs/ui/Mobile-Client.md) (first sign-in), the BasicComms README, the review's COM-UI-012 and 014
  rows.
- **Done when.** The regression specs above pass within budget; on a real phone, the row of §18.4.

#### Phase 13: RCS rich features and labels (R2, §16 RCS phase 2)

Designed 2026-11-17, on phase 10. Built against fake servers only, so **not yet tested with the provider** (Twilio and
Sinch), as phase 10. Four parts: suggestions as chips, rich cards and carousels as a message kind, the read receipt and
the typing indicator, and the RCS labels.

**The model.** No new entity and no migration. A rich message is a message (`CetMessage`) with `content.rich`, the
pattern of `content.poll` and `content.assist`:

```
rich: {kind: text | card | carousel,
       text,                                        // kind text: the words the chips go with
       cards: [{title, description, attachmentSeq,  // the message's image for the card (1..n)
                suggestions: [...]}],
       suggestions: [...],                          // kind text: the chips
       providerContent: {twilio: {contentSid, contentVariables}}}   // optional, apps only
suggestion: {type: reply, text, postback} | {type: url, text, url} | {type: dial, text, phone}
```

- Its `body` and `bodyText` are the **fallback**, made by the server from the rich content: the words, then each card's
  title and description, then each link as "label: URL" and each call as "label: number", and the replies as one line
  "Reply: Yes, No". Every client and bridge that knows nothing of `rich` shows and sends that: the terminal, search,
  snippets, notifications, mail digests, XMPP, Matrix, and an SMS room (its images go as MMS, as any file does).
- Limits, the strictest of the two providers so one rich message goes on both (Twilio's `twilio/card` and
  `twilio/carousel` pages, Sinch's RCS message support, read 2026-11-17):

  | | Kind `text` | Kind `card` | Kind `carousel` |
  | --- | --- | --- | --- |
  | Words | 1 to 1600 characters | title up to 200, description up to 1600, at least one | each card's title and description 160 together |
  | Image | none | 0 or 1 | one on every card (Twilio needs it) |
  | Suggestions | 1 to 10 chips | 0 to 4, on the card | 1 or 2 a card, the same types in the same order on every card (Twilio) |
  | Cards | — | 1 | 2 to 10 |

  A suggestion's text is 1 to 20 characters (Twilio's quick reply; 25 for a link or call there, 20 kept for all); a
  reply's `postback` up to 200 printable characters, its text when not given; a link `http` or `https`, up to 2048; a
  call an E.164 number. The message's files are its cards' images (PNG, JPEG, GIF, WebP), each card's own, and no
  others. The fallback is at most 1600 characters (an SMS bridge's `maxTextLength`), else the rich message is refused.
- `content.postback {data, toEventSeq}` on a message that answers a suggestion: one a member tapped here
  (`toEventSeq`, checked), or one that came in from RCS (`Inbound.postback`, `toEventSeq` empty).
- `DeliveryRequest.rich` (`crates/moqui-core/src/service/chat_bridge.rs`): the event's `content.rich`, as JSON, for an
  adapter that sends it; the others never read it.
- `ReceiptStatus::Read`; `note#DeliveryReceipt` ranks `read` above `delivered` (a read is never taken back to
  delivered or failed).
- `ChatBridgeDelivery.providerContentId` (`VoiceEntities.xml`, beside `deliveryPath`): the provider's content made
  for this send (a Twilio `HX…`), until it is deleted.

**Services** (new `framework/service/org/moqui/chat/ChatRichServices.xml`, its logic in `ChatRich.js` beside it, as
`Sms.js` is; `ChatServices.xml` gets only one-line changes, beside D1's):
- `send#Rich` (chatRoomId, rich, chatAttachmentIdList, threadRootEventSeq, clientRef): the member's or an app's rich
  message: `normalize#Rich`, then `send#Message` with the fallback as `body`, then `content.rich` set and pushed
  (`push#Update`, fields `{rich}`), in one transaction, as `create#Poll` does. Refused in an encrypted room
  (`encryptedRoom`: the server must read it to make the fallback). `providerContent` only with authorization disabled
  (an app's own call), as `send#AssistCard`'s rule.
- `normalize#Rich` (internal): the checks above, each refusal with its message through `t()`; out: the clean `rich`
  (unknown keys dropped), `body` (safe HTML) and `fallbackText`.
- `reply#Suggestion` (chatRoomId, eventSeq, index: which suggestion, cardIndex): a member taps a reply chip: a message with
  the suggestion's text (`send#Message`), then `note#SuggestionReply`. Refused when it is the member's own message, when
  the room has an outbound bridge (its chips are the other person's, on their phone), or for a suggestion that is not a
  reply. A link or call chip is not a service: the client opens it.
- `note#SuggestionReply` (internal; chatRoomId, eventSeq, postback, toEventSeq, chatBridgeId): sets `content.postback`
  on the answering message and pushes it. It is the hook for apps: an app that sent a card puts its own context in the
  postback (`order:123:confirm`) and runs on a SECA of this service. `Sms.js` `receive` calls it after `ingest#Event`
  for an RCS reply with a postback (accepted, not a duplicate).
- `ChatServices.xml`: `append#Event` and `get#Timeline` give `rich` and `postback` (one line each); `edit#Message`
  refuses a rich message ("A card is not edited: delete it and send another"), beside the poll's line; `signal#Typing`
  starts `ChatBridgeServices.send#BridgeTyping` asynchronously when `typing` is true (one line).
- `ChatBridgeServices.send#BridgeTyping` (new, Rust in `chat_bridge.rs`, internal): see Typing below.
- `get#RoomBridges` and `get#EventDeliveries` give each bridge's `adapterName`, for the provider's name in the labels.
- Who authors one: an app or a service (marketing, mr-boss, an agent) through `send#Rich`; and a member through the
  composer's **Card** (below). A carousel and `providerContent` come from apps only.

**The providers** (`crates/moqui-channels`):

| | `rcs-twilio` (`providers/twilio/rcs.rs`) | `rcs-sinch` (`providers/sinch/rcs.rs`) |
| --- | --- | --- |
| Rich send | Two calls: `POST {contentBaseUrl}/v1/Content` (JSON: `friendly_name` `moqui-{chatBridgeDeliveryId}`, `language` the bridge's `contentLanguage`, `en` by default, `types`), then the Messages API with its `ContentSid` instead of `Body` and `MediaUrl`, and `MessagingServiceSid`, `To`, `FallbackFrom`, `StatusCallback` as for a text. A `providerContent.twilio.contentSid` skips the first call (`ContentVariables` with it) | One `messages:send` as for a text, its `message` the rich kind, and `explicit_channel_omni_message` with our fallback for the fallback's channel |
| Kind `text` | `twilio/card` with `body` and the chips as `actions`, each `chip_list: true` | `choice_message`: `text_message` and `choices` |
| Kind `card` | `twilio/card`: `title`, `body`, `media` (the image's URL), `actions` | `card_message`: `title`, `description`, `media_message.url`, `choices` |
| Kind `carousel` | `twilio/carousel`: `body` (the fallback; RCS drops it), `cards` of `title`, `body`, `media`, `actions` | `carousel_message.cards`, each a card as above |
| A suggestion | reply `QUICK_REPLY` (`title`, `id` the postback); link `URL` (`title`, `url`); call `PHONE_NUMBER` (`title`, `phone`) | reply `text_message` with `postback_data`; link `url_message` (`title`, `url`); call `call_message` (`title`, `phone_number`) |
| The fallback | The same content's `twilio/text` (`body` the fallback) or, with an image, `twilio/media` (the fallback and the first image) | `{"SMS": {"text_message": …}}`, or with an image `{"MMS": {"card_message": …}}` (MMS takes a card: the title its subject) |
| No fallback (`smsFallback` false) | No `twilio/text` or `twilio/media` type; `To` `rcs:+1…` | `RCS` alone, no override |
| Read | Status `read` is `ReceiptStatus::Read` (it was taken as delivered) | `MESSAGE_DELIVERY` `READ` is `Read` |
| Typing out | `POST {messagingBaseUrl}/v3/Indicators/Typing.json` JSON `{channel: RCS, from: rcs:{rcsSenderId}, to: rcs:+1…, event: START}`; shown up to 20 s or until a message arrives. Needs the new key `rcsSenderId` (the agent's id); none without it | `POST …/events:send` with `event.composing_event` to the `RCS` identity, `channel_priority_order ["RCS"]` |
| New config | `contentBaseUrl` (`https://content.twilio.com`, a test's fake), `messagingBaseUrl` (`https://messaging.twilio.com`), `contentLanguage`, `rcsSenderId`, `typingIndicators` (true) | `typingIndicators` (true) |

- `sms.rs`: `Outgoing.rich: Option<&Rich>`, the neutral parse of `DeliveryRequest.rich` in a new `sms/rich.rs`
  (`Rich`, `RichKind`, `Card`, `Suggestion`; a card's image as an index into `Outgoing.media_urls`). The core sends
  a rich message's images only through the provider's rich form, never as `MediaUrl` beside it. `SmsProvider` gains
  `typing_request(config, settings, ours, theirs) -> Option<ApiRequest>` (none by default). An SMS provider and
  `sms-twilio`, `sms-sinch`, `sms` (Telnyx) ignore `rich`: they send the fallback and the images, as now.
- Twilio's content per message (decided 2026-11-17): one `Content` resource for each rich send (an image's URL is
  signed and expires, so two sends never make the same content), deleted once its message is done:
  - its `HX…` id is kept on the delivery (`ChatBridgeDelivery.providerContentId`, by `note#DeliveryContent`, in its own
    transaction) as soon as Twilio makes it, before the send;
  - a final receipt (delivered, read, failed or undelivered) deletes it (`DELETE {contentBaseUrl}/v1/Content/{sid}`)
    and clears the field: `note#DeliveryReceipt` answers the delivery's `providerContentId` with a final status, and the
    adapter deletes it after the receipt is kept;
  - a daily sweep (`ChatBridgeServices.clean#ProviderContent`, run by the delivery worker once a day beside its other
    cleaning) deletes those left on deliveries older than 2 days (a receipt that never came, a crash before the send),
    through `ChatBridgeAdapter::delete_content`;
  - best effort: a failed DELETE is logged and the field kept for the sweep; one Twilio no longer has (404) is cleared;
  - an app's own `providerContent.twilio.contentSid` is never kept there, so never deleted.

  An app that sends one card to many people names a `ContentSid` it made once in Twilio (`providerContent`).
- Answers of the first call (the content): 2xx with a `sid` goes on to the send; 2xx without one, a lost answer, 429
  and 5xx `Retry` (no message was sent: a second content is harmless); 401 and 403 `Retry` not connected; another 4xx
  `Failed` `richRefused` with Twilio's text. The send's answer is read as a text's.

**Typing** (§16.4 step 3). Twilio and Sinch document the typing indicator **out** (ours to the person), and **neither
documents one in** (the person's `IS_TYPING`): Twilio's status callbacks and inbound webhooks name none, and Sinch's
RCS page lists only `MESSAGE_INBOUND`, `MESSAGE_DELIVERY`, `OPT_IN` and `OPT_OUT`. So only out is built:
- `ChatBridgeAdapter::typing(bridge, external_room_id) -> Result<(), String>` (default `Ok`, nothing sent);
  `SmsAdapter` sends the provider's `typing_request` for a pair room (never a group), when `typingIndicators` is not
  false, and not to a person who opted out of our number (`SmsOptOut`: a STOP ends every contact).
- `send#BridgeTyping` (chatRoomId): at most once in 15 s for a room on this node (in memory), before any read; then
  each active outbound mapping of an enabled bridge, its adapter's `typing`. A failure is logged and the next mapping
  goes on; nothing is kept or retried. It runs asynchronously, so the user's typing signal never waits on a provider.
- Read out (our read of their message, Sinch's `read_message_event`) is left out (decided 2026-11-17): it tells the
  person when a user of ours read their message, later work with its own setting.

**Labels** (§16.5, §6.2). `moqui.chat.e2eeLabel` gets the state `rcs`, before `sms`, for a room with an RCS bridge:

| Where | Text |
| --- | --- |
| Header (`text`), with the open lock and a `sym_o_verified` badge (`label.badge`) | "RCS Business - not end-to-end encrypted" |
| Banner | "Encrypted in transit only. {provider}, Google, carriers and this server can read it." (`moqui.chat.providerName(adapterName)`: Twilio, Sinch, Telnyx; else "The provider") |
| Composer hint | "RCS message" |
| Info, a line "Encryption" for an RCS destination, by its path | `rcs`: the banner's text; `sms`: "The phone did not support RCS. This message went as SMS."; `mms`: the same, "as MMS"; no path yet: the banner's text |
| Terminal (`ChatTui.xml`) | The banner's one line; Info's "Encryption" line |

The SMS banner leaves RCS rooms (phase 10 kept it). Never a closed lock (§16.5 rules). The per-message path stays as
phase 10 built it ("Delivered to RCS as SMS").

**Read** (§16.4 step 3). The caption of a message whose receipt is `read`: "Read on {network}", the done-all icon in
the primary colour; Info's line "Read: the network's receipt says so". A local member's read markers are unchanged.

**Screens** (`Chat.qvue`; a self-contained `chatRich` sub-component and the `cardBox` dialog, each its own block, with
one tag in the message template, one button in the composer and one entry in `components`, so D3's work there merges):
- **A text with chips:** the words as any message, then the chips: Quasar `q-chip`s, a reply with the reply icon, a
  link with `sym_o_open_in_new`, a call with `sym_o_call`. On a phone (`room.mobile`) one row that scrolls sideways
  under the bubble; on a desktop they wrap. A tap: a reply sends its text (`reply#Suggestion`), a link opens in a new
  tab (`noopener`), a call goes to the app's dialer (`tel:` where there is none). Display only (no tap, the tooltip
  "For the other person to tap") on my own message and in a room with an outbound bridge.
- **A card:** a `q-card` (at most 320 px; 85 % of the width on a phone): the image (its attachment, the chat's file
  URL), the title, the description, the suggestions as flat buttons. Its image is not shown again as a file.
- **A carousel:** the cards side by side in a row that scrolls with snap (240 px each), on every width.
- **An answer to a suggestion:** a small "Chose a suggestion" caption with the reply icon under its text.
- **The composer's Card** (a button beside Poll, plain rooms only, offline disabled): a dialog with Title, Text, an
  image (pick one file, an image), and up to 4 buttons, each Reply (its words), Link (words and URL) or Call (words and
  number). With no title and no image it is a text with chips. It shows the fallback ("People without RCS get:") as
  the user types, from the same rules (`moqui.chat.richFallback`, kept in step with `ChatRich.js` by the specs). Its
  Post button calls `send#Rich` through the outbox as a poll does.
- The message menu: no Edit on a rich message (as on a poll); Copy copies the fallback.
- The terminal shows the fallback (its `body`); chips cannot be tapped there.
- Phone and desktop differ only in the chip row (scroll or wrap) and the card's width.

**Bridges and encryption.**
- Every non-RCS bridge sends the fallback and the images, unchanged: the core reads `rich` only in the RCS providers.
- A rich message is refused in an encrypted room (`send#Rich`, and the Card button is hidden there).
- RCS is encrypted in transit only; its labels never show a lock.

**Failure tolerance.**
- A rich send that fails at Twilio's content step is the delivery's failure or retry, as any send; the other bridges of
  the room go on.
- A typing indicator that fails is logged and dropped; the next mapping and the next signal go on.
- `note#SuggestionReply` after an RCS reply runs after the message is kept; its failure is logged (`Sms.js`), and the
  text stays.
- A `content.rich` that does not parse is ignored by the client and the adapter: the fallback shows and goes.

**Security and abuse.**
- Every rich RCS send goes through `check#SmsSend` as a text: the grants, the opt-out, the day's limits.
- Links in a suggestion are `http` or `https` only, opened with `noopener noreferrer`; a call is a checked E.164
  number. The card's words are text, never HTML; the fallback HTML is made from escaped text.
- The images are the message's own uploads, served to the provider by the signed, expiring media URLs.
- `providerContent` (a Twilio template of the account) only from an app's own call, never from a client.
- An inbound postback is untrusted text, cut to 1024 (phase 10) and kept only on its message; nothing is run from it
  but the SECAs an app put on `note#SuggestionReply`. `reply#Suggestion` checks the suggestion is on that message.
- No typing indicator to a person who opted out; at most one a room each 15 s.

**Left out.**
- The person's typing in (no provider documents it; §16.4 RCS phase 4 watches for it).
- Our read receipt out (above).
- Telnyx RCS rich content (Telnyx RCS is optional and not built).
- Rich messages in a direct send (`send#BridgeDirect`: a campaign's texts) and from the terminal.
- A carousel or `providerContent` from the composer.
- Calendar, location and share-location suggestions (Sinch has them; Twilio's cards do not).
- Linking an RCS reply to the card it answers (`toEventSeq` of an inbound postback): an app puts its context in the
  postback.

**Tests** (regression):
- `crates/moqui-channels/src/tests/rcs.rs`, with the fake Twilio (its Content API, Messages API and Indicators) and the
  fake Sinch:
  - Twilio: a card, a text with chips and a carousel: the content's body and the send with its `ContentSid`; the
    fallback type (`twilio/text`, `twilio/media` with an image) and none without the fallback; a `providerContent`
    with no content call; the content refused (`richRefused`) and unavailable (`Retry`, no message sent);
  - Sinch: the `choice_message`, `card_message` and `carousel_message` bodies and the SMS and MMS overrides;
  - `sms-twilio` with a rich message sends the fallback and the image;
  - `read` (Twilio) and `READ` (Sinch) to `read`, and never back to delivered;
  - typing: Twilio's and Sinch's requests; none with `typingIndicators` false, without `rcsSenderId` (Twilio), to a
    group or to an opted-out pair; once in 15 s.
- `crates/moqui-core/src/service/chat_bridge_tests.rs`: `DeliveryRequest.rich` from the event; `send#BridgeTyping`'s
  throttle and its skip of a failing adapter.
- `runtime/components/base/basic-comms/test/BasicCommsRich.spec.js` (new, server): each limit's refusal; the fallback
  `body` and `bodyText`; an encrypted room refused; `providerContent` refused from a user; edit refused;
  `reply#Suggestion` (its message and `content.postback`, refused on my own message, in a bridged room, for a link);
  `note#SuggestionReply`; `get#Timeline` with `rich`; `note#DeliveryReceipt` with `read`.
- `runtime/components/base/webroot/test/client/WebrootChat.spec.js`: `e2eeLabel`'s `rcs` state, `providerName`,
  `richFallback` (the same cases as the server's), Info's "Encryption" and read lines. No new client walk: the
  basic-comms client budget is at about 10.8 of 12 s.

**Docs:** `docs/interfaces/RCS.md` (rich messages, the read receipt, typing, the labels, and the new "Not confirmed"
items below); `docs/interfaces/Chat-Bridges.md` (`DeliveryRequest.rich`, `typing`, the `read` receipt);
`docs/tools/Comms.md` (cards and chips, the Card dialog); `docs/ui/Mobile-Client.md` (the chip row); the BasicComms
README; the crate's notes (`sms.rs`, `providers/twilio.rs`, `providers/sinch.rs`).

**Not confirmed in the providers' docs** (read 2026-11-17; listed in RCS.md, checked in the live tests):
- Whether a Twilio content for RCS needs approval before it is sent (its pages name approval for WhatsApp only).
- That Twilio sends the content's `twilio/text` or `twilio/media` as the SMS or MMS fallback (its page says "most
  complex type the channel supports"), and that `FallbackFrom` holds with a `ContentSid`.
- That Twilio returns a quick reply's `id` as `ButtonPayload` on RCS.
- The shape of Sinch's `explicit_channel_omni_message` value (its reference names a map of channel to content, with no
  schema), and that MMS shows a `card_message` as described.
- Whether either provider bills a typing indicator.

**Status (2026-11-17): built**, against fake servers only. Not yet tested with the provider.
- Built as designed, with these details:
  - The fallback HTML is one paragraph with a blank line (`<br><br>`) between blocks, so `send#Message`'s `bodyText`
    (what every bridge sends) is the fallback text.
  - The trait's provider hooks are `SmsProvider::send_typing` and `delete_content` (each sends its own request: Sinch
    needs its access token), with `sms::plain` for a one-request call; the adapter's are `ChatBridgeAdapter::typing`
    and `delete_content`, in the new `crates/moqui-core/src/service/chat_bridge_provider.rs` with `send#BridgeTyping`
    and `clean#ProviderContent`.
  - A rich message's pictures are counted and sized by the core's RCS limits (10 pictures, the provider's per-picture
    limit, 64 MB together), not the MMS ones; a plain text on `rcs-sinch` still carries one file.
  - A link or call button opens on any message; only a reply is the other side's to tap.
- Review fixes (2026-11-18):
  - Sinch's `explicit_channel_omni_message` is the AppMessage's (`message`), not the request's.
  - Postbacks are untrusted: a member's is `user:` and its words, only an app's own call names one (never `user:`);
    `note#SuggestionReply` gives `authorUserId`, `fromApp` and `outside`; an RCS reply's is the device's, unsigned.
  - Twilio: a retry sends the Content it kept (no picture) or deletes it and makes another; a Content is named
    `moqui-{delivery}-{attempt}`, and a retry with none kept deletes the earlier attempts' found by name; the sweep is
    keyed on the last attempt, leaves pending and held deliveries, goes oldest first, lets one go after 5 tries
    (`providerContentTries`), then deletes our Contents no delivery names; the lease is renewed between the steps; a
    Content Twilio refuses sends the text form instead (`deliveryNote` `richRefused`); carousel cards name no
    `chip_list` and its `body` is cut to 1024.
  - A card has its title or words and one thing more; pictures are PNG, JPEG or GIF, sized by the RCS limits, the
    fallback with the first picture only within the MMS limit; a bridge with no RCS gets the text form and the first
    picture.
  - Typing starts only for a room with an RCS bridge and goes only to a person a message of ours reached; a read never
    changes a failed delivery; the RCS label uses only an outbound RCS bridge and says another bridge reads messages.
- Left:
  - the live tests with Twilio and Sinch, and the facts listed above;
  - our read receipts out (later, with their own setting);
  - rich messages in direct sends and from the terminal.

#### Phase 14: GroupMe adapter (D5, §13.3, §14.5, §17 #10)

Designed 2026-11-17. Built against a fake GroupMe only, so **not yet tested with GroupMe**. The adapter is
`crates/moqui-channels/src/groupme/`, on GroupMe's public API v3 with the user's access token. The facts it relies on,
and those not confirmed, are at the end of this entry.

**New messages: the push channel.** GroupMe offers three ways:

| Way | For | Against |
| --- | --- | --- |
| Push: Faye/Bayeux at `push.groupme.com/faye`, long-polling over HTTPS (chosen) | Official (the push tutorial). One outbound connection a bridge, from the core node: no webhook, no edge node, nothing public. The user channel `/user/<id>` carries every group's, Topic's and direct conversation's messages and likes | A thread a bridge holds a long poll open. Its event types beyond `line.create` are documented by the community only |
| Bot callback URL | Official, a plain POST | A bot in each group, which the group sees; no direct messages; no signature, so anyone with the URL can forge a message; needs a public webhook (the edge node) |
| Polling | Simple | A request a room an interval; slow; GroupMe's rate limit (420) |

The push channel is chosen. Polling is only its catch-up after a reconnect.

**The connection** (`groupme/push.rs`).
- `start()` spawns one thread per bridge, `moqui-groupme-<chatBridgeId>`, holding a `Weak` factory. `stop()` sets its
  stop flag and does not wait for it. Its loop:
  1. `GET /users/me`: the token's GroupMe user id.
  2. Handshake (`/meta/handshake`, `long-polling`), then subscribe to `/user/<id>` with `ext: {access_token, timestamp}`.
  3. Catch up (below).
  4. `/meta/connect` long polls, each with a read timeout of the server's `advice.timeout` plus 15 s. Each answer's
     messages are handled in order.
- Every hour a new handshake and subscription (the tutorial: "Signature must refresh every hour").
- A failed connect, `advice.reconnect: handshake`, a network error or a 5xx: a new handshake after 2 s, doubling up to
  5 min.
- Before each poll the thread reads the bridge row again (status and `configJson`):
  - disabled or removed: the thread ends;
  - a changed token or settings: a new handshake at once.
- The bridge is connected once subscribed. A 401 (from the API or the push server) marks it not connected with "GroupMe
  refused the access token: get a new one (docs/interfaces/GroupMe.md)". The thread then tries again every 5 minutes,
  and at once when `store#Bridge` changes the token.
- New core helpers in `chat_bridge.rs`:
  - `note_status(facade, bridge_id, connected, error)` wraps `set_status` for an adapter's own thread;
  - `bridge_row(facade, bridge_id)` returns the `BridgeInfo` and status.
- **In a cluster** one node holds each bridge's connection (decided 2026-11-17, at the design check). It is the lease
  the replication sender and the SIP trunks already use: a `moqui.edge.EdgeSenderLock` row `groupme:<chatBridgeId>`,
  through `moqui_core::edge::replica::sender::claim`.
  - The lease is 120 s, renewed between steps (each long poll is at most the server's timeout plus 15 s).
  - Another node asks every 15 s and takes the lease over when it lapses.
  - A holder whose claims fail stops after two thirds of the lease since its last good claim.
  - A thread that ends lets the lease go.

  Should two nodes still meet, duplicates stop in `ingest#Event`: one transaction id a GroupMe event
  (`gm:<message id>`), and `ChatEventExternal` is unique on the event.
- **Edge nodes:** nothing. The connection goes out from the core node, and files go out as uploads, not addresses.

**Catch-up** (the polling fallback).
- When it runs:
  - after each subscription;
  - every `syncMinutes` (60).
- Per run:
  - `GET /groups?omit=memberships&per_page=100` (up to `maxRooms`) and `GET /chats?per_page=100`;
  - at the start and hourly, `GET /groups/:id/subgroups` for the Topics.
- What it does:
  - It makes the rooms of chosen groups, Topics and conversations that have none.
  - A room whose `last_message_id` is not its newest mapped event fetches `GET /groups/:id/messages?after_id=<newest>&limit=100`
    (or `GET /direct_messages?other_user_id=…&after_id=…`), at most 5 pages a room.
  - The newest mapped event is the mapping's latest `ChatEventExternal` that is not a like. No cursor field is needed.
  - A new room starts at GroupMe's newest message. History is §15's, later.

**Rooms.**
- `externalRoomId`:
  - `g:<group id>`: a group;
  - `g:<topic id>`: a Topic, which posts and reads as a conversation of its own;
  - `dm:<other user id>`: a direct conversation. Its chat id (`a+b`) is the mapping's `externalThreadId`
    (`store#MappingThread`), for likes.
- Each is a `CrtPrivate`, `CrvMembers` room, not encrypted, with a `CrbBoth` mapping. Not `CrtExternal`: that is a phone
  number's room. So GroupMe rooms list under Bridged.
- The room's name and topic:
  - a group: the group's name, topic "GroupMe group";
  - a Topic: "Group › Topic", topic "GroupMe Topic in {group}";
  - a conversation: the person's name, topic "GroupMe direct messages".
- The bridge's owner (`configJson.ownerUserId`) is the room's owner and its one local member when it is made. The owner
  may add others: they read, and their sends are refused (below).
- A new name from GroupMe renames the room only while the room still has GroupMe's last name (the mapping's
  `externalRoomName`): the owner's own name wins.
- Which rooms (`configJson`):
  - `groupIds` (empty: every group);
  - `skipGroupIds`;
  - `topics` (true);
  - `directMessages` (true);
  - `maxRooms` (200, Topics and conversations counted).

  An event of a room left out is dropped, logged at debug.
- New service file `org.moqui.chat.GroupMeServices` (`GroupMeServices.xml`, `GroupMe.js`), internal (no remote), which
  the adapter calls with authorization off:
  - `ensure#Room` {chatBridgeId, externalRoomId, kind (group, topic, direct), name, parentName, conversationId}: under the
    bridge's row lock, the active mapping or a new room and mapping, as `ensure#PairRoom`. It reads the owner from the
    bridge's `configJson` itself, so no caller names a user. Out: chatRoomId, chatRoomBridgeId, created.
  - `receive#Likes` {chatBridgeId, externalRoomId, messageId, likedByList, ownGroupMeId, names}: the likes of one message
    as a state.
    - It compares the heart reactions of the room's external actors on that message with `likedByList`.
    - It sends `ingest#Event`: a reaction for each new like (`like:<message>:<user>:<n>`), and a redaction of the latest
      like for each one taken back.
    - The owner's own GroupMe id counts as liked while the owner has a local heart on the message, so the owner's like
      does not come back as a second heart.
    - One transaction id per state (`gml:<message>:<hash of the sorted ids>`), so a repeat, or another node's, is a
      duplicate.

**In** (`groupme/events.rs`). Each push message or caught-up message is one `ingest#Event` batch. The adapter runs it
under its bridge's transport mark (`sms::with_mark`): its own TLS connection to GroupMe, signed in with the token, is the
transport check. Nothing comes in on the webhook route: `webhook()` keeps the default 401.

| GroupMe | Becomes |
| --- | --- |
| `line.create`, `direct_message.create`, a caught-up message | A message as the GroupMe person (`externalUserId` its `user_id`, its `name`, its `avatar_url` when on `https://i.groupme.com`, `originDate` from `created_at`). Image (`i.groupme.com`) and video (`v.groupme.com`) attachments are `media`, up to 10. A location is a line "Location: {name}" with an OpenStreetMap link built from the coordinates. A file is the line "[A file on GroupMe: open GroupMe to get it]". A `reply` attachment is the reply's target. Its `favorited_by` goes to `receive#Likes` |
| `@all` or `@everyone` as a word in the text | `item.mentions: ['@room']`. `process#InboundItem` (small change) makes the first such word `<span data-mention-id="@room">`. Any other value in `mentions` is `malformed`. `send#Message`'s @room rule still applies |
| `favorite` | `receive#Likes` with `subject.line.favorited_by` |
| `message.update`, `message.deleted` (community-documented) | An edit (`edit:<id>:<updated_at>`) or a delete of the message. `ingest#Event`'s own-sender rule applies |
| `membership.create` | `ensure#Room` for the group, when chosen |
| `system: true`, `typing`, `ping`, other types | Dropped |
| The owner's own message sent from here (its `user_id` the token's, its `source_guid` starting `moqui-chat:`) | Dropped: the echo of a send, even before the send's mapping is written |

- The owner's own messages and likes from the GroupMe app show as the owner's GroupMe person, a member like the others.
  The bridge layer has no puppet of a local user.
- `fetch_media` fetches only `https://i.groupme.com/…` and `https://v.groupme.com/…`, through the guarded client (public
  addresses only, `maxInboundMediaBytes`, at most 20 MiB). `store#BridgeMedia`'s safe types apply.

**Out** (`groupme/mod.rs` `deliver`).
- **Only the owner speaks for the account.** An event of another local member fails `notBridgeOwner` ("GroupMe messages
  go from {owner}'s account only").
- **A message.**
  - Text: up to 1,000 characters (`textTooLong`).
  - `POST /groups/:id/messages` `{message: {source_guid, text, attachments}}`, or `POST /direct_messages`
    `{direct_message: {source_guid, recipient_id, text, attachments}}`.
  - `source_guid` is the delivery's idempotency key, and `X-Access-Token` the token.
  - 201: `Accepted` with GroupMe's message id.
- **@room.** An `@room` mention in the HTML goes as the text `@all`, with a `mentions` attachment of every member but
  the owner, each at that text. The members come from `GET /groups/:id`, kept 10 minutes. There is none in a direct
  conversation.
- **Pictures.** Each image file is read through a new core helper, `chat_bridge::delivery_file(bridge_id,
  attachment_id)`: `own_media`'s check that the message went to the bridge, without the media token. It is posted to
  `image.groupme.com/pictures` with its content type and the token, and its `payload.url` becomes an `image`
  attachment.
  - A picture over `maxImageBytes` (10 MB) fails `mediaTooLarge`.
  - Any other file fails `fileNotCarried` ("GroupMe takes only pictures from this server"). GroupMe's video and file
    services are community-documented only.
- **A like.** A heart reaction (❤️, ❤, ♥️, ♥) is `POST /messages/<conversation>/<message>/like`; its removal is
  `…/unlike`. The conversation is the group's or Topic's id, or the mapping's chat id.
  - Any other reaction fails `reactionNotCarried` ("GroupMe carries only the heart").
  - An edit or a delete is `bridgeUnsupported`, refused before the change by the capabilities.
- **GroupMe's answers.**
  - 200 and 201: accepted.
  - 400: `Failed` `badRequest`, with GroupMe's `meta.errors`.
  - 401: `Retry` with the bridge not connected.
  - 403: `Failed` `refused`, with GroupMe's error (its update limits; not in the group).
  - 404: `Failed` `notFound`.
  - 420, 429, 502, 503: `Retry`.
  - 409 (the same `source_guid` within a minute), a 500, or an answer lost in flight: the room's latest 20 messages are
    looked up for our `source_guid`. Found, it is `Accepted` with that id; else `Uncertain`.
- **Capabilities:**
  - `edit` false, `delete` false, `reaction` true;
  - `idempotent` false: GroupMe dedupes for a minute only, shorter than the lease;
  - `sends_per_minute` 20 (`sendsPerMinute`);
  - `encryption` `None`, `group_max` 0.

**The data model and configuration.**
- `ChatEntities.xml`: Enumeration `CbtGroupMe` (GroupMe, `enumCode` `groupme`). No new entity or field, and no
  migration.
- `configJson`, encrypted as every bridge's:
  - `accessToken` and `ownerUserId` (both required);
  - the room choices above, `syncMinutes`, `sendsPerMinute`, `maxImageBytes`, `maxInboundMediaBytes`;
  - `apiBaseUrl` (`https://api.groupme.com/v3`), `imageBaseUrl` (`https://image.groupme.com`), `pushUrl`
    (`https://push.groupme.com/faye`), and `allowInternalAddresses` (tests only, never on production).
- `store#Bridge` for `CbtGroupMe` checks (`GroupMeServices.check#Bridge`):
  - the adapter is `groupme`;
  - `accessToken` and `ownerUserId` are there, and the owner is a user;
  - the owner does not change while the bridge has rooms (`ownerFixed`);
  - the owner has no other GroupMe bridge (`oneBridge`).
- `lib.rs`: `pub mod groupme;` and its line in `adapters()`.

**Self-service** (decided 2026-11-17, at the design check: in the first release).
- Settings, Chat gets a GroupMe section (`Settings/Chat.xml`, which renders in both modes). It shows:
  - the status (Connected with the room count; Not connected with the reason; Disconnected);
  - Access token (a password field, write-only) and Connect, or Connect again;
  - Disconnect.
- `GroupMeServices`, the current user's own bridge:
  - `get#MyGroupMe`: the status, never the token.
  - `connect#MyGroupMe`: makes the user's GroupMe bridge (owner the user, adapter `groupme`), or replaces its token and
    connects it again. An empty token keeps the one kept. At most one per user (the user's row lock, then a look).
  - `disconnect#MyGroupMe`: the bridge is disabled, so the worker stops its thread. The token is forgotten. The
    `lastErrorText` says "Disconnected by its owner: the rooms keep their history and no longer sync with GroupMe".
    The rooms stay.
- Only the owner (these services) and a `ChatBridgeAdmin` (System, Chat Bridges, `find#Bridges`, `store#Bridge`) see or
  change the bridge. The token is kept in the encrypted `configJson`.
- The property `chat@groupme-self-service` (`true` by default) turns it off. When `false`, Connect is refused
  (`selfServiceOff`) and the section shows only a bridge the user has, without its token form.
- The administrator's path stays: System, Chat Bridges takes a GroupMe bridge as any other (the network, the adapter and
  the JSON configuration), and the guide gives the keys.

**Screens.**
- Settings, Chat, GroupMe (above). BasicComms.spec's one render of Settings, Chat checks it.
- GroupMe rooms show in Chat's Bridged section with the GroupMe chip, a failed send's reason, and Send again.
- The label (§6.2), in `WebrootChat.js` `e2eeLabel` and `ChatTui.xml`: a room whose bridges all have `encryption`
  `None` and are not SMS says "Not end-to-end encrypted. {network} has no end-to-end encryption, and the bridge reads
  messages to pass them on." The rule is generic, not GroupMe's alone.
- The terminal shows the same label line, and nothing new besides.
- Phone and desktop: the same rooms, label and captions. Nothing depends on the width.

**Failure tolerance.**
- Each push message, each caught-up message, each room and each like state is worked on its own. One that fails is
  logged with the bridge and GroupMe's id, and the rest go on. A room not made is tried again at the next sync.
- A file not fetched is named in its message (`ingest#Event`'s rule).
- One message's handling runs under `catch_unwind`: a panic is logged and the thread goes on.
- The thread ends only when its bridge is stopped, disabled or removed. A failure backs off and connects again.
- A catch-up page that fails is logged, and that room waits for the next run.

**Security and abuse.**
- The token is only in the encrypted `configJson`. It is never returned (`store#Bridge`), never logged (errors are
  scrubbed of it), and sent only over https: `start()` refuses an http URL but a test's `allowInternalAddresses`. It
  goes in the `X-Access-Token` header and in the Faye `ext`, never in a URL.
- No public endpoint: input comes only over the bridge's own connection, and every `ingest#Event` check applies (no
  local identity, the batch's bounds, the dedupes). The transport mark is set only around the adapter's own calls.
- Files are fetched from GroupMe's two media hosts only, through the guarded client. Avatars are kept only from
  `i.groupme.com`.
- No local user speaks through another's account: only the owner's events go out.
- `@room` comes only from the word `@all` or `@everyone`, and `send#Message`'s rule for who may mention the room still
  applies.
- Bounds: `maxRooms` (a stranger adding the owner to many groups, or many direct messages, makes at most that many rooms;
  `directMessages` false stops strangers' rooms); `sendsPerMinute`; 420 and 429 backed off; at most 5 catch-up pages a
  room.

**Left out.**
- Edits, deletes and replies going out. GroupMe's edit is a v4 call documented by the community only.
- Typing, read receipts, polls, events, galleries, emoji PowerUps (they show as their placeholder), reactions other than
  the heart (GroupMe's newer reactions are community-documented), and sending video and files.
- History beyond the catch-up (§15), and GroupMe's system messages.
- A group's whole roster as members: a GroupMe person joins the room when they first post or like.
- GroupMe's OAuth flow for applications: the user pastes their token.
- The WebSocket transport: long-polling needs no new crate.
- Archiving a room when the owner leaves its group. GroupMe's 403 or 404 fails its sends with the reason.
- Bots.

**Tests** (regression):
- `crates/moqui-channels/src/groupme/tests.rs`, on a fake GroupMe on loopback (REST, image service and Faye long-poll).
  The token is built with `format!`. The push session is driven a step at a time (`push::Session::step`), with no
  timing:
  - send: a text with the idempotency key as `source_guid` and the token header; `textTooLong`; another member's
    message refused (`notBridgeOwner`);
  - receive: the handshake, the subscription with the token in `ext`, and a `line.create` into the group's room as
    the GroupMe person with name and avatar; the same message on two connects kept once; the owner's echo dropped;
  - a like both ways: a heart out as `/like` and its removal as `/unlike`; another emoji refused; a `favorite` in as a
    heart and taken back; the owner's own like not doubled;
  - `@all`: in as an `@room` mention; `@room` out as `@all` with every member mentioned;
  - an attachment: a picture out through `/pictures` then as an `image` attachment; a picture in, fetched; a PDF out
    refused; a file from another host not fetched;
  - a Topic: a message with a `parent_id` into its own room, "Group › Topic", and a reply sent to the Topic;
  - a direct message: `direct_message.create` into the conversation's room; the reply through `/direct_messages` with
    `recipient_id`; a like with the chat id;
  - an expired token: a 401 on a send is `Retry` with the bridge not connected; a 401 at the subscription marks it not
    connected with the guide's message; a new token in `configJson` connects again;
  - rate limits: 420 and 429 on a send are `Retry`; the session backs off on a push 5xx;
  - catch-up: after a reconnect, the messages after the room's newest fetched with `after_id`;
  - a lost answer found by its `source_guid`;
  - `store#Bridge`'s checks; the capabilities (`None`);
  - self-service: one bridge a user, the token never shown, Connect again with no token, Disconnect, the property off;
  - the lease: one node holds it, another is refused.
- `webroot/test/client/WebrootChat.spec.js`: the label of a room whose bridge's network has no E2EE.
- `basic-comms/test/BasicComms.spec.js`: the GroupMe section on the existing Settings, Chat render.

**Docs:**
- New: `docs/interfaces/GroupMe.md`, the operator guide, marked **not yet tested with GroupMe**: getting the token
  (dev.groupme.com, Access Token), the owner, which groups (and the keys), what is carried and what is not, the limits,
  the facts not confirmed.
- Changed:
  - `Chat-Bridges.md`: the adapter, the network that connects out, the label;
  - `docs/tools/Comms.md`: GroupMe, beside Bridged rooms;
  - `docs/Run-Deploy.md`: the property `chat@groupme-self-service`;
  - `Cargo.toml`'s description;
  - this plan's status.

**The API facts** (dev.groupme.com, read 2026-11-17).
- Confirmed:
  - the base URL, the `X-Access-Token` header, the `{response, meta}` envelope and the status codes (420 for the rate
    limit);
  - messages index (`after_id`, `limit` up to 100, 304 when none) and create (`source_guid` deduped a minute with 409,
    text up to 1,000, `image` attachments only from `i.groupme.com`);
  - the image service (`POST image.groupme.com/pictures`, `payload.url`);
  - `like` and `unlike` at `/messages/:conversation_id/:message_id/`;
  - `/chats`, `/users/me`;
  - the push handshake, the subscription to `/user/<id>` with `ext.access_token` and `timestamp`, long-poll connect,
    the `line.create` shape and the hourly signature.
- Not confirmed (from the community's docs, groupme-js.github.io/GroupMeCommunityDocs, or not found):
  - `GET` and `POST /direct_messages`: the current page lists Direct Messages but shows only Chats;
  - Topics (`/groups/:id/subgroups`, a Topic's messages under its own id, `parent_id` on `line.create`);
  - the push types `direct_message.create`, `favorite`, `message.update`, `message.deleted` and `membership.create`;
  - the `mentions` (`user_ids`, `loci`) and `reply` attachments, and video at `v.groupme.com`;
  - the order of a chat id (`a+b`);
  - the rate limit's numbers and the image size limit;
  - whether GroupMe itself treats `@all` as anything but text.

**Status (2026-11-17): built**, against a fake GroupMe only. Not yet tested with GroupMe.
- Built as designed, with the design check's two changes (self-service in Settings, Chat, and one node holding each
  connection by the lease), and these details:
  - `receive#Likes` runs with no transaction of its own (`transaction="ignore"`), as `receive#Sms` does, so
    `ingest#Event` stores its batch first.
  - After a refused token the thread asks for the bridge every 10 s during its wait, so a new token connects within
    seconds, not at once.
  - Settings, Chat is an XML screen that renders in both modes, so the terminal has the GroupMe section too.
- Review fixes (2026-11-17):
  - **Re-likes.** A likes state's transaction id has the time too, so a like after an unlike after a like applies.
  - **The catch-up mark.** `ChatRoomBridge.syncCursor` replaces "the room's newest mapped event". It moves past a page
    taken whole; a failed page or message, or the page cap (`catchUpPageSize`, `catchUpPages`), stops it there, and the
    next run (within a minute) resumes. A pushed message moves it only in a caught-up room.
  - **Redirects.** The token's calls follow none; a file follows at most three, each to the file hosts only.
  - **The label.** `Encryption::NoneByNetwork`, which only the GroupMe adapter says, is what the label names a network
    by; `None` alone stays the plain bridged label.
  - **One coordinator a node**, not a thread per bridge on every node.
    - It lists the GroupMe bridges in one query and claims or renews all their leases in one pass (`claim_all`), up to
      `chat@groupme-node-max` (50), and runs a session thread only for each bridge it holds.
    - The leases are renewed apart from the sessions, so no step outlasts them; a long poll is at most 60 + 15 s.
    - Each coordinator claims under its own token, so an old one never frees a new one's lease.
    - A bridge's configuration is read again only when its `lastUpdatedStamp` moved.
    - A long wait is kept as `ChatBridge.retryDate`, so a hand-over waits it out.
    - A panic in a pass is caught, and the next pass runs; a coordinator that ended starts again at the next `start`.
  - **The status.** The adapter says `keeps_status`, so the worker's `start` no longer marks it connected; only the
    holder's session writes it, only when the row says otherwise. The last error is a code and a text (`tokenRefused`,
    `connectFailed`, `noToken`, `roomLimit`, `disconnected`), which Settings, Chat says in the user's language.
  - **Outcomes.** A picture's or the account's failure before the post is `Retry`. A 500, 502, 503 or lost answer looks
    the message up; not found, it is tried again within the first three attempts (GroupMe's minute of dedupe), then
    uncertain.
  - **Direct messages.** The answer's `message` (the community's docs), else `direct_message`; its chat id is kept when
    new.
  - **`@room` in a Topic** mentions its group's members (the Topic mapping's `externalThreadId` is its group). With the
    members unread, or more than 100 others, `@all` goes as text alone.
  - **Room limits.** `maxDirectRooms` (50) inside `maxRooms`, so groups come before strangers' messages; a refusal is
    kept an hour per conversation; the status says "room limit reached".
  - **Owner.** `ChatBridge.ownerUserId`, kept plain, finds a user's own bridge without reading every configuration.
  - `Config`'s `Debug` hides the token; the screen and service texts go through `ec.l10n.localize`.
- Left:
  - the live test with GroupMe, and the facts not confirmed (`docs/interfaces/GroupMe.md`, "Not confirmed");
  - what "Left out" names.

#### Phase 15: scheduled send and message reminders

Designed 2026-11-17 (§14.2, §14.3, §17 #3), with the user's change for encrypted rooms (§19.1); built 2026-11-18 (the
status at the end of this entry). A chat message gets a send time, and the server holds it and sends it then,
as mail's due drafts are sent (`EmailClientServices.send#DueDrafts`). "Remind me" on any message brings it back later as
a notification and an Activity row.

**Encrypted rooms: the sender's own device sends it** (decided 2026-11-17, §19.1). The server cannot encrypt for the
user, and a ciphertext made at schedule time would be wrong at the send time:
- A Megolm session's key goes to the devices that are in the room when the message is encrypted. A member who left
  before the send time could still read it, and a member or device added meanwhile could not.
- A held ciphertext pins an old session past its rotation.

So the device that scheduled the message holds it and encrypts it at the send time:
- **At schedule time** that device keeps the draft (its text, reply and thread) in its browser storage, sealed with
  AES-GCM under a key derived from the device's store key (HKDF, `moqui-chat-held-draft`). The store key comes from
  `/ChatKeys/storeKey`, which answers only the device's own user, so neither the stored draft nor the server alone can
  open it. The server keeps a placeholder: a `ChatScheduledMessage` with no body and no text, its time, and the device
  that holds it (`heldByDeviceId`, with its name).
- **At the time**, the holding device, if it is open (a browser tab, the desktop app, the phone app while Android lets
  it run), claims the placeholder (`claim#HeldMessage`: a 2-minute lease, a compare-and-set on the row). It then sends
  through the normal encrypted send with clientRef `sched:<id>`, and reports it (`complete#HeldMessage`). A claim that
  finds the room already has the `sched:<id>` message marks the row sent and answers `sent`, so a lost report sends
  nothing twice.
- **No device open at the time:** the holding device sends it when it next opens Chat. The row is then `sentLate`, and
  the sender sees "Sent late, at 9:14" in the room's scheduled list until they dismiss it.
- **The holding device is gone** (revoked, or its stored draft lost: a cleared browser, another browser profile): every
  device of the sender lists it as "Can't be sent: the draft was on Firefox on Linux". Only Cancel is offered.
- **Other devices** of the sender list it, with its time, as "Held on Firefox on Linux". They can Cancel it. Edit and
  Send now are offered only on the holding device.
- The server's job never sends a held row. It fails one only when its device is revoked (`deviceGone`).
- **The limits** (said in the dialog and in [Comms](../docs/tools/Comms.md)):
  - it goes only when that device is open;
  - text only: files, polls and voice notes are sent now;
  - it can be edited only on that device;
  - a cleared browser loses it.

The rest of a room's encryption is untouched. Reminders work in encrypted rooms: a reminder keeps only the room and
the message's number.

**Data model** (new `framework/entity/ChatScheduleEntities.xml`, listed in `MoquiDefaultConf.xml` `load-entity`)

| Entity, field | What |
|---|---|
| `moqui.chat.ChatScheduledMessage` (`chatScheduledMessageId` pk, sequenced) | A message the server holds: `chatRoomId`, `senderUserId`, `sendDate`, `statusEnumId`, `body` (the composer's safe HTML, cleaned as `send#Message` cleans it), `bodyText` (for the list), `replyToEventSeq`, `threadRootEventSeq`, `recordsJson` (the record cards looked up at schedule time), `sentEventSeq`, `sentDate`, `attemptCount`, `reasonCode`, `failText`, `createdDate`. An encrypted room's placeholder: no body or text; `heldByDeviceId`, `heldByDeviceName` (the `ChatDevice` that holds the draft), `leaseDeviceId`, `leaseExpiresDate`, `sentLate` (Y: sent after its time, by the device opening), `lateSeenDate` (the sender dismissed it). Indexes: (`statusEnumId`, `sendDate`), (`senderUserId`, `statusEnumId`) |
| `ChatScheduledMessageStatus` (Enumeration) | `CsmScheduled`, `CsmSent`, `CsmFailed` (not sent: `reasonCode`, kept for the sender to see), `CsmCancelled` |
| `ChatAttachment.chatScheduledMessageId` (extend-entity) | A file uploaded for a scheduled message: `clean#PendingAttachments` leaves it while the message waits |
| `moqui.chat.ChatMessageReminder` (`chatMessageReminderId` pk, sequenced) | A reminder: `userId`, `chatRoomId`, `eventSeq`, `remindDate`, `statusEnumId`, `firedDate`, `reasonCode`, `createdDate`. No text of the message, in any room. Indexes: (`statusEnumId`, `remindDate`), (`userId`, `statusEnumId`), (`userId`, `chatRoomId`, `eventSeq`) |
| `ChatMessageReminderStatus` (Enumeration) | `CrmPending`, `CrmFired`, `CrmCancelled`, `CrmDropped` (not sent: the message is gone or the room can no longer be read) |
| `ChatUserActivityKind` `CuaReminder`; `PushOutboxKind` `PokReminder` | A fired reminder in Activity; its push in the outbox (seeded in the new file) |
| `NotificationTopic` `ChatReminder`, `ChatScheduleFailed` | The in-page alert of a fired reminder (pushed by the chat push worker only: in `PUSHED_TOPICS`); a scheduled message that was not sent (`ChatSetupData.xml`) |

Limits: 100 scheduled messages and 200 pending reminders a user; a time at least one minute ahead and at most a year.

**Services** (new `org.moqui.chat.ChatScheduleServices`, logic in `ChatSchedule.js`). Each checks the caller: a row of
someone else is "not found".

| Service | What |
|---|---|
| `schedule#Message` | `chatRoomId`, `body`, `replyToEventSeq`, `threadRootEventSeq`, `chatAttachmentIdList`, `recordList`, `sendDate`. The rules of `send#Message` now: a member, not archived, a body or files, the files the sender's unsent uploads of the room (each then marked with the message). Refused: an encrypted room (`encryptedRoom`), a time out of range (`badTime`), over the limit (`tooMany`). In an encrypted room: no body, no files, `heldByDeviceId` (a device of the caller's, not revoked) required; a body there is refused (`encryptedRoom`). Out: `chatScheduledMessageId`, `scheduled` (as listed) |
| `claim#HeldMessage` | `chatScheduledMessageId`, `deviceId`, `now` (Send now): the holding device takes a held row that is due (or Send now), still `CsmScheduled`, with no live lease of another claim: a lease of 2 minutes. Out: `claimed`, or `sent` when the room already has its `sched:<id>` message (then marked sent) |
| `complete#HeldMessage` | `chatScheduledMessageId`, `eventSeq`: the claiming device's report. The event must be the sender's `sched:<id>` message of the room. `CsmSent`, `sentLate` when sent more than 5 minutes after its time |
| `dismiss#ScheduledMessage` | The sender has seen "Sent late": `lateSeenDate` |
| `update#ScheduledMessage` | A new `body` or `sendDate`, with the row locked; only while `CsmScheduled` (else `notScheduled`: it was sent or is being sent). The files stay; to change them, cancel and schedule again |
| `cancel#ScheduledMessage` | `CsmCancelled`; its files removed (`remove#AttachmentFile`), each on its own |
| `send#ScheduledMessageNow` | The due path for one of the sender's rows (`CsmScheduled` or `CsmFailed`), at once |
| `get#ChatSchedule` | The caller's `scheduledList` (scheduled and failed, and sent late not yet dismissed, by room, soonest first; a held row with `heldByDeviceName` and `deviceGone` when its device is revoked) and `reminderList` (pending, soonest first): each reminder with the room's name, the sender's name and, in a plain room, the message's current snippet read now. An encrypted room's reminder has no snippet: `encrypted` true and the event's ciphertext (`encryptedContent`) for the browser, as `find#ChatActivity` answers |
| `set#Reminder` | `chatRoomId`, `eventSeq`, `remindDate`: a message (`CetMessage`, not redacted) of a room the caller can read now (`get#RoomAccess`). It replaces the caller's pending reminder of that message. Refused: `badTime`, `tooMany`, not found |
| `cancel#Reminder` | `CrmCancelled` |
| `send#DueMessages` (job) | `send_DueChatMessages_frequent`, every minute (`ChatSetupData.xml`). The `CsmScheduled` rows due that the server holds (no `heldByDeviceId`), oldest first, at most 200 a run, no send started after 45 s. Each row in a transaction of its own (`deliver#ScheduledMessage`), see below. A held row due whose device is revoked is failed (`deviceGone`); the others wait for their device |
| `send#DueReminders` (job) | `send_DueChatReminders_frequent`, every minute. Each pending reminder due, in its own transaction: the room still readable by its user and the message still there (else `CrmDropped`, with `reasonCode` `noAccess` or `messageGone`); a `CuaReminder` Activity row (`activityDate` now, `actorUserId` the message's sender); its push (`queue#ReminderPush`); the `ChatReminder` notification; `CrmFired` |
| `queue#ReminderPush` | Rust, in `chat_push.rs`: one `PushOutbox` row of kind `PokReminder` for each device of the user, in the caller's transaction; the worker woken after the commit |
| `clean#ScheduledMessages` (job) | `clean_ChatSchedule_daily`: sent and cancelled rows and fired, cancelled and dropped reminders after 7 days; failed rows after 30 days, with their files |

**The send at its time** (`deliver#ScheduledMessage`):
1. Lock the row; go on only if it is still `CsmScheduled` and due (another node or Send now took it otherwise).
2. Run as the sender (`run#AsSender`, see the review fixes below), so the chat rules take the sender at the send time
   and the app's screens are checked again as them.
3. Refuse when the sender's place in the room is out of their sight (`filterEnumId` `CmfBlocked`: they blocked its
   number or its person since): reason `blocked`.
4. `send#Message` with the row's body, reply, thread, files and records, `roomUrl` from `comms@app-root`, and
   `clientRef` `sched:<id>`. It checks membership (`notMember`) and the archive (`archived`) itself, and resolves the
   mentions as they are then.
5. In the same transaction: `CsmSent`, `sentEventSeq`. The `sched:` clientRef also makes a retry after a crash answer
   the message it made (`duplicate`): it is sent once.

A refusal (a `reasonCode`, a validation error) rolls the send back. Then, in a transaction of its own, the row is set
to `CsmFailed` with the reason, the sender gets `ChatScheduleFailed` ("Your scheduled message in Team was not sent: you
are no longer in the room"), and their lists are told. Any other error leaves the row `CsmScheduled` with
`attemptCount` + 1, to be tried at the next run, and it fails after 5 tries.

**Bridges.** At its time, a scheduled message is an ordinary message of its sender, through `send#Message` and
`append#Event`. So SMS, RCS and every other bridge get it through their `ChatBridgeDelivery` and adapter as any message.
The adapter's checks then apply at that moment: `check#SmsSend` (the destination's class, STOP and the opt-out, the
day's limits), the bridge's capabilities and its fallback. Nothing is checked only at schedule time.

**Blocks** (D1, phase 11):
- The sender blocked the room's number or person since scheduling: not sent (`blocked`).
- The other person blocked the sender: the message is sent as an immediate send would be, hidden from them and
  notifying nothing. Failing it would tell the sender they are blocked, which a block never does.

**Pushes to the sender's lists.** Every change goes on `chatUser:<userId>` as
`{kind: schedule, chatRoomId, what: scheduled | reminder}` (`push#Signal`, after the commit). Each open Chat of the
sender reads `schedule` again. No feed entry: `get#ChatSchedule` is read whole at Chat's opening and on that push.

**The reminder push** (§7.2 rule 9, new). A `PokReminder` row, sent by the same worker, encryption and transport, with
these differences from a message push:
- Not dropped by a read marker (rule 3) or by `queue#ReadClear`: the message was usually read already.
- Its own collapse key and tag (`chat-reminder-<id>`), so a room's clear never closes it and it replaces nothing of the
  room.
- Held during do not disturb, as messages are, and sent when it ends. It lives a day.

The payload is the kind every client already shows: `{kind: notification, topic: ChatReminder, title, body, link,
tag}`.
- `title`: "Reminder: Ann in Team", or "Reminder: Ann" in a direct room.
- `body`: the snippet in a plain room when `CHAT_NOTIFY_PREVIEW` is not N; else "A message you asked to be reminded
  of".
- `link`: the message (`Chat?chatRoomId=…&eventSeq=…`).

The web's service worker shows title, body and link as they are. The Android app shows the title of a `notification`
push and opens the app; showing the body and opening the message wait for its next change.

**Transitions** (in a new `basic-comms/template/ChatScheduleTransitions.xml`, each one `transition-include` line in
`Chat.xml`): `chatSchedule` (get), `scheduleMessage` (looks up the body's record cards as `send#ChatMessage` does,
`lookup#Records`), `updateScheduled`, `cancelScheduled`, `sendScheduledNow`, `claimHeld`, `completeHeld`,
`dismissScheduled`, `setReminder`, `cancelReminder`. A refusal
is answered with its `reasonCode` and errors, as `send` is.

**Screens** (`Chat.qvue`; two new sub-components in their own block: `chat-when` and `chat-scheduled`)

| Part | Phone (bubbles) | Desktop |
|---|---|---|
| Send later | A long press on Send (450 ms, with a short vibration) opens a sheet: "Send later…" | A narrow arrow beside Send, and a right click on Send, open a menu: "Send later…" |
| When (`chat-when`) | Full-screen sheet. Send later: In 1 hour, Tomorrow morning (8:00), Tomorrow afternoon (13:00), Monday morning (8:00, on a Friday or Saturday), Pick a date and time (`m-date-time`). The choices are in the device's zone, and the next step says the time picked ("Sends tomorrow at 8:00") | A 400 px dialog with the same list |
| Scheduled bar | A line above the composer: "Scheduled: tomorrow at 8:00", or "3 scheduled messages", and "1 not sent" in red when one failed. A tap opens the list | Same |
| The list (`chat-scheduled`) | Full screen. Each message: its time, its text (two lines), its files, the message it replies to, "in a thread"; Edit (the text in `m-rich-text` with mentions, and the time), Send now, Cancel (confirmed). A failed one shows its reason, with Send now and Cancel. An encrypted room's: its text from this device's sealed draft; on another device "Held on Firefox on Linux" with Cancel only; "Can't be sent: the draft was on …" when that device is gone; "Sent late, at 9:14" with a × | A 560 px dialog, the same rows |
| Remind me | "Remind me…" in `messageActions`, after Mark unread from here, for one who can read the room and a sent message (not a pending or failed one): In 20 minutes, In 1 hour, Tomorrow (9:00), Pick a date and time. A message with a reminder says "Reminder at 15:40…" there, with Cancel the reminder in the picker; an alarm icon by its time | Same, from the hover toolbar's More and the right-click menu |
| Reminders | At the top of Activity: the pending reminders, each with its time, room, sender and text (an encrypted room's decrypted here), opening the message, and a × to cancel. A fired reminder is an Activity row (alarm icon, "Reminder: Ann's message") | Same |

- Disabled with a reason, never hidden: Send later while offline ("Send later needs a connection"); while files go
  up; in an encrypted room with files ("In an encrypted room a message with files is sent now") or with no crypto
  device here.
- In an encrypted room the When step says the limits: "This device keeps the message and sends it when the time
  comes. It must be open then; if not, it sends it when it next opens."
- The held drafts: `moqui.chat.heldDrafts` in `WebrootChat.js` keeps them sealed in `storage` under the user's scope
  (`{iv, sealed}` by `chatScheduledMessageId`); `WebrootCrypto.js` gains `sealLocal` and `openLocal` (AES-GCM, the key
  derived from the device's store key with HKDF). Chat's timer looks each minute, and when Chat opens, for this
  device's held rows that are due, and claims, sends and completes each in turn. A send that fails keeps the draft and
  the row, to be tried at the next look.
- Send later is not offered for a voice note, a poll, a forward or an edit.
- The thread composer has Send later too.
- Every text through `t()`.
- The helpers are in a block of their own in `WebrootChat.js`:
  - `moqui.chat.whenChoices(kind, now)`: the choices and their times for `send` and `remind`;
  - `scheduledLine(list, now)`: the bar's text;
  - `reminderOf(reminders, chatRoomId, eventSeq)`;
  - `reduce` answers the `schedule` push as `{kind: schedule}`;
  - `dueHeld(list, deviceId, now)`: this device's held rows to send.

**Terminal.** No change: `ChatTui.xml` neither schedules nor reminds. A scheduled message shows there once it is sent.

**Phone and desktop.** The same bundled Chat in the Android app and the desktop app (`bundled-screens.js`
regenerated). The desktop app shows a fired reminder by its socket (the `ChatReminder` notification), as it shows
messages.

**Failure tolerance.**
- Each due message and reminder is on its own, in its own transaction: one that fails is logged and the rest go on.
- A run stops starting sends after 45 s, and what is left goes at the next run.
- A reminder whose push cannot be queued (no device, an error) is logged and still fires: its Activity row and its
  notification hold it.
- The failure notice and the list push are logged when they fail, and never fail the send.
- `get#ChatSchedule` failing at Chat's opening: logged, and Chat works without the bar and the reminders.
- The file cancel: each file on its own.

**Security and abuse.**
- Only the sender sees or changes their scheduled messages and reminders. Another member learns of a message only when
  it is sent.
- The rules are those of the send time: signed in as the sender, membership, the archive, the sender's blocks and the
  account are checked again, and the mentions are resolved then.
- The server holds no text of an encrypted room's scheduled message: its placeholder has the time and the device
  only. The device's sealed draft opens only with the store key, which the server gives only to that device's signed-in
  user. A claim and a report are the sender's own, for a device of theirs that is not revoked; the report must name the
  room's `sched:<id>` message of the sender.
- A reminder keeps no text of any message. Its push follows rule 5 (no snippet for an encrypted room or with previews
  off). It is checked against the room's access at its time, so a user taken out of a private room gets nothing of it.
- Bounded: 100 scheduled messages and 200 reminders a user, at most a year ahead, and 200 sends a run. The send
  limits of each network are the adapter's, at the send time.
- The body is cleaned at the schedule (`cleanChatHtml`, our own images only) and again by `send#Message`.
- A block is never revealed to the person blocked.

**Left out.**
- In encrypted rooms: a send by the server, files, and editing on another device (above).
- A recurring message or reminder.
- Snoozing a fired reminder (set it again from the message).
- A note on a reminder: it would be text the server keeps.
- Changing the files of a scheduled message.
- Scheduling a forward, poll or voice note.
- The terminal and XMPP clients.
- Android's reminder body and its link to the message: the current APK shows the title only, until its next change
  (§18.4, [Mobile-Client](../docs/ui/Mobile-Client.md)).
- A held message sent by the phone app in the background: it goes when Android lets the app's WebView run.
- Showing in the rail which rooms have scheduled messages.

**Hooks in shared files** (small, so the parallel phases merge):
- `ChatServices.xml`: `clean#PendingAttachments` leaves files with a `chatScheduledMessageId`; `find#ChatActivity`'s
  kind maps gain `reminder`.
- `Chat.xml`: the ten `transition-include` lines.
- `ChatSetupData.xml`: the three jobs and two topics.
- `MoquiDefaultConf.xml`: the entity file.
- `chat_push.rs`: `queue_reminder_push`, its registration, the exemption from rule 3, the collapse key, and
  `PUSHED_TOPICS`.
- `WebrootChat.js`: the helper block and one `reduce` branch; `WebrootCrypto.js`: `sealLocal`, `openLocal`.
- `Chat.qvue`: the Send menu, the bar, the menu item, the Activity section and the feed's kind.

**Tests (regression).**
- Server: `framework/test/ChatSchedule.spec.js` (new), on `ec.user.setEffectiveTime` and direct calls of the job
  services:
  - `schedule#Message`'s refusals: not a member, archived, a body in an encrypted room or no device there, past,
    more than a year, empty, over the limit;
  - a held placeholder: the job never sends it; `claim#HeldMessage` only by its device, when due, once while leased;
    `complete#HeldMessage` refused for another event; sent late; a claim after a lost report answers `sent`; the
    device revoked: `deviceGone`; Cancel from another device;
  - not sent before its time, sent at it as the sender (the body, a file, the reply), then never again;
  - edited (text, time) before; refused after it was sent; cancel removes its files; Send now;
  - at the send time: left the room (`notMember`), archived (`archived`), the sender blocked the room (`blocked`),
    each `CsmFailed` with a `ChatScheduleFailed` notification; the other person blocked the sender: sent and hidden
    from them;
  - a bridged room (a `ChatRoomBridge` out): its `ChatBridgeDelivery` written at the send time;
  - `clean#PendingAttachments` leaves a scheduled message's file;
  - `set#Reminder` refused for a room the user cannot read, a past time and over the limit; it replaces a pending one;
  - `get#ChatSchedule` with a plain room's snippet and none for an encrypted room's;
  - `send#DueReminders`: a `CuaReminder` Activity row (`find#ChatActivity` kind `reminder`), a `PokReminder` row for
    the user's device with no snippet in an encrypted room, nothing on a second run, a redacted message and a room
    left `CrmDropped`.
- Rust: `chat_push_tests.rs`:
  - `queue#ReminderPush` writes one row a device;
  - it is sent though the user read past the message, and `queue#ReadClear` leaves it;
  - its collapse key is not the room's;
  - it is held during do not disturb.
- Client, offline: `webroot/test/client/WebrootChat` (`whenChoices` in the device's zone: in 20 minutes, tomorrow at
  9:00, Monday from Friday, nothing in the past; `scheduledLine`; `reminderOf`; `dueHeld`; the `schedule` push;
  `heldDrafts` sealed, not plain, in `storage`, and opened again). The webroot
  client budget is 40.6 of 48 s.
- basic-comms client (10.9 of 12 s): at most one small `ShellSync` check on the bundled Chat: Send later in Send's menu,
  Remind me in the message menu. It is measured, and dropped if it does not fit.
- No acceptance spec.

**Docs.**
- [Comms](../docs/tools/Comms.md): scheduled send and reminders.
- [Mobile-Client](../docs/ui/Mobile-Client.md): the long press on Send.
- [Desktop-Client](../docs/ui/Desktop-Client.md): Send's arrow.
- [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): a scheduled message goes out through the normal path at its
  time.
- The BasicComms README.
- §7.2 rule 9.

**Done when.** The regression specs above pass within their budgets, and on a real phone the row of §18.4: a long press
on Send, and a reminder's notification on the phone.

**Review fixes (2026-11-18).** What the review changed in the entry above:
- *The due queue.* The server's due query takes only the rows it holds. Held rows have their own bounded queries: a
  revoked device fails its rows (D3's daily revoke of unused automatic devices among them), and a held row due more than
  `chat@held-overdue-days` (7) ago fails `deviceAway`. A device not seen for 7 days shows as "can't be sent from" it.
  At most `chat@schedule-user-per-run` (20) messages of one sender go in a run; the rest wait.
- *The sender at the send time* is `run#AsSender` (`crates/moqui-core/src/service/chat_schedule.rs`): the user set as
  `user::as_user` sets it, with none of a sign-in's checks, so a required or expired password change stops nothing. Only
  an account closed for good or terminated fails (`accountDisabled`). A sign-in lock (`disabledDateTime` within
  `login/@disable-minutes`) waits and is not counted. The screens of the request that made the row (`screenPathJson`,
  root first) are pushed again as the user: one who lost the app's grant fails `noAccess`, and a reminder drops. Reminders
  follow the same rules.
- *No plain text kept.* `body`, `bodyText` and `recordsJson` are cleared when a row is sent or cancelled. A failed row
  keeps them 7 days, then the daily cleanup removes it. A new text redoes the record cards.
- *The held send.* The device sends through `send#HeldMessage` (the `sendHeld` transition), which locks the row and
  refuses (`notClaimed`) unless it still waits, claimed by that device with its lease live. A message cancelled
  meanwhile does not go. One that went before a cancel or a revoke is marked sent by the next claim or report, and the
  list says "sent after you cancelled".
  - A verified person's new device or identity puts the row in review (`reviewNeeded`), which the list shows with
    "Review and send" (the composer's problems box).
  - Any other client failure is one try (`report#HeldFailure`), and the fifth fails it with the notice.
  - A failed held row is not sent again, and its draft is dropped. On each read of the schedule the device drops the
    drafts of rows it no longer holds waiting.
- *Time zones.* The choices and a picked time are in the user's zone (`moqui.l10n.zonedWall`, `wallMillis`), the one the
  labels use. Monday is Monday there.
- *Tests added:* the account rules, the app's screens, the five tries, 210 held rows before a server-held one, the
  per-run cap, the locked held send and a cancel before a stalled send, a message that went before a revoke
  (`ChatSchedule`, 12 features); `run#AsSender`'s account rule (`chat_schedule` Rust test); the user's zone, the drafts
  pruned, the real `sealLocal`/`openLocal` round trip and `sendHeld`'s paths (`WebrootChat`).

**Status (2026-11-18): built.**
- Regression: `ChatSchedule` (8 features, 0.2 s), `chat_push` (26 tests, the reminder's among them), `WebrootChat` (4
  new features; webroot client 0.2 s of its spec), the precompiled screens regenerated.
- The basic-comms suites, unchanged, measured 8.8 s of 9 (server) and 11.4 s of 12 (client). So the small `ShellSync`
  check was not added: the composer's Send later, the bar, the list, Remind me and Activity's reminders have no client
  walk.
- Left:
  - §18.4's row on a real phone;
  - Android's reminder body and its link to the message (the next APK change);
  - a held message's send from the phone app in the background.

#### Phase 16: disappearing messages and view-once media

Designed 2026-11-17 (§14.4, §17 #4); built 2026-11-18 (the status at the end of this entry). A room has a timer: its new
messages disappear that long after they are sent, on the server and on every member's device. A photo or video sent
view-once opens once for each person, then goes.

**The timer starts when a message is sent, for everyone.** Signal starts it at sending for the sender and at reading for
each recipient. That works because Signal's server keeps nothing. Here the server keeps the room's timeline for every
member and every device, so:
- one due time per message (`expireDate`) is one row in one index; read-start would need a due time per member;
- the server and all of a person's devices agree on when a message goes, with no per-device read state to sync;
- a message nobody reads still goes, which a read-start timer would keep on the server for ever;
- WhatsApp's disappearing messages also start at sending.

The cost: someone offline longer than a short timer never sees the message. The timer dialog says so ("New messages
disappear this long after they are sent, read or not"). A scheduled message (phase D2a) starts its timer when it is
actually sent.

**Data model** (`ChatEntities.xml`, `ChatKeyEntities.xml`)

| Entity, field | What |
|---|---|
| `ChatRoom.messageTimerSeconds` | The room's timer for new messages: 30 to 2,419,200 (4 weeks); empty means off. `find#Rooms`, `get#Room` and `get#Timeline` answer it |
| `ChatEvent.expireDate` (index `CHAT_EVENT_EXPIRE`) | When the event goes. A message gets `sentDate` plus the room's timer. A view-once message gets the earlier of that and `chat@view-once-days` (14). An edit, reaction, redaction or pin of a message gets the message's `expireDate`, so all of them are due together. Membership, call and system events never expire |
| `ChatEvent.viewOnce` | Y: a view-once message |
| `ChatEvent.megolmSessionId` (index `CHAT_EVENT_SESSION`: `chatRoomId`, `megolmSessionId`) | `content.encrypted.session_id` of an encrypted event, set by `append#Event`. It tells when the server's key backup of a session can go |
| `moqui.chat.ChatViewOnceOpen` (`chatRoomId`, `eventSeq`, `userId` pk; `openedDate`, `downloadCount`) | A recipient's open of a view-once message. Its rows go with the message |
| `moqui.chat.ChatEventDueRoom` (view: `chatRoomId`, `min(expireDate)`) | The rooms with events due, so a round takes rooms, not one room's rows |
| `moqui.chat.ChatExpirySession` (`chatRoomId`, `sessionId` pk; `lastUsedDate`, `dueDate`) | A session whose messages went while it may still be in use: its backup goes at `dueDate` |
| `ChatKeyBackupSession` index `CHAT_BACKUP_ROOM_SESSION` (`roomId`, `sessionId`) | The rows of a session across users' backups, found in one query |
| `ChatEventLink` index `CHAT_LINK_HASH` (`urlHash`) | Whether another message still links a URL whose preview cache row could go |
| `ChatSetupData.xml` | No new job: the worker below runs the deletes, and the daily `clean#ChatRetention` also runs `expire#DueMessages` as a backstop |
| `MoquiDefaultConf.xml` | `chat@view-once-days` (14), `chat@expiry-round-limit` (2000) |

**The exact delete.** A due-time index and a worker per node, with no per-message scheduling:
- `crates/moqui-core/src/service/chat_expiry.rs`, on the pattern of `chat_push.rs`: a thread per node, started beside
  the push worker (`moqui-host` `lib.rs`), stopped by the factory.
  - It sleeps until the earliest `expireDate`. That is one indexed query: `expireDate` not null, ordered, limit 1. It
    sleeps at most 60 s, so it also sees what other nodes wrote.
  - `wake#ChatExpiry` (type rust) wakes it after the commit, with the new event's due time, when that time is sooner
    than its planned wake.
  - Each round calls `ChatExpiryServices.expire#DueMessages`. A round that fails is logged, and the next one starts
    within 60 s.
  - The message is deleted within about a second of its time.
- `expire#DueMessages` (`nowMillis` for the tests; `limit`):
  - It reads the due rows by the index (`expireDate` up to now, up to the limit, only `chatRoomId`, `eventSeq` and
    `expireDate`) and groups them by room.
  - Each room is done by `expire#RoomMessages` in its own transaction. A room that fails is logged, and its rows are
    still due next round.
  - Out: `roomCount`, `deletedCount`, `failedCount`, `more`. When `more` is true, the worker runs again at once.
- Cost:
  - Each round reads only due rows. The next wake is one row of the index.
  - The writes are those of the deletes, plus one room change and one socket push per room per round.
  - A cluster's nodes may run the same round. The room row is locked first, so the second node finds nothing.
  - A busy room with a 30 s timer is a delete per message, which is the same order of work as its sends.
- `expire#RoomMessages` (internal; `ChatExpiry.js`), with the room row locked:
  - the due events, and any edit, reaction or redaction of them (as `clean#RoomRetention` finds them);
  - each message's content through `clear#MessageContent`: mentions, activity, links, files, poll, previews and search;
    plus its `ChatViewOnceOpen` rows, and each `ChatLinkPreview` whose `urlHash` no other link has;
  - the thread roots of the expired replies get their count and last reply back (as `redact#Message` does);
  - the room's last message is the last one that stands (as retention does);
  - in an encrypted room, the backed-up sessions: for each `megolmSessionId` of the deleted events that no event of the
    room still has, the `ChatKeyBackupSession` rows of `roomId` `!<chatRoomId>:<server>` and that session, in every
    user's backup, go;
  - the room's changes of the deleted events go, and one room change stands for them (below);
  - the counts are published to the members who had them unread;
  - a socket push `{kind: 'expired', chatRoomId, before, eventSeqList}` goes to the members (at most 200 seqs; `before`
    covers the rest).

**Sync.** The other devices are told through the room's feed, which costs one row per room, not one per member:
- `record#RoomChange` with `eventSeq` empty and `{cause: 'expired', before}`. It replaces the room's earlier marker, so a
  room has one, and `before` only grows.
  - The marker means every event of the room whose `expireDate` is at or before `before` is gone.
  - `before` is the latest due time the round finished, so it still holds when a round stopped at its limit.
- The room's `lastChangeSeq` moves, so `get#UserChanges` heads send the client to the room's feed (§5.2), as for an edit.
- A client that does not know the marker skips it: it has no `eventSeq`.
- A view-once open is a change of that message (`push#Update` with `viewOnceChanged`, a room change `update`): each
  client reads the message again, so the sender's devices read "Opened", and the opener's other devices drop their copy.
- A reset of the feed reads the room's latest page again and drops what is held, so nothing expired survives a reset.
- Clients also delete at the time by their own clock, corrected by the server's (below). The marker and the push cover
  a device that was asleep, or whose clock is wrong.

**Services** (`framework/service/org/moqui/chat/ChatExpiryServices.xml`, `ChatExpiry.js`; hooks in the shared files)

| Service | What |
|---|---|
| `set#RoomTimer` (`chatRoomId`, `seconds`; 0 or empty is off) | Who may set it: any member of a direct room; a moderator or owner of another room. Refused, with a `reasonCode`: `notAllowed`; `range` (not 30 s to 4 weeks); `recordRoom` (a record room keeps its history); `bridged` (a mapping in force, `ChatRoomBridge`, so phone rooms too); `federated` (`federationEnabled` Y); `archived`. It writes the field and appends a system event `{action: 'timer', seconds, byUserId}` ("Ana set disappearing messages to 1 hour", "… turned them off"), and the members' feeds get a `room` change. New messages only: what was sent keeps its time |
| `open#ViewOnce` (`chatRoomId`, `eventSeq`) | A current member who did not send it. The first call writes the open and answers the file (`chatAttachmentId`, `fileName`, `contentType`) with `until`, the end of a 2-minute window in which `get#ViewOnceFile` (the `viewOnceFile` transition, `no-store`) serves it at most 3 times (a download cut off midway). Refused: `opened` (after the window), `gone` (the file went), `sender`. The first open is a change of the message. Once every current local member but the sender has opened it, the message is due at the end of that window: `expire#RoomMessages` then removes only its file and sets it back to its own time (`content.viewOnceUntil`), so it stands as Opened |
| `expire#DueMessages`, `expire#RoomMessages` | Above |
| Hooks in `ChatServices.xml` | See the list below |

The hooks in `ChatServices.xml`:
- `append#Event`: one call, `ChatExpiry.js` `expiryOf`, for `expireDate`, `viewOnce` and `megolmSessionId`, then
  `wake#ChatExpiry`.
- `send#Message` takes `viewOnce`. Refused when the message:
  - has text, has more or other than one photo (png, jpeg, gif or webp) or one video (mp4, webm or quicktime), or names
    records (`viewOnceFiles`);
  - is in a bridged or federated room (`bridged`).
- Forwarding a message that expires, or a view-once message, is refused (`expiring`).
- `edit#Message` refuses a view-once message.
- `get#Attachment` never serves a view-once file (only `get#ViewOnceFile` does), and answers `expiring`, for the
  `attachment` transition's `Cache-Control: no-store`.
- `get#Timeline`:
  - each event has `expireDate` (millis);
  - a view-once message has no attachments and has `viewOnce` `{kind: 'photo' | 'video', openedByMe, gone}`; for its
    sender it also has `openedCount` and `recipientCount`, which leave out members whose `CHAT_READ_RECEIPTS` is N;
  - a reply to a message that went has `replyTo` `{gone: true}`;
  - the answer has `serverNow`.
- `index#ChatEvent` indexes no event that expires: nothing reaches OpenSearch. Without OpenSearch, the database search
  finds a message while it stands.
- `deliver#AgentMessage` hands an expiring message to no agent: an agent's model and its logs (`clean#LlmCallContent`,
  14 days) would keep it.
- `notify#Mention` gives an expiring message's notification no summary.

Other hooks:
- `ChatBridgeServices.store#RoomBridge` is refused while the room has a timer (`timerOn`). No service sets
  `federationEnabled` today; one that does later gets the same check.
- `XmppServices.get#RoomInfo` answers `disappearing` for a room with a timer; the XMPP session refuses that room as it
  refuses an encrypted one (`crates/moqui-xmpp/src/session/chat.rs`).
- `XmppServices.get#AttachmentUrls` makes no XMPP URL for a view-once file. The XMPP session sends a view-once message as the text
  "View-once photo: open it in MR Moqui".
- `chat_push.rs` puts no snippet in the push of an expiring message, as for an encrypted one, and its `PushOutbox`
  `expireDate` is the earlier of the 4 weeks and the message's own `expireDate`.

**Bridges: refused, and said.** A bridge cannot delete on the other network, and an SMS cannot be taken back. A room
that is bridged, a phone room or a federated room therefore gets no timer and no view-once:
- The timer dialog shows the choice disabled, with "This room is bridged to <networks>: a message that went there cannot
  be deleted there."
- The attach flow shows no view-once toggle.
- The other way round: a room with a timer cannot be bridged or federated until the timer is off. The bridge form says
  so.

The other choice was a warning on the room. It was not taken: the timer would then promise less than it says.

**Encrypted rooms.** The clients delete their decrypted copy; the server deletes the ciphertext and the key material it
holds:
- The ciphertext and the files' blobs go as any message's do.
- The key backup: a backed-up Megolm session goes once no event of the room uses it (above).
- To-device room keys are deleted when acknowledged, and after `chat@to-device-retention-days` as today.
- So that a session does not outlive its messages, the sender's browser in a room with a timer rotates its outbound
  session: `EncryptionSettings.rotationPeriod` is the timer (at most a week), with `rotationPeriodMessages` 100. A timer
  change invalidates the session (`WebrootCrypto.js` `encryptRoomNow`).
- The device's own store keeps the inbound session key, because matrix-sdk-crypto has no delete for one. With the
  ciphertext gone from the server and the device, it opens nothing.
- A view-once file in an encrypted room is decrypted into a blob in memory only, and the blob is revoked when the viewer
  closes.

**Where a deleted message could stay, and how each is cleared**

| Place | Cleared by |
|---|---|
| `ChatEvent` and its edits, reactions, redactions and pins | `expire#RoomMessages` |
| Mentions, Activity (`ChatUserActivity`), links, polls, view-once opens | `clear#MessageContent` and `expire#RoomMessages` |
| Files: the `DbResource` bytes, `ChatAttachment`, the XMPP upload rows | `remove#AttachmentFile`; one that fails is logged |
| Link previews: `linkPreviewJson`, and the shared `ChatLinkPreview` cache row | The row goes with the message; the cache row goes when no other link has its hash |
| Search | Never put in OpenSearch's index. Without OpenSearch, the database search reads the rows, so it finds a message while it stands and not after |
| The room list's last message (`ChatRoom.lastMessage*`) | The last message that stands |
| Threads: a root's count and last reply | Set back, as for a redaction; a root that goes leaves its replies, each with its own time |
| Replies | The quote reads `replyTo {gone}`: "This message disappeared". Nothing of it is copied into the reply |
| Push payloads (`PushOutbox`) and stored mention notifications | No text in them. A push is not sent after the message's time |
| The changes feeds (`ChatRoomChange`) | Its changes go; the marker stands for them |
| The web page (Vue state; `moqui.chat`'s `opened`, `sealedTexts`, `reactionKeys`, and `fileUrls` revoked) | `moqui.chat.expiry` at the time; the `expired` push |
| The offline store, desktop and Android (`moqui-client` SQLite) | `chat_event.expire_ms`, with an index (in the store's schema as is: no migrations before 0.1.0; a store of an earlier `SCHEMA_REV`, kept in `PRAGMA application_id`, is made again when it opens, its outbox, drafts and uploads copied across) swept by `Store::expire_chat_events(now)`, by the server's clock, at each feed run, push fold and offline chat answer; an expired row is never answered. The room's kept `/timeline`, `/roomView` and `/room` answers and the kept room lists go, and a kept page holding an expired event is written again without it. The store sets `PRAGMA secure_delete`, so deleted rows do not stay in free pages |
| The WebView's HTTP cache | The `attachment` transition sends `Cache-Control: no-store` for a file of an expiring or view-once message |
| The browser outbox (`localStorage`) | A waiting message is held only until it is sent, as today; its timer starts then |
| Agents and LLM logs | Not handed |
| Bridges, XMPP, federation | Refused, or not served |
| Database backups | An archive made while the message stood keeps it until that archive ages out. A restored archive's past-due messages go at the worker's first round, because `expireDate` is in the row. An encrypted room's archive holds only ciphertext. The timer dialog says this for a room that is not encrypted |
| Exports | Chat has no export. An administrator's entity export or data transfer holds what stood at that moment (said in [Comms](../docs/tools/Comms.md)) |

**Clients** (`webroot/screen/webroot/js/WebrootChatExpiry.js`, new, loaded after `WebrootChat.js` in `qapps.xml` and the
desktop shell's `index.html`: `moqui.chat.expiry`)
- Pure parts:
  - `options(t)`: Off, 30 seconds, 5 minutes, 1 hour, 8 hours, 1 day, 1 week, 4 weeks;
  - `label(seconds)`;
  - `left(expireDate, now)`: "4m", "2h", "3d";
  - `learn(serverNow)` and `now()`: the server's clock, from `get#Timeline` and each pushed event's `sentDate`;
  - `due(events, now)`, `nextDue(events, now)`.
- `track(roomId, events)` and one page timer to the next due time. At that time it emits
  `{kind: 'expired', chatRoomId, eventSeqList}` on `moqui.chat.on`, the shape the server pushes. So every list a room
  keeps drops the event, the same way for the server's push and the page's own timer: the timeline, the thread pane,
  the details tabs, pinned, search results, the Activity and Threads lists.
- `moqui.chat.forgetEvents(roomId, seqs)` (a hook in `WebrootChat.js`) drops the decrypted copies and revokes their
  blob URLs.
- `secureScreen(reason, on)`: the app host's `moquiDesktop.secureScreen`, nothing on the web. See view-once.
- A notification of an expiring message, the page's own or the desktop app's, shows the sender and the room only.

**Screens** (`Chat.qvue`, hooks only where it can; `Chat.xml` transitions `setRoomTimer` and `openViewOnce`)
- Room settings:
  - a "Disappearing messages" row, with its value and Change, opens the timer dialog (`m-dialog`, 420 px; full screen
    under 600 px): the choices as a radio list, and the notes;
  - the notes say that messages disappear this long after they are sent, read or not, on every device; that new
    messages only are affected; that a member can still copy or photograph a message; the bridge note; and the
    backup note in a plain room;
  - a member who may not change it sees the value only.
- The header: a timer icon with the short value beside the room's name, which opens the dialog. The rail: the same icon
  on the room's row.
- The timeline:
  - an expiring message has a small timer icon by its time, with its time left in the tooltip and in Info;
  - the system event says who set the timer, and to what;
  - an expired message leaves no trace in the timeline.
- Attach:
  - with exactly one photo or video waiting and no text, its chip has a "View once" toggle (`sym_o_looks_one`), on the
    phone and on the desktop;
  - once on, the text box says "A view-once photo has no text" and is disabled;
  - the toggle does not show in a bridged room.
- A view-once message:
  - its bubble shows "Photo" or "Video" with the view-once icon, and no preview;
  - the recipient taps it to open it in a full-screen viewer: no download, no forward, no save, no context menu, a
    video played once;
  - on close, the blob is revoked, and the bubble reads "Opened" (`openedByMe`);
  - the sender's bubble reads "Sent", then "Opened", or "Opened by 2 of 4" in a group (Info names them);
  - `gone` reads "Expired".
- Screenshots: while the viewer is open, the Android app sets `FLAG_SECURE` through `moquiDesktop.secureScreen('viewOnce',
  true)`:
  - a command in `crates/moqui-mobile/src/commands.rs`, then `android.rs`, then `MainActivity.setSecure(reason, on)`;
  - `MainActivity` holds a set of reasons, and the flag is on while any reason is;
  - D2g adds its reasons (`app`, `room:<id>`) to the same set.
  The desktop app and the web cannot stop a screenshot. Their viewer says so in a line.
- Phone and desktop: the same components. On a phone, the dialog and the viewer are full screen and the toggle is on
  the attach tray's chip. On a desktop, the dialog is 420 px and the viewer is a dark full-window overlay; Esc closes it.
- Terminal (`ChatTui.xml`):
  - an expiring message's line ends "disappears";
  - a view-once message reads "View-once photo: open it in the app";
  - the terminal does not change the timer, and it keeps nothing.

**Failure tolerance**
- The rooms of a round are each on their own. A file that cannot be removed is logged, and its message still goes.
- A round that fails is logged, and the next one starts within 60 s.
- A process with no worker is covered: the daily `clean#ChatRetention` runs `expire#DueMessages` too.
- The client's sweeps are each event on their own. A view-once download cut off is tried again within its window.

**Security and abuse**
- The timer does not protect against a member who copies, photographs or keeps a message with a changed client. The
  dialog says so.
- A plain room's messages are readable by the server's administrators until they go; an encrypted room's never are.
- A timer change is an event in the room, so nobody can turn it off unseen to keep what follows.
- `set#RoomTimer` checks rights as `update#Room` does. `open#ViewOnce` and `get#Attachment` are for current members
  only; the sender is refused the view-once file; the window and the count bound the downloads.
- The 30 s floor, the existing send limits and the round's limit bound the work, so no user can make the worker heavy.
- A record room is refused: a record's history is the business's.

**Left out**
- A timer that starts when a message is read.
- A timer on one message.
- A user's default timer for new chats.
- Custom durations in the dialog (the server takes any value from 30 s to 4 weeks).
- Deleting on another network.
- Clearing a phone notification already shown, at the message's time (it has no text).
- Stopping screenshots on the desktop and the web, and detecting them.
- Scrubbing database backups.
- Deleting the device's inbound Megolm session.
- View-once for files other than photos and videos, and for voice notes.

**Tests (regression)**
- Server:
  - `framework/test/ChatExpiry.spec.js` (new):
    - `set#RoomTimer`: a direct room's member may; a group's member may not and its moderator may; `range`,
      `recordRoom`, `bridged`, `federated`; the system event;
    - `store#RoomBridge` refused while the timer is on;
    - a message's `expireDate`, and the same on its edit and reaction; none on a membership event;
    - `expire#DueMessages` with `nowMillis` (the job called directly): the message, its edit and reaction, file, links,
      preview cache row (kept while another link has it), activity and mentions go; a thread root's count and the
      room's last message are set back; the marker is there and the message's own changes are gone; a message not yet
      due stays;
    - an encrypted room: a backup session goes with its last event and stays while one is left;
    - a forward of an expiring message is refused; an expiring message is not handed to an agent.
  - `framework/test/ChatViewOnce.spec.js` (new):
    - `send#Message` `viewOnce` rules: text, two files and a PDF refused, and a bridged room refused;
    - `get#Timeline` gives no attachments;
    - `open#ViewOnce` serves the file in the window and is refused after it and for the sender; `get#Attachment` is
      refused outside the window;
    - the sender's `openedCount`; the file goes once all have opened; the 14-day deadline through `expire#DueMessages`.
- Rust:
  - `moqui-core` `chat_expiry_tests.rs`: the wait from the next due time and a wake;
  - `chat_push_tests.rs`: an expiring message's push has no snippet, and its time to live ends with it;
  - `moqui-client` store tests: `expire_chat_events`, `secure_delete`; the feeds tests: the marker
    folded; the push tests: `expired` folded;
  - `moqui-xmpp` session tests: a room with a timer refused.
- Client, offline: `webroot/test/client/WebrootChat.spec.js`, a feature for `moqui.chat.expiry`:
  - the options and labels, `left`, `learn` and `now` with a clock 10 minutes off, `due` and `nextDue`;
  - the timer's emit removes an event from a room's list;
  - `forgetEvents` revokes the blob URLs.
  The webroot offline budget, 40.6 of 48 s, is measured after.
- `basic-comms` client: nothing. Its budget is 10.9 of 12 s; the dialog and the viewer are checked by hand.
- No acceptance spec.

**Docs**
- [Comms](../docs/tools/Comms.md): the timer, view-once, the list of places, backups and exports.
- [Chat keys](../docs/interfaces/Chat-Keys.md): the backup sessions deleted, and the rotation.
- [Mobile-Client](../docs/ui/Mobile-Client.md) and [Desktop-Client](../docs/ui/Desktop-Client.md): `secureScreen`,
  the store's sweep and `secure_delete`.
- [XMPP](../docs/interfaces/XMPP.md): a room with a timer is not served.
- The headers of `ChatServices.xml` (beside Retention) and `ChatEntities.xml`.
- The `moqui-client` README (store) and the BasicComms README.

**Done when.** The regression specs above pass within budget. On a real phone, the row of §18.4.

Status (2026-11-18): built. The specs above pass: `ChatExpiry` and `ChatViewOnce` (0.3 s of framework's 21 s), the
`WebrootChat` feature (the webroot offline suite measured 44.3 s of its 48 s), the Rust tests of `moqui-core`
(`chat_expiry_tests`, `chat_push_tests`), `moqui-client` and `moqui-xmpp`; the basic-comms suites unchanged within
budget (server 3.4 s of 9 s, client 10.6 s of 12 s). The `moqui-client` store's `expire_ms` column is in its schema as is
(no migrations before 0.1.0): a store of an earlier schema revision is made again when it opens. Left: §18.4's row on a real
phone, and the Android `ScreenSecurity` path is checked on the phone only.

Review fixes (2026-11-18):
- XMPP history (`get#Archive`, the MAM loop) has no disappearing message in any room, and none of a room not served.
- The Android command `secure_screen` is in `build.rs` and the capability (a test reads the three lists); the viewer's
  note goes by what `secureScreen` answers.
- The client store: `SCHEMA_REV` in `PRAGMA application_id`; an earlier store is made again with its outbox and drafts.
  It deletes by the server's clock (`serverNow` of the timeline and the feed), writes kept pages again without expired
  events, drops the kept room, room view and room-list answers with them, and folds a view-once change.
- The worker: rooms by `ChatEventDueRoom` (grouped by room); a failing room is skipped 5 minutes on its node
  (`chat.expiry.failed`); a round stuck on it waits by a backoff (5 s doubled, at most the sweep).
- Readers leave out what is past its time and not yet deleted: `get#Timeline`, `get#Attachment`, `open#ViewOnce`, the
  database search, `find#RoomFiles`, `get#MessageSummaries`, `find#RoomThreads` and the room list's last message.
- `createFrom` and `fileOnRecord` refuse a disappearing or view-once message (`expiring`). The agent's thread context
  (`moqui-llm` `thread_context`) leaves them out.
- A session's key backup goes only once its newest use is past the rotation period and 10 minutes
  (`ChatExpirySession`); every page that shows the room rotates on the timer's system event.
- `open#ViewOnce` locks the message row; Opened needs read receipts on on both sides, and no change is pushed to the
  sender otherwise; a view-once file is in no list and no summary.
- A merged push keeps the longest time to live. The page's clock takes the largest of its last offsets, from fresh
  answers only. `checkBridge` locks the room row; no service sets `federationEnabled` (said in `ChatExpiry.js`). The
  timer's words go through `t()`, units as keys.

#### Phase 17: invite links and QR codes, and join requests (D2c, §14.4, §14.5, §17 #5)

Designed 2026-11-17; built 2026-11-18 (the status at the end of this entry). A room's moderator makes a link (and its QR code) that lets people with an account into the room,
straight in or through the moderators' approval. A private room its owner lists in Browse takes join requests too.

**Which rooms take links.**

| Room | Links | Who makes and revokes them | Approval |
|---|---|---|---|
| Private (`CrtPrivate`) | Yes | Moderators and owners; the maker revokes their own | Optional, per link |
| Public (`CrtPublic`) | Yes, as a shortcut: any user may join from Browse already | Any member | Never: the room is open |
| Direct, record, phone (`CrtExternal`) | No: a direct room's members are fixed, a record room's readers are the record's, a phone room's are its number's users | — | — |

A link works only while:
- the room is not archived;
- its maker may still make one (a moderator of the private room, a member of the public room);
- it is not revoked, not past its expiry and not used up.

**People without an account.** The join page asks them to sign in, and nothing more. There are no chat guests:
- the call guests (`CallGuest`, `Calls.js guestJoin`) are a credential for one call's signaling for 4 hours;
- a chat member stays, reads history and writes, which needs an account, its rights and its devices;
- an encrypted room needs a crypto device;
- the base framework has no self sign-up.

A person with no account is added by text (phase 18), or an administrator makes an account. The page says: "You need
an account on this server to join. Ask the person who shared the link."

**Data model** (`ChatEntities.xml`)

| Entity, field | What |
|---|---|
| `moqui.chat.ChatInviteLink` (`chatInviteLinkId` pk; `chatRoomId`; `tokenHash`, unique index; `tokenText`, `encrypt="true"`; `label`; `createdByUserId`, `createdDate`; `expiryDate`, empty for never; `maxUses`, empty for no limit; `useCount`; `approvalRequired`; `revokedDate`, `revokedByUserId`; `lastUsedDate`; index `chatRoomId`, `revokedDate`) | A room's link. The token is found by its hash (`ec.user.singleUseCodeHash`, as `CallLink` does). Its text is kept encrypted, so a moderator can copy the link and show its QR again, which the call links cannot (they are one person's, shown once) |
| `moqui.chat.ChatJoinRequest` (`chatJoinRequestId` pk; `chatRoomId`; `userId`; `chatInviteLinkId`, empty from Browse; `statusEnumId`; `note`, at most 200 characters; `requestedDate`; `decidedDate`, `decidedByUserId`; indexes `chatRoomId`+`statusEnumId`, `userId`+`statusEnumId`) | A person's request to join. `ChatJoinRequestStatus`: `CjrPending`, `CjrApproved`, `CjrDenied`, `CjrCancelled`, `CjrExpired`. A pending request older than 30 days reads as expired, and is written so when next touched (no job) |
| `ChatRoom.directoryListed` | Y: a private room listed in Browse, by its name, topic and member count, with "Ask to join". Its messages stay its members' |
| `ChatRoomMember.removedByUserId` | Who took the member out (`remove#Member` by another member). Such a person joins by a link only through a request, whatever the link says |
| `ChatSetupData.xml` | The notification topics `ChatJoinRequest` (to the moderators) and `ChatJoinDecision` (to the person who asked) |

**Links.** The link is `https://<host>/join/<token>`:
- `<token>` is `ec.user.randomString(32)`, about 190 bits;
- the token route `join` of basic-comms (`MoquiConf.xml`, beside `call`) renders the join page in the public shell after
  `check#InviteToken`;
- the QR code is that URL, drawn by `ec.resource.barcodeSvg('QR', url)` on the server, so no QR library is added.

The link's choices:
- expiry: 1 hour, 1 day, 7 days (the default), 30 days, or never;
- uses: no limit (the default), 1, 5, 10, 25 or 100;
- approval: on or off, in a private room only;
- a label, such as "Newsletter".

A use is a join or a request. So a link with approval that allows 10 uses lets at most 10 people into the queue.

**Joining** (`join#ByInvite`, the join page's `join` transition, a POST with the session token):
1. Signed out: the page shows the room's name and "Sign in to join". Its `signIn` transition keeps `/join/<token>` as
   the return path in the session (`ec.web.saveScreenLastInfo`, which Login's `returnTo` uses) and goes to `/Login`.
   The token never goes in a query string.
2. Already a member: "Open the room".
3. Otherwise, under the link row's lock (so its last use is taken once), the link is checked, and then:
   - with approval, or when the person was taken out of the room by another member: a request (`CjrPending`, with
     their note), counted as a use;
   - else they join: a member row, as `add#Member` makes one, and the membership event
     `{action: 'join', userId, via: 'link', chatInviteLinkId}`, shown as "Ann joined by an invite link";
   - a person who left by themselves comes back as `add#Member` brings them back.
4. Out: `outcome` (`joined`, `requested` or `member`), `chatRoomId`, `roomUrl` (`${comms@app-root}/Chat?chatRoomId=`).
   Refused with a `reasonCode`:
   - `notFound`: a bad, revoked, expired or used-up link, or one whose maker is disabled; a person the maker could not
     add to the room: one the maker blocked (D1), or one `get#RefusedUsers` asked as the maker refuses (the apps'
     chat-target providers, as `add#Member` asks them; answered as not found, SEC-COM-01, on the join page too); an
     external user (`EXTERNAL_USER`) and a public room;
   - `notSignedIn`;
   - `deniedRecently`: a request to this room denied in the last 7 days;
   - `tooManyRequests`: 10 pending a person;
   - `queueFull`: 100 pending a room;
   - `rateLimited`: 30 joins and requests an hour a person, counted on the distributed cache as `find#PhoneNames` does.

The join itself is in `ChatInvites.js`, not in `add#Member`. `add#Member` lets a member add others, and lets a person
join only a room they can read. A private room's joiner is neither, and an internal parameter on a remote service would
be a way round its checks. An approval does go through `add#Member`, called as the approving moderator, with all of its
checks (`get#RefusedUsers` among them).

**Join requests.**
- From a link with approval, or from Browse ("Ask to join" on a listed private room, `request#Join`).
- The moderators and owners hear of a new one by the `ChatJoinRequest` notification (the bell, and the socket). A
  moderator who blocked the person (D1) gets no notification and does not see the request. The room's moderators get
  `{kind: 'joinRequests', chatRoomId, pendingCount}` on the room's topic, which moves the room's banner.
- The queue (`find#JoinRequests`, moderators and owners) shows each request's person, note, time, and the link's label
  (or "From Browse"), and "Was taken out of this room" when it applies.
- Approve (`decide#JoinRequest`, one or a list) adds the person through `add#Member` as the moderator. Deny marks the
  request. Each request in a list is done on its own. The person gets the `ChatJoinDecision` notification, and an approval
  brings the room into their list as any add does.
- The person may cancel their own request (`cancel#JoinRequest`). A revoked link's pending requests stay for the
  moderators to decide, as Signal keeps them when a group's link is reset.

**Services** (`framework/service/org/moqui/chat/ChatInviteServices.xml`, `ChatInvites.js`)

| Service | What |
|---|---|
| `create#InviteLink` (`chatRoomId`, `label`, `expiryHours` 1 to 720, 0 for never; `maxUses`; `approvalRequired`) | Out: `chatInviteLinkId`, `token`, `path`. Refused: `notAllowed` (not a moderator of a private room, or not a member of a public room; an external user); `roomType` (direct, record, phone); `archived`; `tooManyLinks` (20 in force a room) |
| `find#InviteLinks` (`chatRoomId`) | The links in force: a moderator sees all, a member of a public room their own. Each with its path, label, uses, expiry, approval, maker, last use |
| `get#InviteQr` (`chatInviteLinkId`, `baseUrl`) | The QR code (`qrSvg`) of the link's URL. The transition passes `baseUrl` from the request's own scheme and host |
| `revoke#InviteLink` (`chatInviteLinkId`) | Its maker, or a moderator or owner of the room. It takes no one from then on |
| `check#InviteToken` (anonymous; the token route's service) | `valid` and `routeContext`: the room's name, member count, room type, approval, encrypted. With a session user also: `signedIn`, `member`, `pending` (their open request), `makerName`. Nothing is read for a bad token |
| `join#ByInvite` (`token`, `note`) | Above |
| `request#Join` (`chatRoomId`, `note`) | A listed private room from Browse; the same refusals as a request by link |
| `cancel#JoinRequest`, `find#JoinRequests`, `decide#JoinRequest` (`chatJoinRequestId` or `chatJoinRequestIdList`, `decision` approve or deny) | Above |
| `find#ListedRooms` (`searchText`) | The listed private rooms the user is not in, for Browse (none for an external user), with `pending` |
| `set#RoomListed` (`chatRoomId`, `listed`) | An owner of a private room |

Hooks in shared files:
- `ChatServices.remove#Member` sets `removedByUserId` when another member takes someone out (one line).
- `Chat.qvue` shows the membership line "joined by an invite link".

**Screens**

| Work | Files |
|---|---|
| The join page: the room's initial, name and member count; "Sign in to join", "Join", "Ask to join" with a note, or "Open the room"; the encrypted note; "Open in the MR Moqui app" on Android | `basic-comms/screen/ChatJoin.xml`, `ChatJoin.qvue` (new); the token route `join` and the rate limits `/join/*/join` and `/join/*/signIn` (10 a minute) in `basic-comms/MoquiConf.xml` |
| Invite links dialog (`m-chat-invites`): the links with Copy, QR, Share (`navigator.share` where there is one), Revoke; New link (label, expiry, uses, approval); the owner's "List in Browse" switch in a private room. The QR view: the code at 240 px, the URL, Copy and Download (SVG) | `webroot/screen/webroot/js/WebrootChatInvites.js` (new: `moqui.chat.invites`, `m-chat-invites`, `m-chat-join-requests`), loaded after `WebrootChat.js` in `qapps.xml` and the desktop shell's `index.html` |
| Join requests (`m-chat-join-requests`): each request with Approve and Deny, and Approve all | The same file |
| Hooks: the room menu's "Invite links" and "Join requests (n)"; a moderator's banner "2 people ask to join · Review" above the timeline; Browse's second list, "Private rooms you can ask to join", with "Ask to join"; the membership line | `Chat.qvue` |
| Transitions `inviteLinks`, `createInviteLink`, `revokeInviteLink`, `inviteQr`, `joinRequests`, `decideJoinRequest`, `cancelJoinRequest`, `listedRooms`, `requestJoin`, `setRoomListed` | `basic-comms/template/ChatInviteTransitions.xml` (new), one `transition-include` each in `Chat.xml` |
| The terminal: the membership line. No links or requests: those are the browser's | `ChatTui.xml` |
| Bundled screens regenerated | `bundled-screens.js` |

**On a phone and on a desktop.**
- Desktop: the dialog shows the QR code large, for a phone's camera to scan, beside Copy.
- Phone: the dialog is full screen under 600 px; Share opens the system's share sheet with the link, and the QR code
  shows for a person standing beside.
- A link scanned or tapped on a phone opens the join page in the browser. On Android the page offers "Open in the
  MR Moqui app": `org.moqui.mobile:/join?host=<host>&token=<token>`.
  - A new `BROWSABLE` intent filter on the path `/join` (`AndroidManifest.xml`) takes it.
  - The app opens `https://<host>/join/<token>` in its window of the account on that host, where the person presses
    Join. With several accounts there, the person picks one.
  - With no account there, it says so and adds none. Nothing the link brings is done by itself (as with the dial link,
    phase 9).
  - The link is not signed, unlike the room link, because a web page makes it. It only opens a page of a server the
    app already has an account on.
  - Files: `crates/moqui-mobile/src/join.rs` (new: `join_link_of`, `is_join_link`), `src/lib.rs` (the
    `RunEvent::Opened` branch).
- Android App Links (an https link opening the app straight away) are left out: each server has its own host, and the
  app cannot list them all.
- The desktop app's bundled Chat has the same dialogs; offline they say "needs a connection".

**Bridges and encryption.**
- A link changes who is a member, so the room's bridges are unchanged. A bridged room's link admits local users only.
- Encrypted room: a person who joins reads messages sent after they join, on a device set up for encrypted chat. Chat
  makes the browser's device when it opens (§19.1, 2026-11-17). The join page says both. In a room with verified devices
  only, their new device gets the room's keys once a member verifies it; the page says that too.

**Failure tolerance.**
- The moderators' notifications go one at a time; one that fails is logged.
- An "Approve all" decides each request on its own, and answers the ones that failed.
- A QR code that cannot be drawn leaves the link as it is, and the dialog shows "No QR code" with the link.
- Not being able to tell whether a moderator blocked the person (`blockersOf`) shows the request to every moderator,
  as D1 treats a failed block lookup.

**Security and abuse.**
- The token: 32 random characters, found by its hash, its text encrypted at rest. A bad token gets the token route's 404
  page, with nothing read. The server records the path as `/join/:token`.
- What the page shows: the room's name, member count and kind. A signed-in person also sees the maker's name. No member
  names, topic or messages.
- Limits:
  - 20 links in force a room;
  - 100 pending requests a room, 10 pending a person;
  - 30 joins and requests an hour a person;
  - 10 POSTs a minute on the route (`web-protection`);
  - 7 days after a deny before the person may ask again;
  - the use limit, taken under the link row's lock.
- Blocks (D1):
  - the maker's block refuses the blocked person, as not found;
  - a moderator who blocked the person does not see their request, and other moderators do;
  - the joiner's own blocks work as in any room.
- A person a member took out of the room comes back only by approval.
- External users: no links made by them, and no public room by a link. A private room by a link is the same reach as a
  member adding them.
- A link stops when its maker is no longer a moderator, so a moderator who was taken out leaves no open door.
- Every service checks the caller's access in the room (`get#RoomAccess`). Only `check#InviteToken` and
  `join#ByInvite` take a token instead of a room.

**Left out.**
- Chat guests without an account, and sign-up from a link.
- Links in direct, record and phone rooms.
- An in-app QR scanner: the phone's camera app opens the link.
- A push to the moderators' phones for a request: the bell and the room's banner only.
- Bans, and Android App Links.
- Joining by text from the join page: a stranger typing any number would make the server text it (cost, toll fraud).
  A moderator adds a number instead (phase 18).

**Tests (regression).**
- `framework/test/ChatInvites.spec.js` (new):
  - who makes a link in each room type, and the refusals;
  - the token check: valid, expired, revoked, used up, maker no longer a moderator, room archived;
  - a straight join and its event;
  - approval: a request, then approve (through `add#Member`) and deny, and `deniedRecently`;
  - a request counts as a use;
  - a person taken out of the room gets a request;
  - the maker's block refuses as not found, and a blocking moderator does not see the request;
  - an external user and a public room;
  - the per-person and per-room limits;
  - a listed room in `find#ListedRooms` and its request;
  - revoke.
- `basic-comms/test/ChatJoin.spec.js` (new): the route's 404 for a bad token; the join page rendered once (qvt) with its
  route context; the `join` transition; `signIn`'s return path.
- `basic-comms/test/BasicCommsTexts.spec.js`: none (phase 18's transitions only).
- `webroot/test/client/WebrootChat.spec.js` (offline, no server): `moqui.chat.invites` expiry and uses texts, a link's
  state (expired, used up), the membership line for `via: 'link'`.
- moqui-mobile `join_tests.rs`: `join_link_of` takes a host and a token, and refuses another path, scheme, a bad host
  or token.
- No basic-comms client spec (its budget is near 12 s) and no acceptance spec. The dialogs and the join page in a phone
  browser are §18.4 checks.

**Docs.**
- [Comms](../docs/tools/Comms.md): Invite links and QR codes, Join requests, List in Browse.
- [Mobile-Client](../docs/ui/Mobile-Client.md): the join link.
- [Desktop-Client](../docs/ui/Desktop-Client.md): the dialogs in the bundled Chat.
- The BasicComms README: the `/join/<token>` route.

**Status (2026-11-18): built** as designed, with these changes:
- The client parts of both phases are one file, `webroot/screen/webroot/js/WebrootChatJoin.js` (`moqui.chat.invites`,
  `moqui.chat.texts`, `m-chat-invites`, `m-chat-join-requests`, `m-chat-text-members`), loaded after `WebrootChat.js` in
  `qapps.xml` and the desktop shell's `index.html`; its render functions are in `WebrootVue.qvt.render.js`
  (`client_components_are_precompiled` reads it).
- A join link with several accounts on its host opens in the current one when it is there, else the first; there is no
  pick dialog.
- The QR code is drawn for the conf's own address (`ec.web.getWebappRootUrl`), never the request's `Host`.
- `WebrootChat.js` passes the room's `joinRequests` push through the chat store (one line), and the room listens for it.
- Files: `ChatInvites.js`, `ChatInviteServices.xml` (new); `ChatEntities.xml` (`ChatInviteLink`, `ChatJoinRequest`,
  `ChatRoom.directoryListed`, `ChatRoomMember.removedByUserId`), `ChatSetupData.xml` (the two topics),
  `ChatServices.xml` (`remove#Member`'s `removedByUserId`); `basic-comms/screen/ChatJoin.xml`, `ChatJoin.qvue`,
  `template/ChatInviteTransitions.xml` (new), `MoquiConf.xml` (the route and its rate limits), `Chat.xml` (the
  includes), `Chat.qvue` (the menu items, the banner, Browse's listed rooms, the membership lines); moqui-mobile
  `src/join.rs` (new), `src/lib.rs`, `src/host.rs` (`open_join_link`), the manifest's `/join` filter.
- Tests. Regression: `framework/test/ChatInvites.spec.js` (new: the rules above), `basic-comms/test/ChatJoin.spec.js`
  (new: the page in qvue, its `join` and `signIn`, and Chat's invite transitions), `webroot/test/client/WebrootChat.spec.js`
  (the pure parts, the push, the components compiling), moqui-mobile `join_tests.rs` (the link, the account's pick).
- Left: §18.4's row; the dialogs and the join page are not walked in a browser.

**Review fixes (2026-11-18).**
- A link takes only people its maker could add: `refusedBy` runs the maker's block and `get#RefusedUsers` asked as the
  maker (`ChatInviteServices.get#RefusedUsersAs`, Rust `chat::get_refused_users_as`: `get#RefusedUsers` as the maker
  with `answerWithAuthzOff`, the apps' chat-target providers asked with authorization on, as `call#Providers` requires;
  everyone refused when it fails). The join page (`check#InviteToken`) answers such a person, and an external user on a
  public room's link, as a bad token; `join#ByInvite` answers "not found". An approval is `add#Member` as the approving
  moderator, so its refusal leaves the request pending.
- A disabled or terminated maker's links take no one (`mayMake`).
- Each moderator's pushed count is their own (`visibleCount`), without the requests of people they blocked.
- `findListed` counts the members of the rooms shown in one query.
- Tests: `ChatInvites.spec.js` (the page and a join refused alike for a blocked person, an expired link, an archived
  room, a disabled maker; `tooManyRequests`, `queueFull`, `rateLimited`), `ChatJoin.spec.js` (a refusing chat-target
  provider of the component's test app, `TextNamesServices.get#ChatTarget`: the page, the join and an approval).

#### Phase 18: text members of a room (D2d, §14.5 "Join by SMS without the app", §17 #6)

Designed 2026-11-17; built 2026-11-18 (the status at the end of this entry). A moderator adds a phone number to a private room. That number gets each message of the room as a
text, "Ann: …", from one of our numbers, and its replies post to the room as that number, as GroupMe's SMS members do.

**Routing: one room per outside number on each of our numbers.** A text a person sends to our number carries no room.
GroupMe gave each group its own reply number. Here:
- A number of ours carries room texts when an administrator marks it (`PhoneNumber.roomTextsEnabled`, System, Voice,
  Numbers).
- An outside number is a text member of at most one room on each such number. While it is one, all of its texts to that
  number go to that room.
- With one marked number, a person is in one room by text. With three, three.
- A fourth room on three numbers is refused: `noFreeNumber`. The add dialog explains the limit where it applies, for
  each number the adder may use: "This person is already a text member of <room> on this number; another number is
  needed."
- Decided by the user (§19.1, 2026-11-17).

The other choices were not taken:
- *The room the person was last texted from*: a reply to an older message goes to the wrong room, and nobody can tell.
- *A short room code in each reply* ("#2 see you"): people forget it, and a reply without it has no room.

A code could be added later, for a number shared by many rooms, if the user asks for it.

The pair room of that number and ours stays for calls and history. A text sent from it is refused while the number is
a text member there: `inRoomByText`, "This number is in “Soccer parents” by text: write there, or take them out of it".
Otherwise its replies would land in the room. The add dialog warns when the pair room had a text in the last 30 days.

**Which rooms.** Private rooms only:
- Not direct rooms (fixed members), record rooms (the record's readers), phone rooms (their numbers) or public rooms
  (open to every user, and large: each message would go out to outside phones).
- Never an encrypted room: a text is plain, and the room's promise would be broken (`encrypted`).
- Not a room with a disappearing timer (`timerOn`): a text cannot be deleted. The other way round, phase 16's
  `set#RoomTimer` refuses a room with a mapping in force (`bridged`), and a text member is one.

**Data model**

| Entity, field | What |
|---|---|
| `ChatRoomBridge.memberChatActorId` | A text member's mapping: the external actor it is for (`ChatExternalUser` of the number's bridge, `externalUserId` the number). Its `externalRoomId` is `<ours>/<theirs>#<chatRoomId>`, so it is never a pair's `ours/theirs` and two rooms never share one |
| `ChatRoomBridge.addedByUserId`, `consentDate` | Who added the member, and when they confirmed the person agreed to get the room's texts |
| `ChatRoomBridge.welcomeDate`, `welcomeReasonCode` | The welcome text sent, or why not |
| `PhoneNumber.roomTextsEnabled` | Y: the number may carry rooms' text members (`VoiceEntities.xml`; System, Voice, Numbers) |
| `VoicePolicy`: `smsRoomMaxMembers` (10), `smsRoomMaxPerDay` (500), `smsRoomAddsPerDay` (20) | Text members a room; texts a room sends a day; text members one user adds a day. Counted in `SmsSendCount` as `room:<chatRoomId>` and `roomAdds:<userId>`, and inside `all` (`smsMaxPerDay`) |
| `ChatRoomBridge.memberName` | The member's name in this room, as the adder gives it; a moderator renames it. It is the room's alone (the user's change, 2026-11-17): the shared `ChatExternalUser` keeps the number, so another user's rooms never show it. Elsewhere the number shows, or its phone-book name (phase 9) |

**Adding a text member** (`add#TextMember`: `chatRoomId`, `phoneNumber`, `displayName`, `ourNumber` (optional),
`consent` Y). A moderator or owner of the room who is granted a room-texts number. It:
1. checks the room (above), the number (E.164; not ours; not emergency; international and premium only where the
   policy allows; not opted out), the consent, and the limits;
2. picks our number: `ourNumber`, else the first of the adder's room-texts numbers on which the outside number is free;
3. makes the external user and actor on that number's bridge, and the member row (no `userId`, role member);
4. makes the mapping (`CrbBoth`, `memberChatActorId`), and a membership event
   `{action: 'add', chatActorId, byUserId, via: 'text'}`, shown as "Ann added Bob (+1 555 …) by text";
5. sends the welcome text, after the commit, through `send#BridgeDirect` with `check#SmsSend` first, as
   `send#SmsMessage` does: "Ann added you to “Soccer parents”. You get its messages here as texts, and your replies go to
   the group. Reply STOP to leave."

Out: `chatActorId`, `ourNumber`, `warning` (`pairRoomActive`). The dialog shows the cost before Add: "Each message in
this room is sent as a text to each text member. With 3 text members, one message is 3 texts, and a long message or a
photo costs more."

**Out: the room's messages as texts.**
- `append#Event` gives a text member's mapping a delivery for a message only. Edits, reactions, deletes, polls, pins
  and calls stay in the room, so one text member does not take edits and reactions from everyone. Their mappings are
  left out of `get#RoomBridges`' `capabilityMap` and out of `check#BridgeSupport`.
- A mapping never gets its own member's messages. The bridge echo rule (nothing back to `sourceChatBridgeId`) would
  keep one text member's reply from the others on the same bridge, so for these mappings it is by actor:
  `memberChatActorId` is not the sender's.
- The SMS adapter (`sms.rs`) knows a member mapping by its `#<chatRoomId>`:
  - It checks the text with `check#SmsSend` in room mode (`chatRoomId`, no `userId`): the number carries room texts;
    the pair has not opted out; the destination class; the room's and all users' day. The sender need not be granted
    our number, since the room sends.
  - It sends `<sender>: <text>`, with the sender's name cut to 30 characters and the text to the bridge's
    `maxTextLength`, ending "…" when cut.
  - Photos and videos within the bridge's MMS limits go as MMS, as in a pair room. Another file is a line:
    "[report.pdf: a file, not sent by text]". There are no links to files, which a text member could not open.
  - No typing to a text member.
- A refusal (opted out, a day's limit, room texts off) fails that delivery alone, visible as any failed delivery, with
  its reason in Info.

**In: a text member's replies** (`receive#Sms`, before the spam filter and `ensure#PairRoom`):
- A pair text (no other recipients, no provider thread) from a number with an active member mapping on our number goes
  through `ingest#Event` to that mapping, as the member. It is not judged for spam: a moderator added the number with
  its consent.
- STOP (applied first, as now, so the pair is opted out) also takes the member out: the mapping ends, the member row
  gets `leftDate`, and the event "Bob left by replying STOP" is added. START opts the pair back in and does not add them
  back.
- HELP gets one reply a day: "Texts of “Soccer parents” from MR Moqui. Reply STOP to leave."
- If the room refuses the text, it goes on to the pair room as before, so nothing is lost.
- A group text from a member number goes to its group room as now.

**Taking a member out.**
- `remove#Member` with the member's `chatActorId` (a moderator or owner, as for any external actor). A SECA
  (`SmsRoomServices.xml`, conditioned on `chatActorId`) then ends its mapping, so its texts go back to the pair room.
- No goodbye text.
- An archived room's mappings count for nothing: `route#MemberText` and the free-number pick pass over them, so the
  person's texts go to the pair room and the number is free for another room.

**Services** (`framework/service/org/moqui/call/SmsRoomServices.xml`, `SmsRoom.js`)

| Service | What |
|---|---|
| `add#TextMember` | Above. Refused: `notAllowed`, `roomType`, `encrypted`, `timerOn`, `archived`, `badNumber`, `emergency`, `internationalNotEnabled`, `premiumNotEnabled`, `optedOut`, `consentRequired`, `noRoomNumber` (the adder has no room-texts number), `noFreeNumber`, `alreadyMember`, `tooManyMembers`, `dailyLimit` |
| `find#TextMembers` (`chatRoomId`) | The room's text members for its members: actor, name, number, our number, added by and when, the welcome's state, opted out, the last delivery's failure |
| `get#TextMemberChoices` (`chatRoomId`, `phoneNumber`) | The adder's room-texts numbers, each free or taken for that number, and whether the pair room was active |
| `rename#TextMember` (`chatRoomId`, `chatActorId`, `memberName`) | A moderator |
| `end#TextMember` (internal; the SECA, STOP) | Ends the mapping |
| `route#MemberText` (internal; `receive#Sms`) | The member mapping of a pair text, or none |

Hooks in shared files:
- `ChatServices.xml`:
  - `append#Event`: a member mapping gets messages only, and not its own member's (two conditions in the mappings
    filter);
  - `get#RoomBridges`: member mappings left out of `bridgeMap` and `capabilityMap`, and counted in `textMemberMap`;
  - `check#BridgeSupport`: member mappings not counted.
- `Sms.js`:
  - `receive`: the member's route, STOP and HELP;
  - `checkSend`: room mode, and `inRoomByText` for the pair room;
  - `inMyPhoneRoom`: the `#` part left out, so text members' names come from the phone book.
- `crates/moqui-channels/src/sms.rs`: `numbers_of` reads the `#<chatRoomId>` part (`member_of`); `deliver` takes the
  room mode and the prefix; `typing` skips a member mapping.

**Screens**

| Work | Files |
|---|---|
| People: each text member with a phone icon, their name and number, "Text" and its state (welcome not sent, opted out, last text failed); a moderator's Rename and Remove. "Add by phone number" for a moderator, in a private room that is not encrypted (`m-chat-text-member-add`: number, name, our number when several, the consent box, the cost note, the pair room's warning) | `webroot/screen/webroot/js/WebrootChatTexts.js` (new: `moqui.chat.texts`, `m-chat-text-member-add`), loaded with `WebrootChatInvites.js` |
| Hooks: the People tab's text member rows and Add; the header chip "3 by text", which opens People; the composer's line "Also sent as a text to 3 numbers"; the delete confirm's "Text members already got it as a text"; the membership lines (`via: 'text'`, left by STOP) | `Chat.qvue` |
| Transitions `textMembers`, `textMemberChoices`, `addTextMember`, `renameTextMember` | `basic-comms/template/ChatTextTransitions.xml` (new), included in `Chat.xml` |
| Numbers: "Room texts" on each number. Fraud: the three limits beside the SMS limits | `System/Voice/Numbers.xml`, `System/Voice/Fraud.xml` |
| The terminal: a text member is a member with its name, as any external actor; no Add | — |
| Bundled screens regenerated | `bundled-screens.js` |

**On a phone and on a desktop.** The same screens. At 390 px:
- Add by phone number is a full-screen dialog with the number field first (`type="tel"`);
- the cost note sits above Add;
- the People rows wrap the number under the name.

On Android, the number field offers the phone's contacts (`moqui.chat.contactOptions`, phase 9), so a person is picked
rather than typed. The desktop app's bundled Chat is the same; offline, Add says "needs a connection".

**Bridges and encryption.**
- A text member's messages are plain (SMS, `Encryption::None`). A room with text members is never encrypted (refused
  above), so its label stays the plain room's.
- The room's other bridges (GroupMe, XMPP) are unchanged. A bridged person's message goes to the text members as
  "<their name>: …", and a text member's reply goes to the other bridges as the member.
- A disappearing timer is refused (above).
- Scheduled messages (phase 15) go to text members when they are sent.
- RCS: a number whose bridge is RCS (`CbtRcs`) sends room texts through the same adapter, with the same prefix.

**Failure tolerance.**
- Each text member is a delivery of its own: one that fails or is refused leaves the others.
- A welcome text that fails leaves the member added, with the reason on their row.
- If the member route fails in `receive#Sms`, it is logged and the text goes to the pair room as before.
- If STOP's removal fails, it is logged; the pair is opted out all the same, so nothing more is sent.
- A room texts number turned off leaves its mappings, so replies still arrive; sends are refused `roomTextsOff`
  until it is on again or the member is taken out.

**Security and abuse.**
- Only a room's moderator or owner adds, with a granted number the administrator marked for room texts, and confirms
  consent. The welcome text names the room and the adder and says how to leave.
- STOP is applied as the carrier's rule says, for the pair.
- Limits: 10 text members a room, 20 adds a user a day, 500 room texts a room a day, inside the deployment's day
  (`smsMaxPerDay`). A text to each member counts once in each.
- Cost: the add dialog and the composer's line say what a message costs. The limits are on System, Voice, Fraud.
- A text member sees only what is sent after they were added. Nothing of the history goes out.
- Their number shows to the room's members, as an outside number shows to its own room's members (§14.4).
- D1:
  - a number on the adder's Blocked list cannot be added by them (`blocked`);
  - a text member's texts are not judged for spam;
  - Report spam is a phone room's, not a group room's. A moderator takes the member out instead.
- Inbound texts and files have the bounds they have in a pair room.

**Left out.**
- A short room code for one number shared by many rooms.
- Group MMS to the members, and text members of a public room.
- Edits, reactions, deletes, polls, rich cards and calls to text members, and links to files.
- History sent to a new member, and quiet hours.
- Hiding a blocked number's texts in a group room (as phase 11 left it out of group texts).
- The SIM's own texts (`sim-sms`) as a room's number.
- A text member who is also a user here is not told apart.

**Tests (regression).**
- `framework/test/SmsRoomMembers.spec.js` (new). No provider is reached: deliveries are rows, and `receive#Sms` is
  called as the webhook would be. It covers:
  - each refusal of `add#TextMember`;
  - the free-number pick and `noFreeNumber`;
  - the welcome checked and counted;
  - a message makes one delivery a member and none for an edit or a reaction;
  - a member's reply posts as the member and goes to the other members, not back;
  - `check#SmsSend` room mode, with its day's limit and room texts off;
  - `inRoomByText` in the pair room;
  - STOP takes the member out, and START does not add them back;
  - HELP once a day;
  - remove ends the mapping, and the next text goes to the pair room;
  - an archived room's text goes to the pair room;
  - `capabilityMap` keeps reactions with a text member.
- moqui-channels `src/tests.rs` (the SMS adapter's, on Telnyx): on the fake server, a member mapping's send has the prefix and the cut, is checked in
  room mode, gives its place back when refused, and has no typing.
- `basic-comms/test/BasicCommsTexts.spec.js`: the transitions `textMembers`, `textMemberChoices`, `addTextMember`.
- `basic-comms/test/BasicComms.spec.js`: Numbers and Fraud render there already; the new fields are checked on that
  render.
- `webroot/test/client/WebrootChat.spec.js` (offline): `moqui.chat.texts` cost line and state texts.
- No client or acceptance spec. A text member with a real phone is a §18.4 check.

**Docs.**
- [SMS](../docs/interfaces/SMS.md): Text members of a room (routing, the number's switch, the limits, STOP and HELP,
  cost).
- [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): member mappings (`memberChatActorId`, the `#` room id, the echo
  rule by actor, messages only).
- [Comms](../docs/tools/Comms.md): Add by phone number.
- The BasicComms README: Numbers' Room texts.
- §18.4: after phase 18, a text member on a real phone (welcome, a message both ways, a photo, STOP), on Telnyx and on
  Twilio.

**Status (2026-11-18): built** as designed, with these changes:
- The user's change (§19.1): the name a moderator gives a text member is the room's alone, `ChatRoomBridge.memberName`;
  the shared `ChatExternalUser` keeps the number. `get#ActorNames` takes `chatRoomId` and names a text member of that room
  so (with `textMember` and `phoneNumber`), for `append#Event`, `get#Room`, `get#Timeline`, `get#MessageSummaries`,
  `find#RoomFiles`, `find#RoomLinks` and `find#RoomReactions`; `get#Room`'s `memberList` carries `textMember` and
  `phoneNumber`; `chat_bridge.rs` `sender_name` uses it for the text's "<sender>: ". Elsewhere (the room list's last
  message, Activity) the number shows, or its phone-book name.
- The add dialog says, for each of the adder's numbers the person is already on, "This person is already a text member
  of <room> on this number; another number is needed." (`get#TextMemberChoices` `takenRoomName`).
- The SMS adapter passes `inRoom` with a pair room's check, so `check#SmsSend` refuses `inRoomByText` there.
- A text member's ingest has a transaction id of its own (`member:<eventId>`), so a text its room refuses still goes to
  the pair room under the provider's id.
- HELP's reply reads its mapping unlocked (its date is written in a transaction of its own).
- Files: `SmsRoom.js`, `SmsRoomServices.xml`, `SmsRoom.secas.xml` (new); `Sms.js` (`receive`'s member route, `checkSend`'s
  room mode and `inRoomByText`, `inMyPhoneRoom`), `SmsServices.xml`, `Voice.js` and `VoiceServices.xml` (the number's
  switch, the three limits), `VoiceEntities.xml`, `ChatEntities.xml` (`ChatRoomBridge` member fields),
  `ChatServices.xml` (`append#Event`, `get#RoomBridges`, `check#BridgeSupport`, `get#ActorNames`, `get#Room`);
  `crates/moqui-channels/src/sms.rs` (`member_of`, `member_request`, room mode, no typing), `crates/moqui-core`
  `chat_bridge.rs` (`sender_name`); `template/ChatTextTransitions.xml` (new), `Chat.xml`, `Chat.qvue` (People's text
  members, the cost line, the delete confirm), `System/Voice/Numbers.xml`, `System/Voice/Fraud.xml`.
- Tests. Regression: `framework/test/SmsRoomMembers.spec.js` (new: the rules above), `basic-comms/test/BasicCommsTexts`
  (the transitions), moqui-channels `src/tests.rs` (on the fake Telnyx: the welcome, a message as "ALICE: …" counted as
  the room's, the reply in and nothing back, a number turned off refused and given back; the room id and the text's
  shape), `webroot/test/client/WebrootChat.spec.js` (the pure parts).
- A room with a timer refuses text members (`timerOn`, `ChatRoom.messageTimerSeconds` of phase 16), and phase 16's
  `set#RoomTimer` refuses a room with text members (`bridged`: a mapping in force).
- Left: §18.4's row; Add by phone number is not walked in a browser.

**Review fixes (2026-11-18).**
- *The adder's permissions.* `add#TextMember` checks the adder's own destination permissions (`VoiceUserSetting`, through
  `Sms.js` `destinationRefusal`). In room mode `check#SmsSend` reads the mapping's `addedByUserId` and applies their
  account (not disabled or terminated), their grant of the number and their permissions, and counts their day
  (`user:<id>`, `smsMaxPerUserPerDay`) beside `room:` and `all` (`counterUserId` for the release). When the adder fails
  them the member's texts stop (`adderNotAllowed`, or the permission's reason) and the room's moderators get the
  `ChatTextMember` notification, once a day for the member (`noteAdderProblem`).
- *One room per number holds.* `store#TextMember` locks our number's `PhoneNumber` row, then the room's (ChatExpiry
  `checkBridge`), always in that order, and checks the room's rules and the timer again under them. A member of an
  archived room still takes the number (`takenOn`; the dialog says the room is archived), so unarchiving never makes two
  rooms for one person; their texts go to the pair room meanwhile (`liveOn`). `routeOf` never picks between two
  mappings: with two it logs an error and the text goes to the pair room. The test bed runs one call at a time, so the
  race itself is not tested; the lock order is in `storeTextMember`'s comment.
- `inRoomByText` is a pair room's: the adapter passes `inRoom` only for one recipient, so a group text with a text member
  goes as ever.
- A private room every user may read (`visibilityEnumId` not `CrvMembers`) takes no text members (`openRoom`).
- Shared numbers: SMS.md and the Numbers screen's help advise numbers of their own for room texts and say what adding a
  customer of a shared number does; the add dialog's `pairRoomActive` warning names it.
- HELP's answer names the deployment by `chat@server-name` when it is set (there is no other name property).
- Tests: `SmsRoomMembers.spec.js` (an archived room's member taking the number, two mappings to the pair room, an
  international number refused for the adder, room texts on the adder's day and its limit, the grant lost with the
  moderators told, `openRoom`, `tooManyMembers`, `dailyLimit`, `blocked`, `optedOut`); moqui-channels (the adder's
  day, a group text with a member sent, the pair room's text refused `inRoomByText`).

#### Phase 19: event messages with RSVP

Designed 2026-11-17 (D2e; §14.5, §17 #8). An event is a message with a title, a start and an end in a time zone, a
place and a description. Members answer Going, Maybe or No; the card shows the counts and who answered. A member who
goes gets an optional reminder (phase 15's), and a calendar entry where the server has a calendar, else an `.ics` file.
A change or a cancellation is posted to the room as a message, so everyone, every bridge and every text sees it.

The poll is the model (§14.1, `create#Poll`): the message's body is the event in words, which every client, bridge,
search and notification already uses; the answers are rows, read into the timeline and pushed as updates.

**What a calendar is here.** The framework keeps no calendar (`docs/interfaces/Sync.md`): CalDAV is the framework's,
its data an adapter's, and mr-mantle has the calendars (`mantle.work.calendar.Calendar`, a `WetEvent` `WorkEffort`, a
`WorkEffortParty` per attendee, `CalendarWorkEffort`). mr-boss already puts a mail invitation on a user's calendar
through the `invitation` provider hook (`ironstone.comms.InboxServices.respond#Invitation`). So:
- **A new provider hook `chat-calendar`** (`PROVIDER_HOOKS` in `crates/moqui-core/src/screen/shell.rs`, its element in
  `xml-screen-3.xsd`, the interface `org.moqui.comms.ProviderInterfaces.chat#CalendarEntry`). It takes only `chatRoomId`,
  `eventSeq` and `action` (`answer`, `change`, `cancel`). The provider reads `ChatCalEvent` and the caller's
  `ChatCalAnswer` itself, so no title, time or user passed in is trusted. On `answer` it puts, changes or removes the
  current user's entry. On `change` and `cancel` it updates or cancels the entries it made for that event (UID
  below). It answers `eventUrl`.
- **Without a provider** (the framework alone, or mr-mantle without an app that declares the hook): "Add to calendar"
  is the event as an `.ics` file (the framework's `ICalendar` tool, `write({method: PUBLISH, events})`; CANCEL once
  cancelled), and in the Android app the phone's calendar app opened on a new event (below).
- The UID is `chat-<chatRoomId>-<eventSeq>@<host>` (the host of `comms@app-root`), and the SEQUENCE is the event's
  `revision`. A calendar that imports the file again after a change updates its entry rather than adding one.
- mr-boss's provider is a change in its own repository, `respond#Invitation`'s path with the UID as `universalId`. It
  is not in this branch (§19.1 asks).

**Data model** (new `framework/entity/ChatCalEntities.xml`, listed in `MoquiDefaultConf.xml` `load-entity`)

| Entity, field | What |
|---|---|
| `moqui.chat.ChatCalEvent` (`chatRoomId`, `eventSeq` pk: the message) | `title`, `startDate`, `endDate` (instants), `timeZone` (an IANA zone: the times are shown and written in it), `allDay` (Y: whole days in that zone), `place`, `description`, `createdByUserId`, `revision` (0, then +1 a change), `cancelledDate`, `cancelledByUserId`, `lastChangeEventSeq` (the last change message), `goingCount`, `maybeCount`, `noCount`. Index (`startDate`) |
| `moqui.chat.ChatCalAnswer` (`chatRoomId`, `eventSeq`, `chatActorId` pk) | A member's answer: `userId`, `answerEnumId`, `answeredDate`, `remindMinutes` (empty: no reminder), `chatMessageReminderId` (the phase 15 reminder this answer made), `calendarDate` (when the provider last put the entry), `calendarRevision`. `chatActorId` as in `ChatPollVote`, so a bridge's answer can come later |
| `ChatCalAnswerType` (Enumeration) | `CcaGoing`, `CcaMaybe`, `CcaNo` |

`content.calEvent` is `{title}` on the message, as `content.poll` is `{question}`; the state is in the rows.

Limits: a title of 1 to 200 characters, a place up to 300, a description up to 2,000; it ends after it starts, at most 31
days later, and in the future; it starts within 2 years; a known zone (`ec.l10n.getAvailableTimeZones`); at most 20
changes an event.

**The event in words** (`ChatCal.js` `fallbackText`, in the sender's locale, the times in the event's zone):

```
Event: Picnic
Sat 21 Nov 2026, 14:00–16:00 (America/Denver)
Where: Liberty Park
Bring a chair
```

An all-day event reads "Sat 21 Nov 2026, all day". This is the message's body and `bodyText`: the room list, search,
notifications, the terminal, XMPP, SMS and every bridge show it. A change message reads "Changed: Picnic. Now Sun
22 Nov 2026, 14:00–16:00 (America/Denver), at Liberty Park"; a cancellation "Cancelled: Picnic, Sat 21 Nov 2026".

**Services** (new `org.moqui.chat.ChatCalServices`, logic in `ChatCal.js`; operations table as `ChatRich.js`)

| Service | What |
|---|---|
| `create#CalEvent` | `chatRoomId`, `title`, `startDate`, `endDate`, `timeZone`, `allDay`, `place`, `description`, `threadRootEventSeq`, `clientRef`, `roomUrl`. A member of a room that is not archived. Refused: an encrypted room (`encryptedRoom`, as polls: the server keeps the answers and makes the text), a bad time or zone (`badTime`), too long (`tooLong`). `send#Message` with the body above, then `content.calEvent` and the row, then `push#Update {calEvent}` (the shared state). Out: `eventSeq`, `event` (with `calEvent`), `duplicate`, `reasonCode` |
| `get#CalEventStates` (internal) | `chatRoomId`, `eventSeqList`, `shared`: `calEventMap` eventSeq to `{title, start, end, timeZone, allDay, place, description, revision, cancelled, cancelledDate, ended, createdByUserId, going, maybe, no (counts), answers {going, maybe, no: [userId]}, mine, myRemindMinutes, myCalendarDate}`. `shared` leaves out the `my…` fields, as `get#PollStates` does |
| `answer#CalEvent` | `chatRoomId`, `eventSeq`, `answer` (`going`, `maybe`, `no`, or empty to take it back), `remindMinutes` (0 to 10,080; empty for none), `addToCalendar`. A member who may post; refused when cancelled (`cancelled`) or ended (`ended`). The row is replaced and the counts are kept on `ChatCalEvent` (locked). Then each of the following on its own, each failure logged and answered (`reminderFailed`, `calendarFailed`), the answer standing: <br>• the reminder: Going or Maybe with `remindMinutes` sets one through `ChatScheduleServices.set#Reminder` at the start less the minutes, or moves the one the answer made; none when that time is less than a minute ahead (`reminderSkipped`); No or a take-back cancels it; <br>• the calendar: with `addToCalendar` and a provider, `chat-calendar` `answer`; No or a take-back removes the entry the provider made. <br>`push#Update {calEvent}` (shared). Out: `calEvent` (with `mine`), `eventUrl` |
| `update#CalEvent` | Its creator while a member who may post, or a moderator or owner. Any of the fields; `revision` + 1. A change of the title, time, place or description posts a change message: `send#Message` as the changer, `replyToEventSeq` the event and in its thread, `content.calChange {eventSeq, revision, what: [title, when, place, description]}`. The answers' reminders move with the start, each on its own (dropped when now past: `CrmCancelled`). Then `chat-calendar` `change`. Refused while it is cancelled or ended, and after 20 changes (`tooManyChanges`) |
| `cancel#CalEvent` | The same people. `cancelledDate`; a cancellation message (`content.calChange {eventSeq, cancelled: true}`); the answers' pending reminders `CrmCancelled`; `chat-calendar` `cancel`. It cannot be undone |
| `get#CalEventIcs` | For one who can read the room: `fileText` (`text/calendar`), `fileName` (`<title>.ics`). PUBLISH, or CANCEL once cancelled; the UID and SEQUENCE above; `DTSTART;TZID=` in the event's zone, or a `VALUE=DATE` for a whole day; `LOCATION`, `DESCRIPTION`, and the message's link as `URL`. Refused with no `ICalendar` tool (`noIcs`) |
| `forget#CalEvent` (internal) | When the message goes (a redaction through `clear#MessageContent`, retention, phase 16's expiry): the rows go, the answers' pending reminders are cancelled, and `chat-calendar` `cancel` is called. Each part on its own |

**Transitions** (new `basic-comms/template/ChatCalTransitions.xml`, one `transition-include` line each in `Chat.xml`):
`createCalEvent`, `answerCalEvent`, `updateCalEvent`, `cancelCalEvent`, `calEventIcs` (a GET that sends the file, as
`attachment` does), `calEventFields` (the zone choices). A refusal is answered with its `reasonCode`.

**Screens** (`Chat.qvue`: two sub-components in a block of their own, `chat-cal-box` and `chat-cal-card`)

| Part | Phone (bubbles) | Desktop |
|---|---|---|
| Make one | "Event" in the composer's new More menu (below), and in the thread composer | The same menu |
| `chat-cal-box` | Full screen: Title, All day, Starts and Ends (`m-date-time`; Ends follows Starts, one hour later), Time zone (the user's zone first, then a searchable list), Place, Details. Change uses the same box with the event's values. In a room with a disappearing timer (phase 16) it says "This room's messages disappear after 1 day: the event's card goes too" | A 520 px dialog |
| `chat-cal-card` | In the message, beside the poll's card: a date badge, the title, the time in the viewer's zone, and the event's own when it differs ("20:00 in America/New_York"), the place, the details (3 lines, More). Going, Maybe and No as one toggle row: a tap answers, the same tap again takes it back. "5 going · 2 maybe · 1 can't go" opens who answered in three groups (a bottom sheet). After Going or Maybe: "Remind me" (none, 15 minutes, 1 hour, 1 day before: only the times still ahead; 1 hour by default for Going, none for Maybe) and "Add to calendar". Cancelled: the title struck through, "Cancelled", no buttons. Ended: "Ended", no buttons. The creator's and a moderator's menu: Change, Cancel the event (confirmed) | The same card; who answered is a menu |
| A change message | Its text, and an "Event changed" chip that scrolls to the card | Same |

- The More menu (`sym_o_add_circle`, beside the poll's button; on a phone also the attach menu's last items) holds
  Event and Share location (phase 20). It is hidden in an encrypted room's composer as the poll's button is; Share
  location stays there.
- "Add to calendar" is one action whose path depends on where it runs:
  - with a provider, it answers with `addToCalendar` and then shows "Open in calendar" (`eventUrl`);
  - without one, on the web and the desktop app, it downloads `calEventIcs`;
  - in the Android app, it opens the phone's calendar app on a new event, filled in. This is a new host command
    `calendar_insert` (`commands.rs`, then `android.rs`, then a small `CalendarIntent.kt` with
    `Intent.ACTION_INSERT` on `CalendarContract.Events.CONTENT_URI`). It needs no calendar permission, since the
    phone's calendar app saves the event, and it needs the next APK. Until then the phone gets the `.ics` file.
- The helpers (`WebrootChat.js`, a block `moqui.chat.cal`): `when(event, zone)` (the time line in the viewer's zone),
  `zoneNote(event, zone)`, `countLine(event)`, `remindChoices(event, now)`, `canAnswer(event, now)`. The card reads
  `item.calEvent`, kept current by `push#Update`, as a poll's state is.
- `/event` stays the apps' quick create (`quickCreateText`); the chat event is the menu's.
- Every text through `t()`.

**Hooks in shared files** (small, so the parallel phases merge):
- `ChatServices.xml`:
  - `get#Timeline`: the batch read, as for polls, and `calEvent: redacted ? null : calEvents[seq] || null`;
  - `send#Message`'s event: `calEvent: null`;
  - `edit#Message` refuses an event ("An event is changed with Change");
  - `clear#MessageContent` calls `forget#CalEvent`.
- `Chat.xml`: the six `transition-include` lines.
- `Chat.qvue`: the card in `messageTemplate`, the More menu, the box.
- `ChatTui.xml`: none (it shows the body).
- `shell.rs`, `xml-screen-3.xsd`, `ProviderInterfaces.xml`: the hook.
- After phase 16 merges: its expiry's delete list calls `forget#CalEvent`.

**Bridges and encryption.**
- Every bridge sends the event's text, and each change or cancellation as a message of its own, through the normal
  path (`ChatBridgeDelivery`).
- SMS, RCS and SMS members (phase 18) get the text. An answer by text ("going") is not read: left out.
- GroupMe's own events are not mapped (left out); a GroupMe group gets the text.
- An encrypted room has no events, as it has no polls: the server keeps the answers and makes the text and the
  calendar file.

**Phone and desktop.** The same bundled Chat (`bundled-screens.js` regenerated). The desktop app downloads the `.ics`
through its save path.

**Failure tolerance.**
- An answer stands when its reminder or calendar step fails: each runs on its own, logs its failure and answers it.
- A change moves each answer's reminder on its own.
- A provider that fails on `change` or `cancel` is logged, and the change message still goes.
- `forget#CalEvent` does each part on its own.

**Security and abuse.**
- Only members who may post answer. Who answered is visible to whoever can read the room, as a poll that is not
  anonymous is.
- Only the creator (still a member) or a moderator changes or cancels. At most 20 changes, each one a message under
  the room's send limits.
- The provider is given ids only and reads the rows itself. The `.ics` goes only to a reader of the room.
- The place and description are plain text, never HTML. The client makes the map search link
  (`openstreetmap.org/search?query=`, with `encodeURIComponent`) only when the person taps it; the server fetches
  nothing.
- A reminder keeps the room and the message's number only (phase 15).

**Left out.** Events in encrypted rooms; recurring events; answers by text or from another network; guests outside the
room; a limit on places and a waiting list; a note with an answer; telling the creator of each answer; answering in the
terminal; GroupMe's native events; floating times with no zone.

**Tests (regression).**
- Server: `framework/test/ChatCal.spec.js` (new):
  - `create#CalEvent`'s refusals: an encrypted room, archived, not a member, ended, over 31 days, past 2 years, a bad
    zone, too long;
  - the body in the event's zone, and an all-day event's;
  - answers: the counts, a replaced answer, a take-back; refused once cancelled, once ended, and from one who is not a
    member;
  - `get#Timeline`'s `calEvent` with `mine`, and the update push with none;
  - reminders: Going with 60 makes a `CrmPending` reminder an hour before the start; a change moves it; No and a
    cancellation cancel it; one too near is skipped;
  - a change: the reply message with `calChange`, `revision` + 1, by its creator or a moderator only, and at most 20;
  - a redaction removes the rows and cancels the reminders; `edit#Message` refused;
  - the `.ics`: PUBLISH with UID, SEQUENCE and TZID, then CANCEL; refused for one who cannot read the room;
  - no provider: `calendarFailed` false, no entry, `eventUrl` empty;
  - a bridged room: the event's and the change's `ChatBridgeDelivery` rows, with the text.
- Rust: `shell.rs`'s hook list (`chat-calendar`) where its test lists the hooks.
- Client, offline: `webroot/test/client/WebrootChat`: `moqui.chat.cal` (the viewer's zone and the event's,
  `remindChoices` past the start, `countLine`).
- basic-comms client: nothing (its budget).
- No acceptance spec.

**Docs.**
- [Comms](../docs/tools/Comms.md): events; "Provider hooks" gains `chat-calendar`.
- [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): an event and its changes as text.
- [Mobile-Client](../docs/ui/Mobile-Client.md): Add to calendar on Android.
- The BasicComms README.
- §14.5.

**Done when.** The regression specs above pass within their budgets; on a real phone, the row of §18.4.

**Status (2026-11-18): built.** What the build changed from the entry above:
- *The writes are committed before the providers.* A `chat-calendar` provider runs in a transaction of its own and reads
  only committed rows, so `answer#`, `update#` and `cancel#CalEvent` make their writes in a transaction of their own
  (`ChatCal.js` `committed`, which a screen's transition needs, as it has one in place) and call the providers after.
- *The own answer is one object*: `calEvent.mine {answer, remindMinutes, calendarDate}`, so a pushed update (which has
  none) keeps it (`moqui.chat.applyUpdate`).
- *The UID's domain* is the property `chat@calendar-uid-domain` (`chat.moqui` by default).
- *mr-boss's provider is built*: `ironstone.comms.ChatServices.sync#ChatCalendarEntry` on mr-boss branch
  `d2e-chat-calendar` (`856b8737`), with `test/Comms/ChatCalendar` (2 features). One Mantle event per chat event, its
  `universalId` the UID; a cancel also when the message is gone.
- Regression: framework `ChatCal` (6 features), webroot `WebrootChat` (one feature); the bundled screens golden
  regenerated. Left: §18.4's row, and Android's `calendar_insert` (`CalendarIntent.kt`) is checked on a phone only.

**Review fixes (2026-11-18).**
- *A failed provider change or cancel is tried again*: `ChatCalEvent.calendarSyncPending`, `calendarSyncTries` and
  `calendarSyncNextDate` (a minute, doubled each try to a day; given up and logged after ten), retried at the event's next
  answer or change (`mark#CalendarSync`). Each failure is logged.
- *mr-boss's provider* (`cbbf1e72` on `d2e-chat-calendar`) locks the chat event's own row (else its room's) before it
  looks for or makes the Mantle event, so two answers at once make one; a change and a cancel touch every event of the
  UID; it checks that the caller reads the chat room first. The Mantle event is shared: an edit in Commons is overwritten
  by the next change of the chat event.
- `create#CalEvent` checks the room's access before it refuses an encrypted room. The answers of one who left the room
  are in no count and no list (their rows stay).

#### Phase 20: sticker packs and location messages

Designed 2026-11-17 (D2f; §14.3, §17 #7, as decided in the batch: packs local with no outside service, GIF search
later; a location with no map, live location later with Maps).

**Stickers.** An administrator keeps the server's packs, and each user keeps packs of their own. A sticker is a
picture stored as content: PNG, WebP or GIF, animated ones too. A sticker picker sits beside the emoji picker, and a
sticker is a message that shows large.

**A sent sticker is a copy in the room.** Sending one copies its picture into the room as an ordinary
`ChatAttachment`. Stickers are small (512 KB at most), and the copy means:
- every path files already take works for it: access by the room, retention, redaction, disappearing messages, the
  bridges' files and MMS;
- removing a pack or a sticker never breaks a message already sent;
- a user pack stays private to its owner.

**Data model** (new `framework/entity/ChatStickerEntities.xml`, in `load-entity`)

| Entity, field | What |
|---|---|
| `moqui.chat.ChatStickerPack` (`chatStickerPackId` pk, sequenced) | `packName` (up to 60), `ownerUserId` (empty: the server's pack), `statusEnumId`, `sequenceNum`, `coverStickerId`, `createdByUserId`, `createdDate`. Index (`ownerUserId`, `statusEnumId`) |
| `ChatStickerPackStatus` (Enumeration) | `CspActive`, `CspHidden` (kept, not offered) |
| `moqui.chat.ChatSticker` (`chatStickerId` pk, sequenced) | `chatStickerPackId`, `sequenceNum`, `emoji` (one emoji: search and the text form), `altText` (up to 80: the description a screen reader reads), `contentLocation`, `thumbLocation` (a still PNG of 128 px, `ec.resource.makeThumbnail`), `contentType`, `fileSize`, `width`, `height`, `digest` (SHA-256: the same picture once a pack), `createdDate` |
| `ChatAttachment.stickerOf` (extend-entity) | The `chatStickerId` a room's copy came from: `find#RoomFiles` leaves stickers out of the Media and Files tabs |

- Storage: `${chat/stickers@root || 'dbresource://chat-stickers'}/<packId>/<stickerId>/<n>.<ext>`, through
  `ec.resource`.
- The type is the decoder's guess from the bytes, never the name or the browser's: PNG, GIF or WebP only, stored with
  that type and extension. The sides are read from the header before any decode, then it is decoded within a small
  memory limit (`ec.resource.makeStickerThumbnail`, which answers its sides and makes its thumbnail).
- Limits (properties): `chat/stickers@max-bytes` 512 KB; `chat/stickers@max-side` 512 px a side; 200 stickers a pack; 50
  server packs; 10 packs and 100 stickers a pack for a user; `chat/stickers@uploads-per-hour` 60 a user.

**Services** (new `org.moqui.chat.ChatStickerServices`, logic in `ChatSticker.js`)

| Service | What |
|---|---|
| `get#StickerPacks` | The caller's own packs, then the server's active ones, each with its stickers `{chatStickerId, emoji, altText, width, height}` and a `version` (the newest change), so Chat reads them again only when they changed |
| `create#StickerPack`, `update#StickerPack`, `delete#StickerPack` | A user's own pack. A server pack needs the `ChatStickerAdmin` permission (ADMIN has it, in `ChatSetupData.xml`). Delete removes its files, each on its own; a sent copy stays |
| `add#Sticker`, `update#Sticker`, `remove#Sticker` | The pack's owner or a sticker admin. `file`, `emoji`, `altText`. Refused: a type other than PNG, WebP or GIF (`badType`), too large or of the wrong size (`tooLarge`), the same picture again (`duplicate`), over the limits (`tooMany`) |
| `get#StickerImage` | `chatStickerId`, `thumb`, for the `stickerImage` transition. A server pack's sticker, for any chat user; a user pack's, for its owner only. The bytes are sent with their stored type, `X-Content-Type-Options: nosniff`, inline, and cached privately for a day |
| `send#Sticker` | `chatRoomId`, `chatStickerId`, `replyToEventSeq`, `threadRootEventSeq`, `clientRef`, `roomUrl`. A sticker the caller may use, in a plain room. The picture is copied as the room's `ChatAttachment` (`stickerOf`, the name `sticker.<ext>`, in the daily file quota), then `send#Message` with no body, that file, and `content.sticker {chatStickerId, packName, emoji, altText, width, height}`. Refused in an encrypted room (`encryptedRoom`): the client sends it there itself (below) |

**Transitions** (new `basic-comms/template/ChatStickerTransitions.xml`): `stickerPacks`, `stickerImage`, `sendSticker`,
and the manage screens' own.

**Location.** A location message carries coordinates or an address or both, an optional label, and "Open in maps".
There is no map, no geocoding and no outside request.
- `content.location {latitude, longitude, label, address, accuracy, source}`:
  - `latitude` and `longitude` are rounded to 6 places;
  - `label` is up to 100 characters and `address` up to 300;
  - `accuracy` is in metres, from the device;
  - `source` is `here` (the device's position), `typed`, or `network` (an inbound RCS location).
- **Its words** (`ChatLocation.js` `fallbackText`; the client's `moqui.chat.locationFallback` makes the same for an
  encrypted room, and a spec keeps the two in step):

  ```
  Location: Liberty Park, 600 E 900 S, Salt Lake City
  https://www.openstreetmap.org/?mlat=40.745210&mlon=-111.875000#map=17/40.745210/-111.875000
  ```

  With an address only, the link is `https://www.openstreetmap.org/search?query=<address>`: a search the person runs on
  OpenStreetMap's site by tapping it; the server never makes it. This is phase 10's `placeText` (`Sms.js`), moved to
  `ChatLocation.js` and used for both directions.
- **Open in maps**:
  - in the Android app: `geo:<lat>,<lon>?q=<lat>,<lon>(<label>)` (RFC 5870, with Android's `q`), or
    `geo:0,0?q=<address>`, which Android hands to the person's maps app;
  - on the web and the desktop app: the OpenStreetMap link.

  The card also has Copy (the coordinates, or the address).
- `send#Location` (`ChatLocationServices`, a plain room): `chatRoomId`, `location`, `replyToEventSeq`,
  `threadRootEventSeq`, `clientRef`, `roomUrl`. Refused: neither coordinates nor an address, a coordinate out of range
  or not finite, too long (`badLocation`). `send#Message` with the words as the body, then `content.location`.
- In an encrypted room the client puts `location` and the words in the ciphertext and sends as any encrypted message;
  the server never sees the place.
- **Inbound.** An RCS location (Twilio `Latitude`/`Longitude`/`Label`/`Address`, Sinch `location_message`: phase 10's
  `sms::Location`) becomes a location message (`source: network`) with the same words. `Sms.js` gives the ingest item
  `location`, and `ingest#Event` sets `content.location`, a hook in `ChatBridgeServices.xml`. A GroupMe location
  attachment takes the same path if the phase 14 adapter hands it over (checked at build).

**Sharing a location** (`chat-location-box`, a sub-component of `Chat.qvue`)
- **"Use my current location"**: `navigator.geolocation.getCurrentPosition`, high accuracy, a 15 s timeout, a fix up to
  a minute old. It shows the coordinates and "± 12 m", with a Label field.
  - Android app: the manifest gains `ACCESS_COARSE_LOCATION` and `ACCESS_FINE_LOCATION`. The patched wry
    `RustWebChromeClient.onGeolocationPermissionsShowPrompt` already asks for them at first use, so only the next APK
    is needed.
  - Browser: its own permission prompt.
  - Denied: "Location is off for this app: allow it in the phone's or the browser's settings, or enter the place". With
    no geolocation (an http page, a desktop WebView without it), only Enter it shows.
- **"Enter it"**: one text field, read by `moqui.chat.parseLocation`:
  - coordinates ("40.7452, -111.875", or degrees, minutes and seconds with N/S/E/W);
  - a `geo:` URI;
  - a pasted OpenStreetMap, Google Maps or Apple Maps link: its coordinates read from the link text (`mlat`/`mlon`,
    `@lat,lon`, `q=`, `ll=`), and nothing fetched;
  - any other text: the address.
- A preview of the card, then Send.

**Screens** (`Chat.qvue`, in a block of their own: `chat-sticker-picker`, `chat-sticker`, `chat-location-box`,
`chat-location-card`)

| Part | Phone (bubbles) | Desktop |
|---|---|---|
| Sticker button | Beside the emoji button (`sym_o_sticky_note_2`); a bottom sheet of half the height | A 360 × 420 menu, as the emoji picker's |
| `chat-sticker-picker` | Tabs: Recent (`localStorage` `moqui.chat.recentStickers.<userId>`, 24), then each pack by its cover, the user's own first. A search by emoji or description. The grid shows the still thumbnails; a long press plays one. A tap sends it at once (in the thread, or as the reply being written). "Manage stickers" opens Settings, Stickers. Empty: "No stickers yet: add your own in Settings, Stickers" | A hover plays one |
| A sticker message | The picture at up to 160 px a side, no bubble, its description as `alt`, the time below; a tap opens the image viewer | Up to 180 px |
| Share location | In the More menu (phase 19) and the phone's attach menu; full-screen `chat-location-box` | A 440 px dialog |
| `chat-location-card` | A pin icon, the label (bold), the address, the coordinates with "± 12 m", Open in maps, Copy | Same; Open in maps opens a new tab |

- Manage: `Settings/Stickers.xml` (basic-comms, a plain XML screen with forms, so it renders in every mode): the
  user's packs, New pack, each pack's stickers with Add (file, emoji, description), Hide and Remove.
  `System/ChatStickers.xml` (beside `PushHealth.xml`, permission `ChatStickerAdmin`) does the same for the server's
  packs. Both include one `template/StickerPackWidgets.xml`.
- Helpers in `WebrootChat.js` (a block of their own): `moqui.chat.parseLocation`, `locationFallback`, `geoUri`,
  `osmUrl`, `stickerSearch`, `recentStickers`.
- Every text through `t()`.

**Encrypted rooms.**
- A sticker: the client fetches the picture (`stickerImage`), seals it (`sealFile`), uploads it and sends an encrypted
  message whose content has `sticker` and the file: the ordinary encrypted send. The server sees a sealed file.
- A location: in the ciphertext only.

**Bridges** (outbound):
- A sticker goes as its picture on a bridge that sends files (MMS, GroupMe's image service, XMPP's file links), with no
  text. A bridge that sends no files, or whose limit refuses the picture, gets "Sticker: 👍 thumbs up". That is
  `request_of`'s text fallback in `chat_bridge.rs`, as a poll's question is.
- WebP over MMS shows on most phones, and GIF and PNG on all. No conversion is done.
- A location goes as its words everywhere, with these exceptions:
  - **Sinch RCS** sends `location_message` (`coordinates`, `title` the label, `label` the address) with the words as the
    SMS fallback. Sinch's RCS page says RCS has no location message of its own, and the Conversation API makes it a
    text with a location choice (read 2026-11-17). Not yet tested with Sinch.
  - **Twilio RCS** sends the words: `twilio/location` is WhatsApp only (Twilio's Content API page, read 2026-11-17).
- `DeliveryRequest` gains `location` (phase 10's `sms::Location`), set from `content.location`. Only the Sinch RCS path
  reads it.
- The room list and the push snippet read "Sticker 👍" and "Location: Liberty Park": hooks in `snippetOf` and
  `chat_push.rs` `Content::of`.

**Hooks in shared files.**
- `ChatServices.xml`:
  - `get#Timeline` and `send#Message`'s event: `sticker` and `location` from the content;
  - `find#RoomFiles` leaves out `stickerOf` files;
  - `snippetOf` reads stickers and locations.
- `ChatBridgeServices.xml` `ingest#Event`: `location`.
- `Sms.js`: `placeText` replaced by `ChatLocation.js`, and the item's `location`.
- `chat_bridge.rs`: the sticker's text and `location`.
- `chat_push.rs`: the snippet.
- `sinch.rs`, `sinch/rcs.rs`: the location message.
- `Chat.xml`: three `transition-include` lines.
- `Chat.qvue`: the sticker button, the More menu's item, the two renderers.
- `AndroidManifest.xml`: the two location permissions.
- `ChatSetupData.xml`: the permission and its grant.

**Terminal.** A sticker reads "Sticker 👍", and a location shows its words.

**Failure tolerance.**
- A pack's delete removes each file on its own.
- A sticker that fails its check is refused alone; the others of an upload go on.
- A thumbnail that cannot be made refuses that sticker only.
- A geolocation timeout leaves Enter it open.
- Chat works without stickers when `get#StickerPacks` fails, which is logged; the button says "Stickers could not be
  loaded".

**Security and abuse.**
- A sticker is checked by its bytes (magic and decode), so an SVG, HTML or script cannot pass as a picture. It is
  served with its stored type, `nosniff` and no `Content-Disposition: attachment` trickery.
- A user pack's pictures go to its owner only. Another member sees the copy, under the room's access.
- Size and count limits apply, and each copy counts in `chat@attachment-daily-bytes`.
- In an encrypted room, a sealed file's size may tell the server which sticker it is (stickers are known files): said
  in [Comms](../docs/tools/Comms.md).
- A location is shared only by the person's act, never in the background, and a device position only with the device's
  permission. Coordinates are kept as sent, rounded to about 10 cm. An address-only location reveals only what the
  person typed.
- Every link is built with `encodeURIComponent`. A pasted maps link is read as text and never fetched.

**Left out.** GIF search; live location and Check In (Maps); a map image or tiles; geocoding; saving another person's
sticker to one's own pack; Lottie or other vector stickers; sticker packs from Signal or Telegram; GroupMe's own
location attachment outbound (its text goes).

**Tests (regression).**
- Server:
  - `framework/test/ChatSticker.spec.js`:
    - a pack per user and the server's (refused without `ChatStickerAdmin`);
    - `add#Sticker` takes PNG, WebP and GIF and refuses an SVG renamed `.png`, an HTML file, an oversized one and the
      same picture twice;
    - `get#StickerImage` is refused for another user's pack;
    - `send#Sticker` makes a copy (`stickerOf`) and `content.sticker`, is left out of `find#RoomFiles`, reads "Sticker
      👍" in the snippet, is refused in an encrypted room, and stays after the pack is deleted;
    - a bridged room's delivery carries the file.
  - `framework/test/ChatLocation.spec.js`: `send#Location`'s refusals, the words with coordinates and with an address
    only, `content.location` in the timeline; an inbound RCS location through `receive#Sms` becomes a location message
    with the same words.
- Rust:
  - `moqui-channels`: Sinch RCS sends `location_message` with the SMS fallback text (fake server); Twilio sends the
    text;
  - `moqui-core` `chat_bridge_tests`: a sticker's text fallback, and `location` set on the request.
- Client, offline: `webroot/test/client/WebrootChat`:
  - `parseLocation`: decimal, DMS, `geo:`, the three maps links, an address, out of range;
  - `locationFallback`, the same as the server's for the same input;
  - `geoUri` with a label to escape;
  - `stickerSearch`.
- basic-comms client: nothing.
- No acceptance spec.

**Docs.**
- [Comms](../docs/tools/Comms.md): stickers, their management, location.
- [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): stickers and locations across bridges.
- [RCS](../docs/interfaces/RCS.md): Sinch's location message.
- [Mobile-Client](../docs/ui/Mobile-Client.md): the location permission.
- The BasicComms README, and §14.3.

**Done when.** The regression specs above pass within their budgets; on a real phone, the row of §18.4.

**Status (2026-11-18): built.** What the build changed from the entry above:
- *An inbound place* is kept by `ChatLocationServices.mark#Location`, which `Sms.js` calls after the text is taken in
  (as the postback is), in place of a hook in `ingest#Event`.
- *A sticker's words are its body* ("Sticker 👍"), so the room list, search and the push show them with no hook in
  `snippetOf` or `chat_push.rs`. `chat_bridge.rs` `request_of` leaves the text out when the sticker's picture goes.
- *A sticker's* `width` and `height` are its 128 px thumbnail's, its shape for the layout (its checks: the review fixes
  below).
- *Android*: `geo:` is one of the links `openExternal` hands to the system.
- *The test kit*: `uploadedFile(name, text, type, binary)` takes a binary file, each character a byte, for the specs'
  pictures ([JS-Testkit](../docs/dev/JS-Testkit.md)).
- Regression: framework `ChatSticker` (3 features) and `ChatLocation` (1), moqui-channels (a Sinch location message
  and Twilio's words; the inbound place a location message), webroot `WebrootChat` (two features). Left: §18.4's row.

**Review fixes (2026-11-18).**
- *A sticker's picture is checked before it is decoded* (`ec.resource.makeStickerThumbnail`, `files.rs`): its type from
  its bytes (PNG, GIF or WebP only; a JPEG or BMP named `.png` is refused; it is stored with its real type and
  extension), its sides from its header (none over `chat/stickers@max-side`, 512 px), then a decode within 16 MiB; a tiny
  PNG claiming 100,000 px a side is refused unread. At most `chat/stickers@uploads-per-hour` (60) stickers a user an hour.
- *A refused file goes as words*: when an adapter refuses a message's files (`mediaTooLarge`, `tooManyFiles`,
  `mmsNotSetUp`, `filesUnsupported`, `noFiles`), the round sends its words alone (`DeliveryRequest.text_form`; a
  sticker's "Sticker 👍"); one with no words stays refused.
- *An inbound place keeps phase 10's words*: a label alone, and coordinates out of range, read "Location: label" with no
  link. The receive's own messages are set aside and given back around its place and code steps, never cleared.

#### Phase 21: screen security, one-time codes, send original

Designed 2026-11-17 (D2g; §14.2, §14.4, §17 #9). Builds on phase 16's `ScreenSecurity` and `secure_screen`, and on its
per-event `expireDate`, so it is built after phase 16 merges.

**Screen security** (Android `FLAG_SECURE`: no screenshots, no screen recording, a blank recents thumbnail)
- A user setting `CHAT_SCREEN_SECURITY`, under Settings, Chat, Privacy:
  - Off (the default);
  - In encrypted rooms (while one is open);
  - Everywhere in the app.
- An encrypted room's details also have "Block screenshots in this room", the member's own choice
  (`ChatRoomMember.screenSecure`, an extend-entity in phase 21's file), for when the setting is Off.
- Reasons in phase 16's set (`ScreenSecurity.kt`, through `moquiDesktop.secureScreen(reason, on)`):
  - `room:<chatRoomId>` while such a room is shown. Chat sets it when the room opens and clears it when the room
    closes, when another room opens, and when Chat goes;
  - `app:<account>` from the shell's start, read from the account's preference
    (`moqui.config.preferences.CHAT_SCREEN_SECURITY`), and on the setting's change.

  The flag is on while any reason holds, so `viewOnce` and these work together.
- **The app reason is remembered on the phone.** `ScreenSecurity.kt` keeps the `app:` reasons in its
  SharedPreferences and sets the flag in `attach()` before the first frame. From the next start, the splash and the
  account list are covered too. This is a small Kotlin change, and it needs the next APK.
- On the web and the desktop app, `secureScreen` answers false, and the setting reads "Only the Android app can stop
  screenshots; this browser and the desktop app cannot".

**One-time codes**
- **Found in incoming texts only**: SMS, MMS and RCS into a phone room (and an SMS member's reply, phase 18). Never in
  chat between users. `ChatOtp.js` `detect(text)` (in `Sms.js` `receive()`, before `ingest#Event`):
  - a text of at most 600 characters, with a keyword: `chat@otp-keywords`, by default code, verification, verify,
    OTP, passcode, one-time, PIN, security code, login, sign in, 2FA, authentication, confirm;
  - a candidate is 4 to 8 digits on word boundaries, with one space or dash allowed inside ("123 456", "123-456"), or
    the digits after a `G-` style prefix;
  - not a candidate:
    - part of a phone number (a `+`, a group of 10 or more digits, `(555) 123-4567`);
    - a price or amount (a currency sign or code next to it, a decimal point);
    - a date or time (`11/17`, `12:30`, `2026-11-17`);
    - a year (19xx or 20xx) unless it follows "code" or "is";
    - inside a URL;
  - the candidate nearest a keyword, within 40 characters. Two equally near are ambiguous, and none is taken.

  The result is `{code, strong}`. `strong` means exactly one candidate in the whole text.
- `content.otp {code}` on the message (`ingest#Event`'s item `otp`, a hook beside phase 20's `location`). The timeline
  gives `otp`, and the message shows a chip under the text: "Code 123456 · Copy".
- **Copy in the notification.**
  - The push gets `otp: {code, label}` only when it gets the snippet: rule 5, a plain room with previews on (`chat_push.rs`
    `Content::payload`). `label` is the action's words, "Copy 123456", made by the server in the user's language, since
    the service worker has no `t()`.
  - **Web push** (`sw.js`): `showNotification` gets `actions: [{action: 'copy', title: label}]` where the browser
    shows actions (Chrome on desktop and Android). A service worker cannot write the clipboard, so `notificationclick`
    with `copy` opens or focuses Chat at the message with `copyCode=1`. Chat then writes the code with
    `navigator.clipboard.writeText` and says "Code copied". If the browser refuses that without a tap, the chip's Copy
    is highlighted for one tap. The plain tap opens the message as today.
  - **Android: the current APK cannot do this.** Its actions are fixed in `ChatNotifications.showRoom` and no payload
    field adds one. The Kotlin change is small:
    - `showRoom` reads `otp` and adds "Copy code" (`action()`, a new `COPY` in `ChatActionReceiver`, the code in the
      intent's extras, `FLAG_IMMUTABLE`, an explicit component);
    - the receiver writes it with `ClipboardManager.setPrimaryClip` (`ClipDescription.EXTRA_IS_SENSITIVE` on Android
      13 and later, so the keyboard's clipboard preview hides it) and shows "Code copied" before Android 13, which
      shows its own;
    - a notification with a code is `VISIBILITY_PRIVATE`, with a public version "A code from +1 555…", so the lock
      screen does not show it.
  - **The desktop app**: no button. Its notification plugin has actions, but they need a Rust change and are left out.
    A click opens the message with its Copy.
- **Delete after 24 hours** (`CHAT_OTP_DELETE`, Settings, Chat, Privacy; off by default): a message with a `strong`
  code gets `expireDate` = its time + 24 h when every local member of the room has the setting on. A shared number's
  room keeps it if anyone wants it. Phase 16's worker deletes it on the server and on every device, as any
  disappearing message. The chip then reads "Deletes tomorrow at 15:40 · Keep". Keep (`keep#CodeMessage`, any member)
  clears `expireDate` and pushes the change. The provider and the carrier keep their own copies: the setting says so.

**Send original** (today the client sends every photo as it is, at full size: there is no compression anywhere. So
compressed is a new default, which §19.1 asks about)
- In the composer's file strip, a photo (`image/jpeg`, `image/png`, `image/webp`, a still image) shows
  "Compressed ▾", which can be switched to Original, for that send. The default is compressed.
- `moqui.chat.compressPhoto(file, {maxSide, quality, maxBytes})` (`WebrootChat.js`, its own block):
  - `createImageBitmap(file, {imageOrientation: 'from-image'})` onto a canvas, then `toBlob('image/jpeg', 0.85)`, with
    a long side of 2048 px;
  - a PNG is made smaller as a PNG, and only when it is larger than that (screenshots keep their sharp text);
  - a GIF, a video, an image the browser cannot decode (HEIC on most) and one that comes out larger are sent as they
    are.

  A compressed photo loses its metadata, its GPS position among them. The Original choice says "Keeps the photo's
  details, its location among them".
- **Encrypted rooms**: it compresses before `sealFile`.
- **MMS.** A room that goes out by text reports its limit. `Capabilities` gains `media_max_bytes`; the SMS adapter
  reports its bridge's `maxMediaBytes`: Telnyx 1 MiB, Twilio 5 MB, Sinch 1 MiB. `get#Room` answers
  `textMediaMaxBytes`, the smallest of its outbound bridges'. Then:
  - compressed steps down (quality 0.85, 0.7, 0.55; side 2048, 1600, 1280) until the files fit;
  - Original over the limit is disabled with the reason "Too large for a text (over 1 MB): it goes compressed";
  - the adapter's `mediaTooLarge` refusal stays for what still does not fit.

**Data model.** The preferences `CHAT_SCREEN_SECURITY` and `CHAT_OTP_DELETE` (`get#ChatPrefs`, `store#ChatPrefs`), and
`ChatRoomMember.screenSecure` (new `framework/entity/ChatPrivacyEntities.xml`, in `load-entity`). There is no new
entity: a code is in the message's content, and its deletion is phase 16's `expireDate`.

**Services.**
- `org.moqui.chat.ChatPrivacyServices`, logic in `ChatOtp.js`:
  - `detect#OneTimeCode` (internal, for `Sms.js` and the spec);
  - `keep#CodeMessage`;
  - `set#RoomScreenSecure` (the caller's own member row).
- `get#ChatPrefs` and `store#ChatPrefs` gain the two settings.
- The transitions `keepCode` and `roomScreenSecure` (in `ChatPrivacyTransitions.xml`).

**Screens.**
- Settings, Chat: a Privacy section with the two settings, its own form, so it does not touch the existing form's
  fields.
- Chat: the code chip, the room toggle, and the `secureScreen` calls on room open and close.
- The composer: the photo choice.
- The shell: the `app:` reason at start.
- Phone and desktop alike, but `FLAG_SECURE` and the notification's Copy are Android's.

**Hooks in shared files.**
- `Sms.js`: the detection and the item's `otp`.
- `ChatBridgeServices.xml` `ingest#Event`: `otp` and `expireDate`.
- `ChatServices.xml` `get#Timeline`: `otp`.
- `chat_push.rs`: `otp` in the payload.
- `chat_bridge.rs` `Capabilities.media_max_bytes`, and `sms.rs` reporting it.
- `sw.js`: actions and the click.
- `Chat.qvue`: the chip, the toggle, the room's `secureScreen`, the file strip's choice.
- `Settings/Chat.xml`: the Privacy section.
- Kotlin: `ScreenSecurity.kt` (the remembered `app:` reasons), `ChatNotifications.kt` and `ChatActionReceiver.kt`
  (Copy, the private lock-screen version).

**Bridges and encryption.**
- Codes come only from the SMS, MMS and RCS adapters' inbound texts.
- A code in an encrypted room never exists: an outside number's room is never encrypted.
- Delete after 24 h deletes nothing on the provider or the phone network.
- Screen security is the same in every room kind.

**Failure tolerance.**
- A detection that throws is logged, and the text comes in without a code.
- A push without `otp` (previews off) still shows the message.
- A clipboard write that the browser refuses leaves the chip.
- `secureScreen` failing is logged, and the room opens anyway; the setting shows that it did not hold.
- A photo that cannot be compressed goes as it is.

**Security and abuse.**
- **False positives**: a wrong code adds only a chip and a button. Deletion needs the opt-in, a `strong` detection, and
  every member's setting, and Keep undoes it.
- A sender cannot make someone's message delete itself: only inbound texts are read, and only the recipients'
  settings act.
- The code goes in the push payload only where the snippet already goes. It is encrypted to the device, as the
  snippet is, marked sensitive on the clipboard, and hidden on the lock screen.
- Nothing fills a code in for the person.
- `FLAG_SECURE` hides what the screen shows from screenshots and recordings. It does not stop a camera, and the setting
  says so.
- Compression drops GPS metadata. The original keeps it, and the choice says so.

**Left out.**
- A Copy button on the desktop app and on browsers that show no actions.
- Codes in mail.
- Autofilling a code into another app (Android's SMS Retriever is for the app's own codes).
- Codes in other languages beyond what an operator adds to `chat@otp-keywords`.
- `FLAG_SECURE` on the desktop and the web, which is not possible there.
- Video compression.
- A remembered default for Original.

**Tests (regression).**
- Server:
  - `framework/test/ChatOtp.spec.js`:
    - `detect#OneTimeCode` over a table of texts:
      - found: "Your code is 123456", "G-482913 is your Google verification code", "123-456", "OTP: 9012";
      - not found: a phone number, "$1234", "Pay 1234.50", "on 11/17", "at 12:30", "in 2026", a URL with digits, two
        codes as near, no keyword;
    - `receive#Sms` puts `content.otp` on the message;
    - with every member's `CHAT_OTP_DELETE`, `expireDate` is 24 h on; with one member's only, nothing;
    - Keep clears it;
    - a plain room's `PushOutbox` payload has `otp`, and none with previews off.
  - `BasicCommsChat` (an existing spec, one feature): the two preferences saved and read.
- Rust:
  - `chat_push_tests`: `otp` in the payload only with a snippet;
  - `moqui-channels`: `media_max_bytes` from the bridge's settings.
- Client, offline: `webroot/test/client/WebrootChat`:
  - `compressPhoto` on a generated 4000 × 3000 canvas image: it comes out at 2048 px, as a JPEG, smaller; a small PNG
    is kept; it steps down to a 1 MiB limit;
  - the code chip's line;
  - the `sw.js` notification options with actions (its existing offline check, if there is one; else a helper
    `moqui.push.notificationOptions` in `WebrootPush.js` that both use, checked there).
- Android (on the phone only, §18.4): Copy, the lock screen, `FLAG_SECURE`.
- basic-comms client: nothing.
- No acceptance spec.

**Docs.**
- [Comms](../docs/tools/Comms.md): the three features.
- [Mobile-Client](../docs/ui/Mobile-Client.md): screen security, the Copy action, the remembered app reason.
- [SMS](../docs/interfaces/SMS.md): codes and the MMS limit in the composer.
- The BasicComms README.
- §7.2 (the `otp` field).

**Done when.** The regression specs above pass within their budgets; on a real phone, the row of §18.4.

**Status (2026-11-18): built.** What the build changed from the entry above:
- *A code is found after the text is taken in* (`ChatPrivacyServices.mark#OneTimeCode`, from `Sms.js`, which reads the
  message's text), in place of a detection before `ingest#Event`.
- *The settings* are saved by `ChatPrivacyServices.store#ChatPrivacyPrefs` (Settings, Chat, Privacy, its own form);
  `get#ChatPrefs` answers `screenSecurity` and `otpDelete`.
- *The push* carries `otp {code}` only; the web's action reads "Copy 482913" and Android's "Copy code" (its string
  resource). `sw.js` builds a notification in `notificationOf`, which its page spec checks.
- *The MMS limit* reaches the composer as the room's `bridgeCapabilities.mediaMaxBytes` (`Capabilities.media_max_bytes`,
  the smallest of the room's outbound bridges), not a `textMediaMaxBytes` of `get#Room`.
- *The app reason* is `app`, one for the phone, remembered by `ScreenSecurity.kt`.
- Regression: framework `ChatOtp` (3 features), moqui-core `chat_push_tests` (the code only with previews), moqui-channels
  (an inbound text's code kept), webroot `WebrootChat` (two features) and `WebrootServiceWorker` (one). The Kotlin
  (`ChatNotifications.kt`, `ChatActionReceiver.kt`, `ScreenSecurity.kt`) is not built here: it is checked with the next
  APK on a phone (§18.4). Left: §18.4's row.

**Review fixes (2026-11-18).**
- *Metadata never goes on the compressed path*: every still photo is drawn again, a JPEG or WebP as a JPEG also when that
  is no smaller, a PNG as a PNG (its eXIf and text chunks gone). The message's files share a text's total limit (each
  photo fitted under its share), and a message still over it is not sent and says so.
- *The whole app's screen security applies at once both ways*: the `CHAT_SCREEN_SECURITY` preference applier
  (`WebrootVue.qvt.js`) and Chat's opening hold or drop `app:<account>`; `ScreenSecurity.kt` forgets a reason turned off.
  Several accounts on a phone: any one set to everywhere keeps it on; turning it off there takes only its own away.
- *A code deletes itself only by a core word*: the text's only candidate within 20 characters of code, OTP, verification,
  passcode, security code or one-time; "confirm", "login" and "PIN" find a code for Copy but never delete it
  (`detect#OneTimeCode` `deletable`).
- *A code goes only where the snippet goes*: none with previews off, in an encrypted room or for a disappearing message.
- The flaky `a_ring_to_a_keyed_device_is_sealed` checks only the clear-text fields.

#### Phase 22: unified contacts and the network per message (D4, §11, §17 #1)

Designed 2026-11-17 (§11.1). Chat groups one user's conversations with a person across networks into one person, with
a person card to merge and split and a network button in the composer, best path first. The rooms stay separate, each
with its own label. Mail stays in the Inbox, as a line on the person's rooms. The user's answers of the same day are in
§19.1: one row per room, the app's contacts may link a number to a user, no finding a user by number.

Status (2026-11-18): built, as designed, with these changes:
- the hook's url is kept only when it is a path on this server;
- Mantle answers no url, because the page that shows a contact is an app's;
- the person card's Add lists the user's other people and takes a typed number or address; a user of this server is
  added by their conversation;
- merging two users of this server is refused (`twoUsers`).

Mantle's visibility rule moved to `filter#VisibleContacts`, which `filter#ContactAddresses` and
`find#ContactPeople` share (mr-mantle branch `d4-contact-people`); IronStone's theme row is on mr-boss's branch of the same
name.

The review's fixes (2026-11-18):
- a typed address is matched only against what the user is shown, and joins a user of this server only when already
  shown (`notShown`);
- a hook contact naming a refused, blocking, disabled or external user is left out whole; `get#RefusedUsers` is asked
  with the blocks, and such a user of a room gets no new chat, no nudge and no merge;
- an automatic person's key is its lowest address, so a new message does not change it;
- the hook's answer is kept three minutes per user (`moqui.chat.people.contacts`), so the people and the mail lines
  spend one ask; the `people` answer says when the contacts were away, the page asks again after 30 s, and merge and
  rename of an automatic person refuse meanwhile (`contactsUnavailable`);
- a moved draft is added after the room's own, a draft with files does not move, and a made direct chat's encryption
  is the server's answer;
- the queries are a fixed count, independent of the number of rooms: the rooms (`ChatPersonRoom`, limited in the
  query), their members, mappings, bridges, GroupMe people and latest inbound messages (`ChatRoomInboundLast`, grouped)
  are one query each, and the blocks, the Blocked list, the opt-outs and the text numbers one or two each. Read from
  the code: about 25 for `find#ChatPeople` with 100 SMS, 80 direct and 20 GroupMe rooms, plus the hook's own (none while
  kept) and the apps' `chat-target` providers;
- Mantle matches a +1 number by its area code and its local number's written forms (the area and number index), and
  another country's by its last seven digits, one scan for all of them; the user link stays inside the cap of 10;
- the hook's url refuses a backslash, whitespace and control characters; the caller's messages are kept apart, not
  cleared; a text number on a bridge that is not connected is never the best.

Measured: basic-comms server 0.6 s for `BasicCommsPeople`; webroot client 0.4 s for the whole of `WebrootChat`; the
basic-comms client suite unchanged (see the report).

Left: the checks on a real phone of the network sheet and the card; a GroupMe direct conversation's person (`ext:`)
has no server spec of its own.

**What exists** (read 2026-11-17):
- `crates/moqui-core/src/service/contacts.rs`: `EmailClientServices.filter#ContactAddresses`. It answers only which mail
  addresses are a user's contacts, for an automatic reply. Its hook is named by a property (`email-client@contact-service`) or
  the user's theme value (`STRT_EMAIL_CONTACT_SERVICE`). The hook runs with authorization disabled for the given user,
  in its own transaction, and a failure is logged once. No hook: null, and nothing counts as a contact. Phase 22 reuses
  this machinery (`hook_service`, `call_hook`, `first_time`) for a second service.
- Phase 9's names: `SmsServices.find#PhoneNames` asks the apps' `call-context` providers, only for the numbers of the
  user's own phone rooms, 600 an hour. `moqui.chat.nameRooms` puts the phone's contacts first (read on the phone, never
  sent). These give names only; nothing is grouped.
- mr-mantle: `Party` (a personal contact has `privateOwnerUserId`), `PartyContactMech` to `ContactMech`
  (`CmtEmailAddress` in `infoString`; `CmtTelecomNumber` with `TelecomNumber` `countryCode`, `areaCode`,
  `contactNumber`), the books (`ContactBook`, `ContactBookParty`, `ContactBookShare`) and `UserAccount.partyId` (its
  extend-entity). Mantle's `filter#ContactAddresses` already defines "a contact the user can see": their own personal
  contacts, their books and the books shared with them, and the directory of their organizations, but never another
  user's personal contact. Mantle has no contact mechanism for an XMPP, GroupMe or Signal identity.
- Chat: `ChatExternalUser` is a person on a bridge (GroupMe's user id). Its `linkedUserId` is a bridge's verified puppet
  link and is not used here. A GroupMe direct conversation is the bridge owner's private room, mapped as `dm:<user id>`.
  An SMS or RCS room is a `CrtExternal` pair room `ours/theirs`; one outside number may have a room on each of our
  numbers.
- XMPP is not a separate network for a person. It is C3a: users of this server with an XMPP app, in the same rooms. A
  direct message from an XMPP app goes to the plain direct room (`create#Room`'s default, phase 12), and an encrypted
  room is not served over XMPP. So Moqui chat covers XMPP, and phase 12's rule already sends to the network a user
  uses: a user with only an XMPP app has no crypto device, so their new direct room is plain and reaches the app.
  Outside XMPP addresses wait for C3b.
- **Without mr-mantle** the framework keeps no contacts. A user still gets:
  - the users of this server (people search);
  - their rooms' numbers and GroupMe people;
  - names from the phone's contacts (Android) and from any app's `call-context`;
  - their own merges, splits and names;
  - mail lines from users' account addresses and from addresses the user linked.

  What they do not get is grouping by contact and names of outside people on the web.

**Data model** (`framework/entity/ChatPeopleEntities.xml`, new, listed in `MoquiDefaultConf.xml`):

| Entity, field | What |
|---|---|
| `moqui.chat.ChatPerson` (`chatPersonId` pk, sequenced; `ownerUserId`, `displayName`, `createdDate`; index on `ownerUserId`) | One person in a user's own list, made by a merge, a split or a rename |
| `moqui.chat.ChatPersonAddress` (`ownerUserId`, `addressKey` pk; `chatPersonId`, `createdDate`; index on `chatPersonId`) | An address the user put in one of their persons. The key makes each address belong to at most one person of that user |
| `addressKey` | `user:<userId>`, `tel:<E.164>`, `email:<address in lower case>`, `ext:<chatExternalUserId>` (GroupMe now; Signal, iMessage and C3b's XMPP later) |
| Enumeration `STRT_CHAT_CONTACT_SERVICE` | The theme value that names the contacts hook (`ScreenEntities.xml` seed) |

A person holds at most 20 addresses, and a user at most 2,000 rows.

**Grouping** (`ChatPeopleServices.find#ChatPeople`, `ChatPeople.js`, for the current user only, with no `userId`
parameter):
1. **The user's rooms.** Rooms where the user is a member who has not left and has no filter (phase 11), the 200 newest:
   - a direct room of two gives `user:` the other member;
   - a pair room (one outside number) gives `tel:theirs`, with `fromNumber` ours and the network `sms` or `rcs` from
     the bridge's type;
   - a GroupMe direct conversation gives `ext:` its `ChatExternalUser`.

   Group direct rooms, group texts and other rooms belong to no person.
2. **The user's rows** win. An address with a row is in that row's person, whatever the hook says.
3. **The contacts hook** is asked once, with the rooms' addresses that have no row (at most 300). Each contact it
   answers joins those addresses into one person, and brings its other addresses. It never moves an address that has a
   row.
4. **What is left.** Each address with no row and no contact is a person of its own.

The person key is `p:<chatPersonId>`, else `a:<its first address>`. Each person in the answer has:
- `name` and `nameFrom` (`mine`, `user`, `contact` or none);
- `edited`, `userId` and `contactUrl` (the app's record);
- `addresses`, the shown ones (below);
- `rooms`, each `{chatRoomId, network, encrypted, fromNumber, lastMessageDate, lastInboundDate, notConnected}`;
- `ways` and `best` (below).

`roomPerson` maps each room to its person key.

**The contacts hook** (`contacts.rs`, `ChatPeopleServices.find#ContactPeople`, a Rust service, `allow-remote` false):
- It is named by the property `chat@contact-service`, else the theme value `STRT_CHAT_CONTACT_SERVICE`. It is called
  as `filter#ContactAddresses` is: authorization disabled, for the given user, in its own transaction.
- In: `userId` and `addressList` (`tel:`, `email:` and `user:` keys).
- Out: `contactList`, each `{contactRef, name, url, addressList}`: the contacts the user may see that have one of the
  addresses, each with its own `tel:`, `email:` and `user:` addresses.
- The framework normalizes the answer: valid keys only, at most 100 contacts and 10 addresses each.
- No hook: `contactList` is null. A hook that fails is logged once by name (`hookFailed`), and the people come from the
  rooms and rows alone.
- mr-mantle answers with `mantle.party.ContactBookServices.find#ContactPeople`:
  - who counts as a contact: the visibility of its `filter#ContactAddresses`, shared as one internal function;
  - phones: matched in E.164 by their last seven digits and then the whole number, as IronStone's `get#CallContext`
    does;
  - mail: any case;
  - `user:`: the party of `UserAccount.partyId`.

  mr-boss names it with a `STRT_CHAT_CONTACT_SERVICE` row in `BossSetupData.xml`, beside the mail hook's row.

**Who sees which address:**
- **An outside person.** The card shows the numbers of the user's own rooms, the user's own rows, and the hook's (the
  app's contact, which the user can already see). Mail addresses show the same way.
- **A person who is a user of this server.** The card shows only the numbers and addresses of the viewer's own rooms
  and rows, plus `@username`. It never shows what the hook or the user account gave: chat reveals no user's phone or
  mail address (§14.4). The account's mail address is used only to find the mail line.
- **Linking a number to a user.** The hook may do it, through an app contact the viewer can see: IronStone's directory
  already shows an employee's phone to the people who can see it. Chat never links a number to a user from its own or
  voice's data: not `CallHandlingSetting`'s mobile, not caller-ID grants, not ring groups.
- **Refused users.** A user that `get#RefusedUsers` refuses with the blocks (an app's `chat-target`, or a user who
  blocked the viewer), a disabled user and an external user are as users that do not exist. A hook contact that names
  one is left out whole: no name, none of its other addresses, so the person shows only as the viewer's own room
  addresses. A room the viewer already has with them still shows as it does today, with no new direct chat, no nudge,
  and no merge of that user.
- **Nothing is shared.** No service reads another user's rows. A merge changes only the user's own rows, and the other
  person sees nothing.

**Merge and split** (`ChatPeople.js`, as the current user, each in one transaction with the user's `UserAccount` row
locked):
- `merge#ChatPeople` (`personKeyList` and/or `addressKeyList`): makes one `ChatPerson` holding every address of the
  persons named, the hook's group included, so the person stays one when the contact changes. Other persons' rows move
  into it, and persons left empty are deleted. Each address must be the user's to name:
  - `user:`: a user they may reach (enabled, not an external user, not refused);
  - `tel:`: a valid number (`normalize#PhoneNumber`);
  - `email:`: a valid address;
  - `ext:`: the person of a GroupMe direct conversation the user is in.

  Past the limits it refuses with `tooManyAddresses` or `tooManyPeople`. A typed number or address is never sent to the
  hook, because an address with a row is not asked about, so typing cannot probe the app's contacts.
- `separate#ChatPersonAddress` (`addressKey`): the address alone in a person of its own, so the hook does not join it
  back.
- `rename#ChatPerson` (`personKey`, `displayName` up to 100 characters; empty clears it): makes the person's rows when it
  had none.
- `reset#ChatPerson` (`personKey`): deletes the user's rows of the person, which goes back to automatic.

**The ways and the best path.** `find#ChatPeople` answers each person's `ways` in order. Each way is
`{kind, network, chatRoomId, userId, number, fromNumber, address, url, encrypted, blocked, reasonCode}`. The kinds:
- `room`: an existing room;
- `direct`: Moqui chat with no room yet;
- `text`: a new text, `open#PairRoom` from `fromNumber`;
- `mail`: the Inbox (the latest thread, or Compose with `to=` for an outside person's address).

The best way (`best`) is the first of these:
1. **Moqui chat**, for a person who is a user of this server and whom the viewer has not blocked: their encrypted
   direct room, else their plain one (phase 12: a plain direct chat that exists keeps opening, and its header offers
   "Start an encrypted chat"), else a new direct chat (`createDirect` with no `encrypted`, so the server decides; the
   label comes from `encryptionDefault`, as a dm link's).
2. **The network they use**: of their rooms on other networks (RCS, GroupMe, SMS), the connected one with their latest
   message to the user (`lastInboundDate`).
3. **SMS**: the pair room of their number with the latest message from either side; else a new text from the user's number
   (`find#MyTextNumbers`: their only number, or the first, with the From choice). RCS is never the default for a
   new conversation, because it comes from the brand agent's name, not the user's number (§19.4 Q1).
4. **Mail**: their latest thread, or Compose to their address.

These are listed but never best, each with its reason:
- a number on the user's Blocked list (phase 11, `CallHandling.js` `blockedUsers`);
- a number that sent STOP (`check#SmsSend`'s reason);
- a bridge that is not connected or is disabled.

A person the user blocked in chat (`ChatUserBlock`) has no way: their rooms are filtered and out of the list, and the
card offers Unblock.

**Separate rooms, with a switcher.** One merged timeline was weighed and left out:
- it would mix encryption in one view, an end-to-end encrypted message beside one the server and a bridge read;
- read markers, paging, search, threads, edits and deletes are per room, and each network supports a different set
  (SMS has no edits);
- the other person sees separate threads anyway.

So it is neither cheap nor safe. With separate rooms, each label stays true.

**The network button** (`m-chat-networks`, `WebrootChat.js`):
- **Where.** At the start of the composer of a person's room: a direct room of two, a pair room, a GroupMe direct
  conversation. It shows the room's network icon.
- **What it lists.** The person's ways, best first and marked "Best". Each shows its network, the number it texts from
  or the address, and its label from `moqui.chat.e2eeLabel` (§6.2): Encrypted, Not end-to-end encrypted, Text message,
  or the bridge's name. A blocked way shows dimmed, with its reason.
- **Choosing a way** opens that room, or makes it (`createDirect`, `textRoom`), and moves the draft's text there. Files,
  a reply and a thread stay behind. The new room's banner and composer hint show its label. A move from an encrypted
  room to one that is not says so once: "Moved to {network}: not end-to-end encrypted".
- **A nudge.** In a person's SMS, RCS or GroupMe room, when the person is a user of this server, a line above the
  composer offers "{name} is on MR Moqui: chat end-to-end encrypted", or "Chat in MR Moqui" when `encryptionDefault`
  is false, with Switch. It can be closed for that room (browser storage).
- **A message's network.** A room is one network, so its messages carry the room's label: the header, banner and
  composer hint (§6.2). An RCS room's message shows its path (phases 10 and 13), and Info says where a bridged message
  came from (built). Nothing per message is added.
- **Mail** opens the Inbox, never a chat timeline (§19.1).

**The person card** (`m-chat-person`, `WebrootChat.js`) sits at the top of the People tab of a person's room. It shows:
- the name, with Rename, and the `@username` of a user;
- a link to the contact's record, when the hook gave one (`contactUrl`);
- each way as a row, with its label, last activity, and Open or Start;
- the mail line.

Its actions:
- **Add** opens the new-chat picker (people, numbers, the phone's contacts, a typed number or address) and merges the
  choice in. A room that already belongs to another person is merged by picking it.
- **Not {name}**, on each address, separates it.
- **Undo my changes** resets the person, when it is edited.

On a phone the card fills the details pane and its actions open as a sheet. On a desktop it is in the 360 px side pane.

**The list.** Rows stay one per room, each with its network's icon and label. A person's room shows the person's name,
taken from the first of:
1. the user's own name for the person;
2. the user's name, for a person who is a user of this server;
3. the phone's contacts;
4. the hook's contact;
5. `call-context`;
6. the number.

`moqui.chat.applyPeople(rooms, people)` sets these names after `nameRooms`. Folding a person's rooms into one row is
left out (decided, §19.1).

**Mail lines.** `find#DirectChatMail` gets its people from `find#ChatPeople` (an internal call):
- for a user of this server: their account's address and the addresses the viewer linked;
- for an outside person: the hook's addresses and the viewer's rows.

The line shows on each of the person's rooms. The limits stay: 180 days, 300 messages.

| Work | Files |
|---|---|
| `ChatPerson`, `ChatPersonAddress`; the load line | `framework/entity/ChatPeopleEntities.xml` (new), `MoquiDefaultConf.xml` |
| `find#ContactPeople`: the hook by property or theme value, its answer normalized, a failure logged once; the hook lookup shared with `filter#ContactAddresses` | `crates/moqui-core/src/service/contacts.rs`, `ScreenEntities.xml` (the enumeration) |
| `find#ChatPeople`, `merge#ChatPeople`, `separate#ChatPersonAddress`, `rename#ChatPerson`, `reset#ChatPerson` | `framework/service/org/moqui/chat/ChatPeopleServices.xml`, `ChatPeople.js` (new) |
| Mail lines from the people | `basic-comms/service/org/moqui/comms/ChatScreenServices.xml` (`find#DirectChatMail`) |
| Transitions `people`, `mergePeople`, `separatePerson`, `renamePerson`, `resetPerson` | `Chat.xml` |
| `moqui.chat.people` (read once a page and shared, as `mailLines`), `applyPeople`, `personOf`, `waysOf` and `wayLabel` (labels and order), `openWay`, `moveNote`, `nudgeWay`, `changePeople`; `m-chat-networks`, `m-chat-person` | `WebrootChat.js` |
| Small hooks: the network button in the composer, the card in the People tab, the nudge, `applyPeople` after `nameRooms` | `Chat.qvue`; `Panel/ChatPanel.qvue` (names only); the shell's bundle regenerated (`bundled-screens.js`) |
| Mantle's hook and IronStone's theme row | mr-mantle `service/mantle/party/ContactBookServices.xml`; mr-boss `data/BossSetupData.xml` |

- **Phone and desktop.** The same components on both:
  - the network button is a bottom sheet on a phone (as the message actions are) and a menu above the composer on a
    desktop;
  - the card fills the details pane on a phone and sits in the side pane on a desktop.

  The Android and desktop apps get them through the bundled Chat.
- **Terminal.** No change: it lists each room as today.
- **Bridges and encryption.** No adapter changes. A way's label is its room's. An encrypted room is never bridged
  (phase 12), so a move to a bridge always leaves end-to-end encryption, and says so.
- **Failure tolerance.**
  - The hook failing or timing out: logged once, and the people come from the rooms and rows.
  - A room whose mapping cannot be read: logged, and that room is a person of its own.
  - The `people` request failing: no network button or card, and the rooms work as today.
  - A way that cannot open (a `textRoom` refusal, a refused `createDirect`): its reason shows, and the draft stays where
    it was.
  - Mail lines: as today, read separately.
- **Security and abuse.**
  - Only the current user's people; no other user's rows are read, and nothing is pushed to anyone.
  - The hook is asked only about the addresses of the user's own rooms, at most 300 at a time, and at most 60 times an
    hour per user. Past that the people come without it (`contactsSkipped`).
  - The hook enforces the app's visibility itself, as the mail hook does.
  - No user's phone or mail address is shown through chat. A number is linked to a user only by an app contact the
    viewer can see.
  - A `user:` address is checked by `get#RefusedUsers`, so a user who blocked the viewer looks like a user that does not
    exist.
  - Names are plain text and limited in length; the merge's limits bound the rows.
- **Left out.**
  - One merged timeline, and folding a person's rooms into one list row.
  - Finding a user by their phone number (Signal's "Who can find me by number"): a user's numbers stay out of chat.
  - Writing a merge into the app's contacts (Mantle's personal contact), which belongs with the Contacts screen and its
    sync (§14.8, §17 #17).
  - Grouping numbers by the phone's contact entries, which needs a contact key from `PhoneBook.kt`.
  - Outside XMPP (C3b), Signal and iMessage identities: `ext:` takes them when their adapters come.
  - Starting a new GroupMe direct conversation; the palette's outside people; a network the user pins per person.
- **Tests (regression).**
  - Rust: `contacts.rs` unit tests for normalizing the answer (key forms, repeats, limits), `cargo test -p moqui-core
    contacts`.
  - Server: `basic-comms/test/BasicCommsPeople.spec.js` (new), with a test hook
    (`test/service/moqui/basiccommstest/PeopleTestServices.xml`, named by `chat@contact-service` in the component's
    `test/MoquiConf.xml`). It covers:
    - each kind of room's person;
    - a merge of a phone room with a user, whose best way is then Moqui chat (the encrypted room first);
    - separate, rename and reset;
    - the hook joining a number with an address, whose mail line then shows on the phone room;
    - a user's person not showing the hook's numbers;
    - a refused user dropped;
    - another user's `find#ChatPeople` not seeing the merges;
    - the network they use (the latest message from them on RCS) and SMS from the room's number;
    - a blocked number not best;
    - merge refusing an `ext:` of someone else's room, a bad number and the 21st address;
    - a typed number never reaching the hook.

    `BasicCommsDesktop.spec.js` gains one `find#DirectChatMail` case: an outside person's linked address gives their
    phone room a line.
  - Client, offline: `webroot/test/client/WebrootChat` covers `waysOf` (the order, the labels, a blocked way),
    `applyPeople`'s order of names, `openWay`'s posts, `moveNote`, and when the nudge shows.
  - mr-mantle: `test/party/ContactPeople.spec.js` (new) covers the visibility (another user's personal contact never),
    a number matched in E.164, an address in any case, and `user:` through `UserAccount.partyId`.
  - No basic-comms client spec, because its budget is at 10.8 of 12 s. The two components' logic is in the helpers
    above. No acceptance spec.
- **Docs.**
  - [Comms](../docs/tools/Comms.md): people across networks, the network button, the card, the hook.
  - [Chat-Bridges](../docs/interfaces/Chat-Bridges.md): the `ext:` address of a bridge's person.
  - [Mobile-Client](../docs/ui/Mobile-Client.md) and [Desktop-Client](../docs/ui/Desktop-Client.md): one line each.
  - The BasicComms README, and mr-mantle's README: the hook.
- **Done when.** The regression specs above pass within budget. By hand on the web and a phone width: a person with
  Moqui chat and SMS has one card, the network button moves a draft to SMS and says it is not encrypted, and a start
  opens the encrypted direct room.

#### Phase 10 and later

Each of these needs its design first:

| Work | Design needed |
|---|---|
| `sim-sms` | §10: MMS, a synced SIM room and its sends, SIM threads beside server rooms |
| `sim-dialer` and the contacts sync | §14.7, §14.8; what a server without mr-mantle offers |
| Encrypted polls and events together | Answers as encrypted messages, counted on the devices; until then an encrypted room has neither (phases 19, §19.1) |

Then the other networks and the rest of §17, in this order:
1. The number on the SMS and SIP provider, block and spam (phase 11, designed), contacts sync (the user stops using Google Messages and Cheogram).
2. Default phone app and the merged call log (the user stops using the Phone app).
4. Signal adapter.
5. iMessage adapter (needs a Mac).
6. Disappearing messages (phase 16, designed); scheduled send and reminders (phase 15, designed); invite links and
   join requests (phase 17, designed); SMS members of a room (phase 18, designed).
7. RCS: business RCS in the SMS provider modules is phase 10 (Twilio and Sinch first), its rich features phase 13;
   then C6c and the personal RCS bridge if §13.4 picks B or C.
8. The rest of §17.

**Done when** for each network: the user sends and gets text, a photo and a reaction with a real person on that network,
and the room shows the right network and E2EE label.

### 18.4 Checks on a real phone

These cannot be shown by a browser at phone width. They are collected here for each APK.

| After phase | Check |
|---|---|
| 1 | The keyboard does not cover the composer. A long press vibrates. Back closes a sheet, a pane and the room in order |
| 2 | The soak cases of §7.6. Reply and Mark read from a notification. A notification after a reboot |
| 4 | The rows of §8.2 between two phones and with the desktop app. A voice call on the earpiece with the screen off at the ear; a video call on the speaker; a Bluetooth and a wired headset taking the call at once and giving it back; the audio button's choices. Home during a video call: picture-in-picture with the video only; Home during a voice call: the call goes on with its microphone (and a video call's camera) for minutes, screen off. A second call while in one: it rings, Answer holds the first, Swap, and a car's or watch's hold. Move a call from the desktop to the phone and back with no gap. The lock screen. An encrypted call on the phone's WebView. Opening a server page or switching account mid-call (the native call ends with the page). The held party hanging up (the native call on hold goes). A Bluetooth headset connected at the call's start. Chromium WebRTC's own audio routing against Telecom's. A car's or watch's mute, and what a car or a watch shows of a call; a second call shown answered on a car or a watch once the first is held; the screen at the ear after the app goes to the back and comes back; a renderer crash mid-call |
| 5 | Wi-Fi to mobile data during a call |
| 7 | A voice note recorded and played |
| 9 | The calling account turned on in Calling accounts; a number dialed in the phone's dialer and from Contacts with MR Moqui is filled in on the app's Dialer and placed on Call (and with the app closed, and the phone locked); the pick of an account with two that call; `*67`; an emergency number stays on the SIM; what a car or watch shows of the cancelled Telecom call. Names from the phone's contacts on phone rooms and in the new-chat search, the permission's question at the first search, and names after allowing contacts in Android's settings. A group MMS between three real phones: one room, a reply all of them get, and each one's receipt (each provider's group receipts are from its documentation), on Telnyx and on Twilio |
| 12 | First sign-in on a new phone with the Android app: the device made when Chat opens (and how long the module takes to load), Set up this device in an encrypted room, the recovery key typed and pasted, then on another new phone Verify with the desktop by emoji, older messages readable after each; My devices in the app at the phone's width, with the keyboard up and in dark mode; a direct chat started from the phone with a person who has a device is encrypted, and with one who has none is plain |
| 15 | A long press on Send opens Send later (and its vibration); a message scheduled from the phone in a plain room arrives at its time with the app closed; one scheduled in an encrypted room goes at its time with the app open, and when the app next opens if it was closed (sent late); a reminder's notification at its time, with the app closed and during do not disturb (held, then shown); the current APK shows a reminder's title only, and opens the app, not the message |
| 16 | A view-once photo and video opened in the Android app: a screenshot and the recent-apps thumbnail are blank while it is open and work again after it closes; a notification of a message in a room with a timer shows no text; a 30-second message gone on the phone and the desktop at the same time, and from the phone's offline store after it was asleep past its time |
| 17 | The join page in a phone's browser: Sign in and back to the page, Join, Ask to join with a note; Share from the invite dialog; the desktop's QR code scanned by the phone's camera; "Open in the MR Moqui app" opening the page in the app's account, with one account and with two |
| 18 | A text member on a real phone: the welcome text, a message each way with the sender's name, a photo, the replies of two text members reaching each other, HELP, and STOP; on Telnyx and on Twilio |
| 19 | Add to calendar opens the phone's calendar app filled in (time, zone, place); an event's reminder at its time; an `.ics` from the web imported into the phone's calendar, then imported again after a change, updating the same entry |
| 20 | The location permission's question at the first Use my current location, its fix and accuracy indoors and out, Deny and then Enter it; Open in maps hands `geo:` to the maps app (with a label, and an address only); a sticker sent and received as MMS (PNG, GIF and an animated WebP) on Telnyx and Twilio; the sticker sheet at the phone's width |
| 21 | Screen security: a screenshot and a screen recording blocked in an encrypted room, then allowed after it closes; Everywhere covers the splash and the account list after a restart, and the recents thumbnail is blank; together with a view-once photo open. A text with a code: Copy code in the notification puts it on the clipboard (the keyboard hides it as sensitive), the lock screen shows "A code from …"; delete after 24 h removes it from the phone. A photo from the camera sent compressed (its size and that the GPS is gone) and as the original; a photo into a phone room fitted under the MMS limit |

## 19. Decisions and open questions

### 19.1 Decided

| Date | Decision |
|---|---|
| 2026-11-16 | The user's number moves to an SMS and SIP provider: Twilio, Sinch or Telnyx (§13.4, choice A). Cheogram's phone number role ends; its XMPP role stays |
| 2026-11-17 | Standard specs where they reach (SIP for calls), and no one provider at the centre: the SMS adapter is a neutral core with a module per provider (Telnyx `sms`, Twilio `sms-twilio`, Sinch `sms-sinch`). Sinch on its Conversation API, with no group texts; Twilio's group texts through its Conversations API. A secret-signed webhook is verified on the core node (§9) |
| 2026-11-17 | Text members of a room (phase 18): one room per outside person on each of our numbers marked for room texts. A further room on a number is refused (`noFreeNumber`), and the add dialog says so: "This person is already a text member of <room> on this number; another number is needed." While they are a member, a send from the pair room of that number is refused (`inRoomByText`). The name a moderator gives a text member is the room's alone |
| 2026-11-17 | Business RCS goes through the same provider modules, Twilio and Sinch first; Telnyx RCS is optional (§16.4, §19.4 Q9) |
| 2026-11-17 | A reply to an RCS fallback text goes to the RCS room when the fallback number is that RCS bridge's own, and to the number's SMS room otherwise (§16.4 step 11, phase 10) |
| 2026-11-17 | Scheduled send in an encrypted room is held by the sender's own device: it keeps the draft sealed under its store key and encrypts and sends it at the time, claiming the server's placeholder (no text) so one device sends it; if no device is open then, it goes when that device next opens, marked sent late; a device that is gone leaves it "can't be sent". Plain, phone and bridged rooms use the server-held send. Reminders are held during do not disturb (phase 15) |
| 2026-11-18 | A photo goes compressed by default (2048 px JPEG, its metadata and GPS removed), and Original is a per-send choice; this changes the earlier behaviour, when every photo went at full size (phase 21) |
| 2026-11-18 | An encrypted room has no events for now, as it has no polls; encrypted polls and events together are a later design (answers as encrypted messages, counted on the devices) |
| 2026-11-18 | The `chat-calendar` provider of mr-boss is built with phase 19 (mr-boss branch `d2e-chat-calendar`) |
| 2026-11-17 | Sticker packs now, local only: the server's and each user's, stored as content; GIF search waits until a provider is chosen (phase 20) |
| 2026-11-17 | A location message has no map: coordinates or an address, a label and "Open in maps" (`geo:`, OpenStreetMap on the web), from the device's position or typed in, with no geocoding service; live location waits for the Maps app (phase 20) |
| 2026-11-16 | The layout thresholds stay as built: 1024 px for the stacked layout, 640 px for bubbles, 900 px for a pane over the timeline (§1) |
| 2026-11-16 | Room rows keep the built swipes: right pins, left archives. Mark read is in the long-press sheet (§2.1) |
| 2026-11-16 | Every call in an encrypted room is E2EE, direct rooms included. No fallback to a plain call (§6.1 rule 4) |
| 2026-11-16 | Mute keeps its built meaning: ordinary messages are silent, a mention still notifies. The level Nothing is fully silent (§7.2 rule 1) |
| 2026-11-16 | A phone notification for an encrypted room shows the room and sender only; the text shows when the app opens. Decryption in the background is a later design (§6.5, §7.4) |
| 2026-11-16 | Mod+K stays the shell palette's key. Chat adds to its sources and adds no key listener of its own (§3) |
| 2026-11-16 | No chat component moves to a plain `.js` file. Helpers go to `moqui.chat`; a new shared component is an `m-` component (§2.2) |
| 2026-11-16 | E2EE by default: a new direct chat or private group is encrypted when every person in it has a crypto device, else plain with the plain banner. The plain direct rooms that exist stay; the picker opens the encrypted room when a person has both (§6.1 rule 5, §6.5) |
| 2026-11-17 | Chat makes the browser's crypto device when it opens (idle time after the room list shows), on the web and in the apps' bundled Chat, so E2EE by default applies (§6.5) |
| 2026-11-17 | A plain direct chat that exists keeps opening from Start, though everyone in it now has a device; its header and details offer "Start an encrypted chat", which makes or opens the encrypted direct room, linked back to the plain one. Only people with no direct chat yet get an encrypted one by default (§6.5) |
| 2026-11-16 | Search in an encrypted room: none at first, said in the pane before a query. Later, in memory over the messages the device has decrypted; nothing decrypted is written to disk for search (§6.5) |
| 2026-11-16 | A phone notification for an encrypted room keeps the room and sender only, with no later design for its text. Revisit only from daily use (§6.5, §7.4) |
| 2026-11-16 | A reaction to my message shows in Activity, with no push (§7.2 rule 4) |
| 2026-11-16 | SIM texts stay on the phone by default, with a per-user opt-in switch to sync them to the server as rooms, both in the first `sim-sms` version (§10) |
| 2026-11-16 | One-to-one calls go peer to peer first, between two users of this server in a direct room, with "Always relay my calls" and the media server as fallback; a third person moves the call to the media server (§8.3 item 15, phase 5) |
| 2026-11-16 | UnifiedPush is in the first release, in phase 2 beside FCM (§7.4, step 2h) |
| 2026-11-16 | Mail stays in the Inbox. A person's latest mail thread shows as a line on their direct chat's row and opens in the Inbox; no mail-only rows (§3, phase 8). If mail ever joins as rooms, the rules of §18.2 "Mail stays working" come with it |
| 2026-11-17 | Unified contacts keep one row per room: a person's rooms on each network are rows of their own, each with the person's name and its network's label; they are not folded into one row (§11.1, phase 22) |
| 2026-11-17 | An app's contacts may link a phone number to a user of this server, only through a contact the viewer can already see in the app (Mantle's books and directory). Chat never links a number to a user from its own or voice's data (§11.1, §14.4, phase 22) |
| 2026-11-17 | No finding a user of this server by their phone number: a user's numbers stay out of chat (§11.1, phase 22) |
| 2026-11-16 | During Do not disturb a call rings from a person on the user's allowed list (Chat settings, beside the hours), or as the same person's second call within 3 minutes (§7.2 rule 4) |
| 2026-11-17 | Disappearing messages: the timer starts when a message is sent, for everyone, not when it is read (§18.3 phase 16) |
| 2026-11-17 | A timer is refused in a bridged, phone or federated room, and such a room cannot be bridged while it has one: a bridge cannot delete on the other network (phase 16) |
| 2026-11-17 | View-once: an unopened one is kept 14 days; an open gives a 2-minute window with at most 3 downloads (phase 16) |
| 2026-11-17 | Expiring messages are never put in OpenSearch's index; without OpenSearch the database search finds one while it stands (phase 16) |

### 19.2 Open: the app

None. The questions asked here are decided in §19.1.

### 19.3 Open: other networks

- §13.4: number on the provider only (A), or keep a SIM line for personal RCS (B or C)?
- Which provider holds the user's number: Twilio, Sinch or Telnyx? None is tested live yet (§9).
- iMessage: is there a Mac that can stay on? Without one, iPhone users get SMS.
- Signal: accept that the server can read bridged Signal messages? The alternative is to keep the Signal app for those
  chats.
- Sidecars (signal-cli, BlueBubbles, mautrix): on the same server, or on a separate machine?
- Stories: wanted?
- After the port, does the phone keep a SIM line (for carrier calls and emergency calls), or only data?

### 19.4 Open: RCS

1. Do you accept that RCS from the server comes from a business agent name, not your own number?
2. Will you pay the provider's brand vetting and carrier launch fees (Telnyx: $100 vetting, $500 T-Mobile launch;
   Twilio's and Sinch's onboarding figures differ by source, Appendix A)?
3. Which agent use case fits: Transactional or Multi-use?
4. Which countries do you need? Telnyx API onboarding is US only today; check Twilio's and Sinch's.
5. Do you want RCS phase 3 (personal RCS bridge)? It has ToS risk and AGPL code.
6. If yes, do you accept a separate AGPL sidecar repo?
7. If yes, is it acceptable that the server stores bridged RCS as plaintext?
8. For `sim-sms` users, should the app warn and turn off RCS, or should it keep Google Messages as default and use only the hand-off?
9. Is a Twilio or Sinch adapter needed, or is Telnyx enough? Answered: Twilio and Sinch first, in the provider modules
   their SMS adapters already have; Telnyx RCS is optional (§16.4, §19.1).

The prices, dates and platform rules in §16 and Appendix A come from the sources in Appendix B as read on
2026-11-16; the Twilio and Sinch RCS API facts were read on 2026-11-17. They were not checked again in the revision.
Check the source before a decision that depends on one.

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

**Twilio and Sinch RCS API shape** (read 2026-11-17):

| Function | Twilio | Sinch (Conversation API) |
| --- | --- | --- |
| Sender | An RCS sender added to a Messaging Service's sender pool. | The `RCS` channel on the Conversation API app. |
| Send | `POST /2010-04-01/Accounts/{AccountSid}/Messages.json` with `MessagingServiceSid` and an E.164 `To`, as for SMS. | `POST /v1/projects/{project}/messages:send` with an `RCS` channel identity. |
| Fallback | Automatic SMS or MMS from the service's numbers when the phone cannot take RCS. A `To` of `rcs:+1…` turns it off. | `channel_priority_order` `["RCS","SMS"]`; a fallback shows as `SWITCHING_CHANNEL` in the delivery report. |
| Rich content | `ContentSid` (Content Templates). | The API's rich message kinds. |
| Status | Status callbacks as for SMS; they include `read`. | `MESSAGE_DELIVERY`: `QUEUED_ON_CHANNEL`, `DELIVERED`, `READ`, `FAILED`, `SWITCHING_CHANNEL`. |
| Signature | `X-Twilio-Signature`, HMAC-SHA1 with the Auth Token. Same as its SMS. | `x-sinch-webhook-signature`, HMAC-SHA256 with the webhook secret. Same as its SMS. |

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
- Twilio, send an RCS message: https://www.twilio.com/docs/rcs/send-an-rcs-message
- Sinch, Conversation API RCS channel: https://developers.sinch.com/docs/conversation/channel-support/rcs
- Sinch, Conversation API callbacks: https://developers.sinch.com/docs/conversation/callbacks
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
