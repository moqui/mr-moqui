# Comms Messaging Plan

Plan, 2026-11-16. Nothing built. One plan for the Comms chat as a messaging app, for one app for all messaging, and
for RCS.

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
3. **RCS** (§16): what is possible, options and phases.
4. **Work and order** (§17–§19): new work, phases, open questions. Sources in §20.

| Part | Status |
|---|---|
| §2–§5: phone layout, desktop gaps, reactions, details tabs | Ready to build |
| §6: E2EE policy and labels | Labels ready to build; E2EE by default needs a design (§6.5) |
| §7: notifications | Ready to build; payload encryption reuses the Web Push scheme |
| §8: voice and video calls | Ready to build; the network fixes are ordered by the §8.4 test results |
| §9: the user's own phone number | Most of it is built; the gaps are listed |
| §10: the phone's SIM texts (`sim-sms`) | Needs a design |
| §11–§15: other networks and app equivalents | Plan; each network needs an adapter or a bridge |
| §16: RCS | Research done; business RCS through Telnyx is ready to build |

# Part 1: The app

## 1. Where we start

**Built** (BasicComms `Chat.xml` and `Chat.qvue`, the mobile app in `crates/moqui-mobile`):

| Area | What exists |
|---|---|
| Messages | Send, edit (24 h), redact, reply, threads with a thread pane, forward, pins, polls, mentions, drafts, link previews, scheduled-state icons (pending, failed, sent) |
| Reactions | `react` transition; aggregated per message as `[{key, count, userIds, mine}]`; six quick reactions and a small emoji list in `Chat.qvue` |
| Read state | Read markers (`markRead`, `markAllRead`); read receipts from each member's `lastReadEventSeq` (`prefs.readReceipts`) |
| Files | `upload`; `files` transition over `find#RoomFiles` (`contentTypePrefix`, `searchText`, `limit`, `beforeEventSeq`); a files tab with All and Pictures |
| Search | `search` over `search#ChatEvents`; "Find a room or person" |
| Desktop layout | Sidebar with filters (All, Unread, Mentions, Direct, Rooms, Records), a Pinned group, unread and mention counts, presence and status, hover toolbar, Up arrow edits |
| Encrypted chat | Megolm rooms, cross-signing, emoji and QR verification, key backup ([Chat-Keys](../docs/interfaces/Chat-Keys.md)). Opt-in for direct and private rooms. The desktop shell owns the crypto device ([Desktop-Client](../docs/ui/Desktop-Client.md#chat-crypto)) |
| Calls | WebRTC with an SFU; SFrame E2EE with `requireE2ee` or the room's `callE2eeRequired` ([WebRTC](../docs/interfaces/WebRTC.md)) |
| Phone number | Telnyx SMS and MMS ([SMS](../docs/interfaces/SMS.md)); SIP trunks, `place#PhoneCall`, the Dialer, voicemail ([SIP](../docs/interfaces/SIP.md)). One room per outside number holds its texts, calls and voicemail |
| Mobile push | FCM through `PushService.kt` and `registerClientPush`, for the call ring only ([Mobile-Client](../docs/ui/Mobile-Client.md)) |
| Mobile shell | Bundled `Root.qvue`. Home is `Panel/ChatPanel.qvue` in a card. A room opens `Chat.qvue`'s `chat-room` in `compact` mode |

**Components in `Chat.qvue`** (2856 lines, one file): `ChatRoom` (room view, thread pane, details),
`ChatMessage`, `ChatComposer`, `ChatEdit`, `ChatPending`, and the root (sidebar plus room).

**Problems on a phone (seen on a real device):**
- The room list sits in a card inside the shell, so it uses a small part of the screen.
- "New message" is a small inline "To:" box, not a full-screen picker.
- The room view is the desktop view made small: no bubbles, and the composer is not fixed above the keyboard.
- No long-press menu. The hover toolbar does not work on touch, so reactions, reply, copy and forward are hard to reach.
- The files tab has only All and Pictures. No links, voice, or reactions list.
- No push for chat messages: a message only shows when the app is open.

## 2. Phone layout

### 2.1 Design

The screen width selects the layout: below the Quasar `md` breakpoint (1024 px) is the phone layout.
`moqui.chat.layout()` returns `'phone'` or `'desktop'`. It reads `$q.screen` and updates on resize. Every part reads it.

| Area | Phone layout |
|---|---|
| Home | The room list at full size, not a card. Search bar on top. Start chat button (FAB) at the bottom right |
| Room row | Avatar, name, last message, time, unread badge, E2EE or SMS icon. Swipe left: archive. Swipe right: mark read or unread |
| New chat | Full-screen picker: search people and phone numbers, select one or several, Start. Uses `searchUsers` and `createDirect`, or `createRoom` for a group |
| Room header | Back, avatar, name, E2EE status icon (§6), call and video buttons, menu |
| Messages | Bubbles. Own on the right in the primary color. Others on the left with the avatar in groups. Date chips. Group by sender within `GROUP_MILLIS` |
| Reply | Swipe right on a bubble. The quote shows above the composer and above the sent bubble. Tap a quote to jump |
| Composer | Fixed at the bottom, above the keyboard: attach (camera, gallery, file), emoji, text, then a send button that becomes the voice button when empty |
| Back | The Android back gesture closes the open layer: menu, then details, then the room |

### 2.2 Work

1. Move `ChatMessage`, `ChatComposer`, `ChatEdit` and `ChatPending` to `BasicComms/Chat/components.js`, so
   `ChatPanel.qvue` and `Chat.qvue` share them. Add it to the bundled screens list.
2. Add a `layout` prop to `ChatMessage`: `bubble` or `row`. The data and actions stay the same.
3. Rewrite `Panel/ChatPanel.qvue` as the phone home: full-height list, FAB, full-screen new-chat picker.
4. `Root.qvue`: render the home panel at full size with no card, and hand the header to the open room.
5. `ChatRoom` in `compact` mode: bubbles, fixed composer (use `visualViewport` to follow the keyboard), swipe to reply.
6. Regenerate the bundled screens (`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`).

### 2.3 Done when

- At 390×844 the list fills the screen, and Start chat opens the full-screen picker.
- A new direct chat with one person, and a group with two, can be started from the phone.
- Bubbles show own messages on the right. The composer stays above the keyboard on Android.
- Back closes layers in order and does not leave the app from inside a room.
- The desktop layout at 1440×900 looks and works as it did before.

## 3. Desktop gaps

The desktop layout exists (§1). Add:

| Item | Work |
|---|---|
| Quick switcher | Ctrl+K (Cmd+K on macOS) opens a dialog over the rooms and people search ("Find a room or person"). Arrow keys and Enter |
| Activity view | A sidebar entry: mentions, reactions to my messages, and replies in my threads, newest first. New service `find#ChatActivity` (§5) |
| Threads view | A sidebar entry for all threads I am in, from the `threads` transition with "Mine" |
| Sidebar sections | Fold the groups. Add groups: Phone (rooms of type `CrtExternal`) and Bridged |

**Done when:** Ctrl+K opens a room in three keys. The Activity view lists a mention and a reaction from another user.

## 4. Reactions and the message menu

| Item | Phone | Desktop |
|---|---|---|
| Open the menu | Long-press a bubble (500 ms, with haptic feedback through the host) | Hover toolbar (exists) and right-click |
| Menu | Quick reaction bar with `QUICK_REACTIONS` and "+", then Reply, Copy, Forward, Edit, Delete, Pin, Info | Same |
| Full picker | A searchable emoji picker with categories and recent emoji (kept in `storage`) | Same |
| Who reacted | Long-press a reaction chip | Hover a chip |
| Info | Sent, delivered, and read by whom (from the read receipts) | Same |

**Done when:** on a phone a user can react, remove a reaction, see who reacted, and open Info, all by touch.

## 5. Details tabs and new services

### 5.1 Tabs

The room's details (full screen on a phone, the right pane on a desktop) get these tabs. Each tab loads more as the
user scrolls.

| Tab | Source |
|---|---|
| People | Members (exists) |
| Media | `files` with `contentTypePrefix` `image/`, then `video/`. Grid of thumbnails. Full-screen viewer with swipe |
| Files | `files`, all types except image, video and audio |
| Voice | `files` with `audio/`. Inline player |
| Links | New `links` transition (§5.2) |
| Reactions | New `reactions` transition (§5.2) |
| Pinned | Pins (exists) |

### 5.2 New services (`org.moqui.chat.ChatServices`, next to `find#RoomFiles`)

All are read-only. All check that the user can read the room, the same check as `find#RoomFiles`. All page by
`beforeEventSeq` and return `nextBeforeEventSeq` (empty at the end). `limit` is 1 to 100, default 40.

| Service | In | Out | Source |
|---|---|---|---|
| `find#RoomLinks` | `chatRoomId`, `searchText`, `limit`, `beforeEventSeq` | `links`: `eventSeq`, `url`, `title`, `siteName`, `imageUrl`, `senderUserId`, `sentDate` | `ChatLinkPreview` joined to its event, newest first. Messages with no preview are not listed |
| `find#RoomReactions` | `chatRoomId`, `reactionKey`, `onlyToMe`, `limit`, `beforeEventSeq` | `messages`: `eventSeq`, `snippet`, `senderUserId`, `reactions` (the aggregated form) | Reaction relations grouped by target event, newest reaction first |
| `find#ChatActivity` | `limit`, `beforeEventSeq` | `items`: `kind` (`mention`, `reaction`, `reply`), `chatRoomId`, `eventSeq`, `actorUserId`, `snippet`, `date` | Mentions of the user, reactions to the user's messages, replies in threads the user is in. Only rooms the user can read |

Encrypted rooms: the server cannot read links or snippets. `find#RoomLinks` returns nothing for them. The client
finds links in the messages it has decrypted. `find#RoomReactions` and `find#ChatActivity` return the event and
reaction only, and the client decrypts the snippet.

Tests: a service test for each one (paging, a room the user cannot read gives no rows, an encrypted room). Check the
indexes on the relation and preview tables for the room and sequence filter.

**Done when:** each tab shows the right items for a test room with photos, a PDF, a voice note, two links and three
reactions, on a phone and on a desktop.

## 6. E2EE policy

### 6.1 Rules

1. Every room and call shows its E2EE status in the header and in the room info.
2. A room that is not E2EE says why, in one short sentence (§6.2).
3. Verified people get a check mark. A new device of a verified person shows a warning before the next send.
4. Calls in an encrypted room require SFrame E2EE. A call falls back only when the user agrees, and the call says so.
5. New direct rooms and private groups are encrypted by default (needs the design in §6.5 first).

### 6.2 Labels

| State | Icon | Banner at the top of the room | Composer hint |
|---|---|---|---|
| Encrypted, all devices verified | Lock with check | "Messages and calls are end-to-end encrypted. Only people in this chat can read them." | — |
| Encrypted, not all verified | Lock | Same, plus a "Verify" link | — |
| Plain MR Moqui room | Open lock | "Not end-to-end encrypted. The server can read these messages." | — |
| Public channel | Open lock, no banner | — | — |
| SMS or MMS | Open lock | "Not end-to-end encrypted. Texts go through the phone network and the SMS provider." | "Text message" |
| Bridged network | Open lock | "Not end-to-end encrypted on this server. The bridge reads messages to pass them on." | The network's name |
| Phone call | Open lock | "Phone calls are not end-to-end encrypted." | — |

### 6.3 By network

| Network | E2EE | Why not, when not |
|---|---|---|
| MR Moqui chat | Yes, Megolm, for encrypted rooms | Plain rooms and public channels: the server reads them, so search and agents work |
| MR Moqui calls | Yes, SFrame | Guests have no crypto device yet (Future-Plans) |
| XMPP | OMEMO between XMPP apps only | The server keeps OMEMO as it came. Megolm rooms are not served over XMPP |
| Matrix (C6) | Possible, Megolm end to end | Only if the room stays encrypted across the bridge |
| SMS, MMS, phone calls | Never | The phone network and the provider read them |
| RCS | See §16 | Depends on the path |
| WhatsApp (C4) | On WhatsApp's side only | The bridge on the server decrypts and encrypts again |
| Telegram (C4) | No | Bot and normal chats are not E2EE |

### 6.4 What encrypted rooms give up

The UI explains this when the user picks an encrypted room:
- Server search, link lists and link previews.
- Polls, forwards, bridges, agents and terminal send.
- The server still sees who is in the room, when each person sends, and which message a reply or reaction points to.

### 6.5 Design needed before E2EE by default

- **Rooms.** Today an encrypted direct room is a separate room from the plain direct room with the same person, and
  encryption cannot be turned off. Decide what `createDirect` makes, and what happens to the plain direct rooms that exist.
- **Android.** The crypto device lives in the desktop shell. Check that `matrix-sdk-crypto-wasm` runs in the Android
  WebView, and where the app keeps the device's store passphrase (Android Keystore).
- **First sign-in on a new phone.** Restore from key backup with the recovery key, or verify from another device.
- **Search on the device.** The offline store is plain SQLite and relies on the OS disk encryption
  ([Desktop-Client](../docs/ui/Desktop-Client.md)). Keeping decrypted text there for search is a security decision.
  Options: no search in encrypted rooms, search in memory only, or an encrypted index.

**Labels done when:** each state in §6.2 shows on a phone and on a desktop, and the SMS room shows the SMS banner.

## 7. Notifications

Notifications must work every time: on a phone with the screen off, after a reboot, on a closed app, and on a desktop.
A message the user should see must never be missed. A message already read on another device must not ring.

### 7.1 Today

| Path | What it does |
|---|---|
| Chat socket | Each event goes to members' open pages (`chatRoom:<id>`). Nothing when the app is closed |
| Mentions | A persisted notification on topic `ChatMention` (`notify#Mention`), if the app seeds the topic. Goes to Web Push and FCM through `send#WebPush` |
| Direct and room messages | Not pushed. ChatServices: "Socket messages (a chat event, a signal) are never pushed" |
| Web Push | Title, link and topic only, never the body. Disabled after 404, 410 or 5 failures in a row |
| FCM | Call ring (high-priority data message) to `PushService.kt`; registration tied to the device key |
| Settings | Room level `CnlAll`, `CnlMentions`, `CnlNone`; `CHAT_NOTIFY_DEFAULT`; `CHAT_NOTIFY_PREVIEW`; mute. Do not disturb is applied by the client only |

### 7.2 Rules

1. **Who gets a push.** Each member except the sender, by the room level: All gets every message, Mentions gets
   mentions and replies to their own messages, None gets nothing. Direct rooms default to All. Muted rooms get nothing.
2. **When.** At once, after the commit. No batching delay on the server. The phone groups notifications by room.
3. **Not when read.** A user who has read up to the event on any device gets no push. When the read marker moves
   (`chatUser` `read` change), the server sends a `clear` push, so other devices remove that room's notification.
4. **Do not disturb.** The server holds pushes while the user's status is `dnd` and sends one summary when it ends.
   Calls from people the user marks as allowed still ring.
5. **Content privacy.** The push carries no readable text. The server encrypts a small payload (room, sender name,
   snippet when the room is plain and `CHAT_NOTIFY_PREVIEW` is not N) with a key that belongs to the device. This is
   the same scheme as Web Push (RFC 8291). Google and Apple see only that a push came. Encrypted rooms: no snippet; the
   app decrypts the message on the device (§7.4).
6. **Texts and calls.** A text from an outside number pushes like a direct message. A call rings through the existing
   ring path. A missed call and a voicemail push as messages in the number's room.
7. **Every device.** Each registered device of the user gets the push: phones by FCM, browsers by Web Push, the desktop
   app by its socket while running.

### 7.3 Server work

| Item | Work |
|---|---|
| Fan-out | New `push#ChatEvent` after the commit of `append#Event`: apply §7.2 rules 1 to 4, then queue one push per device |
| Durable queue | Pushes go to a table (`PushOutbox`: device, payload, attempts, next try, status), so a restart loses none. A worker sends them |
| Retries | FCM 429 and 5xx: retry with exponential backoff and jitter, up to 1 hour, with FCM's `Retry-After`. 404 `UNREGISTERED`: delete the registration. Each device fails on its own |
| TTL and collapse | Messages: TTL 4 weeks, collapse key per room so a phone offline for a week gets one per room, not hundreds. Calls: TTL = ring time |
| Priority | High priority for messages that will show a notification, normal for `clear` and silent updates (Android removes high priority from apps whose high-priority pushes show nothing) |
| Device key | `register#PushDevice` gets the device's push public key and auth secret. Payloads are encrypted per device |
| Acknowledgement | The app reports each push it showed (`ack#Push`, push id). The server keeps sent, acked and failed counts per device |
| Health | System screen: per device: last registration, last push sent, last ack, failures. Alert when a device has had no ack for pushes sent in the last day |

### 7.4 Android app work

| Item | Work |
|---|---|
| Show without the WebView | `PushService` decrypts the payload and shows the notification in Kotlin, as the ring does today. It must not wait for the WebView or a network fetch |
| Encrypted rooms | Show "New message" from the payload at once. Then decrypt in the background (Rust, `matrix-sdk-crypto` with the device's store) and update the notification. If decryption fails, keep "New message" |
| Channels | Messages, Mentions, Texts, Calls (full screen), Missed calls, Other. The user sets sound and vibration per channel in Android settings |
| Style | `MessagingStyle` with the sender's avatar and name. One notification per room, grouped under a summary. Conversation shortcuts so Android shows them as conversations (bubbles, priority) |
| Actions | Reply (inline text, sent by Rust through the outbox, works offline), Mark read, Mute 1 hour. Tap opens the room at the message |
| Clear | The `clear` push and a read in the app remove the room's notification |
| Permissions | Ask for `POST_NOTIFICATIONS` (Android 13+) on first sign-in, with a reason. Ask for full-screen intent (Android 14+) for calls |
| Battery | Detect battery optimization and maker task killers. Show a one-tap fix to allow the app to run in the background |
| Token | Register the FCM token at every start and on `onNewToken`. Retry until the server answers |
| After reboot and update | Nothing to start: FCM delivers to a stopped app. Test it (§7.6). A force-stopped app gets nothing until it is opened; the app says so |
| Without Google Play | Optional UnifiedPush distributor (for example ntfy). Same encrypted payload. Later, after FCM works |

### 7.5 Desktop and browser

- The desktop app shows native notifications from its socket, with the same rules (§7.2) and the same `clear`.
- On start and on resume it reads the changes feeds and shows notifications for unread messages it missed.
- Browsers keep Web Push. The payload gets the room and sender, still no body.
- One device shows the notification at a time where possible: a page in focus marks the room read, which clears the
  others (rule 3).

### 7.6 Notification check screen and tests

**In the app** (Settings, Notifications):
- A checklist: permission given, channels on, battery optimization off, token registered, last push received.
- "Send a test notification": the server sends one through the full path. The screen shows the time it took.

**Automated tests:**
- Server: rules 1 to 4 (levels, mute, read on another device, DND), outbox retries, 404 deletes the device, TTL and
  collapse key, encrypted payload.
- Android instrumented tests with a fake FCM message: notification shows with the screen off, reply works, `clear`
  removes it.

**Soak test on real phones** (a Pixel and a Samsung, before each release):

| Case | Pass |
|---|---|
| App open, app in background, app swiped away, phone in Doze (`adb shell dumpsys deviceidle force-idle`), after reboot | 100% of 500 messages shown |
| Time from send to notification | 95% under 3 s, all under 30 s, in each case above |
| Read on the desktop | Phone notification gone within 5 s |
| Phone offline for 1 hour, then online | One notification per room, with the right count |
| Encrypted room | Notification shows the decrypted text within 10 s |

**Done when:** every soak test case passes on both phones, and the System screen shows no device with missed acks.

## 8. Voice and video calls

Calls must feel like the calls in a phone's own messaging app: one tap from a chat, they ring at once, they connect fast,
and they keep going when the network gets bad or changes.

### 8.1 Today

| Part | What exists |
|---|---|
| Media server | `crates/moqui-rtc` on str0m: ICE-lite, DTLS-SRTP, three simulcast layers, TWCC bandwidth estimate, audio only below a floor ([WebRTC](../docs/interfaces/WebRTC.md)) |
| E2EE | SFrame, per room or per call link |
| Ring on Android | FCM high-priority `ring` to `CallManager`; self-managed Android Telecom call (`CallConnectionService`); CallStyle notification; full-screen ring over the lock screen; Decline with no WebView ([Mobile-Client](../docs/ui/Mobile-Client.md)) |
| Media on Android | The WebView's WebRTC (`WebrootCall.js`). A native audio client is planned later (MT2) |
| Reconnect | A closed socket keeps its leg for 30 s. A reconnect joins again with the next connection generation |
| TURN | Embedded TURN server ([TURN](../docs/interfaces/TURN.md)) |
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

### 8.3 Work

**Call flow and UI**
1. Call and video buttons in the room header on both layouts (§2, §3). A direct room calls the person; a group room
   starts a group call that members can join from the chat.
2. A full-screen call screen on the phone: big avatar for voice, self view and remote video for video, mute, speaker,
   camera, flip, add people, hang up. The same screen in a side panel on the desktop.
3. Voice-to-video upgrade and downgrade in the same call (renegotiation exists).
4. Picture-in-picture on Android; a small floating call window on the desktop.
5. Audio routing on Android through Telecom `CallEndpoint` (earpiece, speaker, Bluetooth, wired). Proximity wake lock.
6. "Move to this device": a second device of the same user joins the call as a new leg, then the first leg leaves.
   The other side hears no gap.
7. Call waiting and decline with a quick text.

**Network resilience** (the test results from §8.4 decide the order)
8. Opus with in-band FEC, DTX, and RED (redundant audio) at low bandwidth. Audio gets priority over video.
9. NACK and RTX for video. Simulcast layer switch from the TWCC estimate (exists); check it reacts in under 2 s.
10. ICE restart on network change: Android `ConnectivityManager` callback tells the page, the page restarts ICE.
    The server keeps the leg during the restart.
11. Reconnect: keep the leg for 60 s (today 30 s). Media resumes on the new path without a new ring.
12. TURN over TCP and TLS on port 443 for networks that block UDP. The client tries UDP, then TCP, then TLS.
13. Keepalives tuned for mobile NATs (STUN consent and the socket's ping), so a quiet call does not lose its path.
14. A quality icon from `getStats` (loss, RTT, jitter), and "Your network is weak" when the uplink is the cause.

**Direct path for one-to-one calls (option)**
- One-to-one calls could go peer to peer when both sides can reach each other, and fall back to the media server.
  This cuts delay. It adds a second media path to keep working. Decide after the §8.4 tests show the server's delay.

### 8.4 Network tests

Calls and the chat socket must work on bad mobile networks. A test suite runs real calls between two headless browsers
through the media server, with network conditions made by standard open-source tools (Linux network namespaces with
`tc netem` and `tbf`, and recorded cellular traces). The suite and its first results are in a separate change.

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

The suite runs before each release and when `moqui-rtc`, `WebrootCall.js` or the TURN server changes.

### 8.5 Done when

- Every row of §8.2 works between two Android phones, and between a phone and the desktop app.
- Every row of §8.4 passes.
- An encrypted call (SFrame) passes the same tests as a plain one.

## 9. The user's own phone number

The app replaces an XMPP phone-number app (such as Cheogram Android with a gateway service). The user ports the number
to the server's Telnyx account. The server already does texts, MMS, calls and voicemail for a number (§1).

### 9.1 Gaps

| Item | Today | Work |
|---|---|---|
| Port the number | — | A Telnyx port order (a portal step, not code). Then add the number in System, Voice: Numbers with the messaging bridge, the trunk and the user's routes |
| 10DLC | Required for texts from a US local number | Register the brand and campaign in the Telnyx portal ([SMS](../docs/interfaces/SMS.md)) |
| Checked live | Telnyx calls, SMS and MMS are tested with fakes only | Test on the real number: text, MMS, call in, call out, voicemail |
| Push for messages | FCM rings calls only | §7 |
| Phone rooms in the list | Rooms of type `CrtExternal` with an SMS icon | Phone group in the sidebar and a phone filter on the phone layout (§2, §3) |
| Group texts | Not built (Future-Plans SMS) | Group MMS: one room for a set of outside numbers |
| Android dialer and contacts | `CallConnectionService.kt` for app calls | Register a `PhoneAccount`, so the dialer and contacts can call through the app with `place#PhoneCall` |
| Phone book | — | Read Android contacts (asked on first use). Show names on phone rooms. Search contacts in the new-chat picker |

### 9.2 Done when

- The ported number sends and gets texts and MMS in the app, with the SMS banner.
- A call to the number rings the app with the screen off. A call out from the Android dialer goes through the app.
- Voicemail shows in the number's room.
- A text while the app is closed shows an Android notification. Reply from the notification works.

## 10. The phone's SIM texts (`sim-sms`)

Optional: the app becomes the Android default SMS app for the phone's own SIM.

**Decided:**
- A cargo feature `sim-sms` in `moqui-mobile` turns on an optional crate `tauri-plugin-sim-sms`. Its Android library is
  merged only with the feature, so the default build asks for no SMS permissions.
- The plugin has the parts Android needs for the default SMS role: an `SMS_DELIVER` receiver, a `WAP_PUSH_DELIVER`
  receiver for MMS, the `RESPOND_VIA_MESSAGE` service, and handlers for `sms:`, `smsto:`, `mms:` and `mmsto:` links.
- Release builds make two APKs: the default one and one with `sim-sms`.

**Design needed:**
- Do SIM texts stay on the phone only, or sync to the server as rooms? Syncing puts the phone's texts on the server.
- MMS send and receive: the PDU encoding and the download of MMS content.
- How SIM threads and server rooms show together in the list, and what happens when a person has both.

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
| Matrix (C6, planned) | Matrix users | Megolm possible | Also the way to host Matrix bridges (§13.2) |

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
`ChatBridgeAdapter` in `crates/moqui-channels`, one room per outside conversation, the bridge chip and the E2EE label.

| Way | Use for | Why |
|---|---|---|
| A Rust adapter on a documented API | GroupMe (REST), BlueBubbles (REST and its event socket), Telnyx RCS | Small, no extra process, same as the SMS adapter |
| A Rust adapter on a sidecar's JSON-RPC | Signal (`signal-cli` daemon, GPL-3) | The Signal protocol stays in a maintained client. The sidecar is a separate process |
| The Matrix appservice API (C6c) with a mautrix bridge as sidecar | Personal RCS (`mautrix-gmessages`), later WhatsApp, Telegram, Discord | One API in MR Moqui opens many maintained bridges. Each bridge is AGPL and runs as a separate process |

Each sidecar is optional (a cargo feature and a System screen to start it). The sidecar's license applies to the
sidecar, not to MR Moqui; a legal check is still needed before shipping one.

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

The plan builds A. B and C stay open (§19.2).

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
| Photos, video, files, voice notes | Uploads; voice notes in §2.1 | Partly built |
| Search, media gallery | Server search; Details tabs | Built; tabs in §5 |
| Pin, mute, archive | Pin, mute; archive | Pin and mute built; archive §2 |
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
| Scheduled send | Scheduled send: a send time on the outbox item, sent by the server | New |
| Spam protection, block, report | Block a person or number; spam filter for outside numbers (Telnyx number reputation and a local list) | New |
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
| Share location, live location | A location message with a map (the Maps app), and live location for a set time | New |
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
| Usernames, phone number hidden | Usernames; a phone number is never shown to other MR Moqui users | Built (check) |
| Linked devices list | Apps and devices | Built |
| Registration lock, PIN | Device keys, password, recovery key for key backup | Built |
| Screen security (no screenshots) | Android `FLAG_SECURE` option, per app or per encrypted room | New |
| Group links with admin approval | Room invite links and QR codes, with optional approval | New |
| Stories | Later, if wanted | Later |
| Sealed sender | Not possible: the server sees who sends. The UI says so (§6) | Not planned |
| Proxy for censored networks | TURN on 443 and the web socket on 443 | Partly (§8) |

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
| Dialer keypad, `tel:` links | Dialer screen (exists on the web), `tel:` and `DIAL` intents | Dialer built; intents new |
| In-call screen for SIM calls | An `InCallService`: the same call screen as Moqui calls (§8) for carrier calls | New |
| Pick the line per call | SIM line, the Telnyx number (through `place#PhoneCall`), or a Moqui call to a user. Default by contact, last used | New |
| Incoming carrier calls | Ring screen over the lock screen (exists for Moqui calls), with the caller's contact name | Ring built; carrier calls new |
| Call log | One history: SIM calls (Android `CallLog`), Telnyx calls and Moqui calls (server call history), merged and by person | New |
| Voicemail | Server voicemail in the number's room (built). The carrier's voicemail: dial-in only (visual voicemail of carriers is closed) | Server voicemail built |
| Spam and unknown callers | Android `CallScreeningService`: block numbers on the block list (§14.2), silence unknown callers as an option | New |
| Hold, mute, speaker, Bluetooth, merge, swap | Android Telecom for SIM calls; the server for Telnyx and Moqui calls | Partly built |
| Emergency calls | Always on the SIM and the carrier, through Android, never through the server. The app must not block or delay them | Required |
| Calls from the Contacts app and the car | Android `PhoneAccount` for the Telnyx number and Moqui calls, so other apps and cars can call through them | §9 |

**Build:** a cargo feature `sim-dialer`, like `sim-sms`, with its Android parts in an optional plugin crate. The default
APK has no `CALL_PHONE`, `READ_CALL_LOG` or `ANSWER_PHONE_CALLS` permissions. Google Play allows call log permissions only
for the default phone app, so the call log is in this build only. `sim-sms` and `sim-dialer` can ship in one "phone" APK.

**Done when:** with the role given, a carrier call in and out, a Telnyx call in and out, and a Moqui call all use the
app's screens and appear in one call log; an emergency number goes to the carrier at once.

### 14.8 Android Contacts app

| Feature | Equivalent | Status |
|---|---|---|
| Contact list and details | A Contacts screen in the app: name, photo, numbers, emails, and each network the person is on (§11 unified contacts) | New |
| Contacts synced to the server | The server's CardDAV contact books (Mantle's `ContactDavServices`, [Sync](../docs/interfaces/Sync.md)): personal books read and write, shared books read only | Built on the server |
| Contacts in other Android apps | A built-in Android sync adapter: an Android account "MR Moqui" that syncs the CardDAV books into Android's contact store, both ways. The phone app, the keyboard and the car see them | New |
| Existing phone contacts | Import once from the phone's other accounts into the personal book, with duplicates merged | New |
| Caller name and photo | From the synced contacts, for SIM, Telnyx and Moqui calls and for texts | New |
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

#### Finding 1: Personal RCS in a third-party default SMS app

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

#### Finding 2: Business RCS (RBM) through a provider

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

#### Finding 3: RCS end-to-end encryption

| Item | Finding |
| --- | --- |
| Standard | GSMA Universal Profile 3.0 (2025-03-14) defines MLS (RFC 9420) for RCS. |
| Rollout | Apple and Google started cross-platform E2EE RCS in beta on 2026-05-11. Needs iOS 26.5 and current Google Messages, plus supported carriers. On by default. A lock icon shows. |
| Android to Android | Google Messages had its own E2EE since 2020. Google says "only Google Messages supports end-to-end encrypted RCS" on Android. |
| Third-party reach | None. MLS keys live in Google Messages or Apple Messages. No API exposes them. |
| Business RCS | Google: "Verified RCS chats between businesses and users aren't end-to-end encrypted." Provider, carrier and Google can see content. |
| Bridge (section 5) | The phone decrypts, then sends plaintext to the bridge over the web-pairing channel. The bridge sees plaintext. |

#### Finding 4: Open-source RCS clients

| Project | Status | Practical? |
| --- | --- | --- |
| `Hirohumi/rust-rcs-client` (Apache-2.0) | Rust RCS client library. Works mainly on Chinese carriers. CPM standalone, HTTP file transfer, chatbots. | No for US/EU. It needs carrier auto-configuration and IMS access. Jibe needs DroidGuard. |
| `android-rcs/rcsjta` | Old GSMA Java RCS stack. Inactive. | No. Old profile. Same access problems. |
| AOSP Messaging | No RCS. | No. |
| GrapheneOS plan | Future RCS + MLS client. Still needs sandboxed Google Play. | Watch only. |
| Rooted phone + privileged app | Could get `PERFORM_IMS_SINGLE_REGISTRATION`. | No. Carrier and Jibe servers reject non-certified clients. Play Integrity blocks root. |

#### Finding 5: Bridging Google Messages for Web

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

### 16.3 Options

| Option | How it works | Who can use it | E2EE? | Cost / approval | Effort | Risks | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Native RCS in the app | App talks RCS to carrier/Jibe | Nobody (no API) | n/a | Not possible | n/a | n/a | Do not build |
| 2. Hand-off to Google Messages | App opens the thread in Google Messages by intent | Any user with Google Messages | Yes, in Google Messages | Free | Low | Two apps; not parity | Build as fallback |
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
5. Label every SIM SMS/MMS thread "SMS - not encrypted".

#### RCS phase 1: Telnyx RCS business adapter (server)

1. Add `crates/moqui-channels/src/telnyx_rcs.rs` next to `telnyx.rs`. Name the adapter `rcs`.
2. Add a chat bridge type for RCS (for example `CbtRcs`). Keep `configJson` encrypted, like the SMS adapter.
3. Config keys: `apiKey`, `publicKey`, `agentId`, `messagingProfileId`, `smsFallbackFrom`, `mmsFallback`, `apiBaseUrl`.
4. Reuse the Ed25519 webhook check from `telnyx.rs` and `webhook.rs`. Move shared code into a small `telnyx_common` module.
5. Map `deliver()` to `POST /v2/messages/rcs`. Send attachments as `rich_card` media by `file_url`.
6. Map inbound `text`, `user_file`, `location` and `suggestion_response` to `ChatBridgeServices.ingest#Event`.
7. Map `message.sent`, `message.finalized` and `message.read` to delivery and read status.
8. Record `data.type` (RCS or SMS) on each sent message. Show the real path in the UI.
9. Set `Capabilities`: `edit=false`, `delete=false`, `reaction=false`, `idempotent=false`.
10. Add a capability check before send. Fall back to the SMS bridge when the number is not RCS-capable.
11. Add tests with a fake Telnyx server, like the current SMS tests.
12. Write an operator guide in `docs/interfaces/`: brand, agent, test devices, launch, fees.

#### RCS phase 2: Rich features and labels

1. Add suggested replies and actions to the chat model. Show them as chips in the mobile UI.
2. Add rich cards and carousels as a message kind. Render them in the mobile UI.
3. Add typing and read events when Telnyx exposes RBM `IS_TYPING` and `READ`.
4. Add an `encryption` field to `Capabilities` in `chat_bridge.rs`. Values: `None`, `Transport`, `BridgeDecrypted`, `EndToEnd`.
5. Send the field to clients. Show the labels below in thread headers and message info.

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

| Path | Label (thread header) | Detail text (message info) | Icon |
| --- | --- | --- | --- |
| MR Moqui chat, E2EE on | "End-to-end encrypted" | "Only people in this chat can read it." | Closed lock |
| SIM SMS / MMS (`sim-sms`) | "SMS - not encrypted" | "Your carrier can read these messages." | Open lock |
| Telnyx SMS / MMS bridge | "SMS - not encrypted" | "Telnyx, carriers and this server can read these messages." | Open lock |
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

## 18. Phases

| # | Work | Section |
|---|---|---|
| 1 | Shared components file, phone layout | §2 |
| 2 | Notifications | §7 |
| 3 | Calls: buttons, call screen, audio routing, network resilience | §8 |
| 4 | Reactions and the message menu | §4 |
| 5 | Details tabs, `find#RoomLinks`, `find#RoomReactions` | §5 |
| 6 | E2EE labels | §6.2 |
| 7 | Desktop gaps, `find#ChatActivity` | §3 |
| 8 | Phone number: port, live checks, dialer, contacts, group texts | §9 |
| 9 | E2EE by default, after its design | §6.5 |
| 10 | `sim-sms`, after its design | §10 |
| 11 | Other networks, app equivalents, RCS | §13–§17; order below |

After phases 1 to 3: a pull request and a new APK for a test on a real phone. Again after phase 6.

**Test harness:** a static page that loads the bundled screens with a fake `moquiDesktop` host, pointed at a dev server
with two test users. A headless browser opens it at 390×844 and 1440×900 and runs each phase's "done when" steps.

**Order for phase 11** (after phases 1 to 3: phone layout, notifications, calls):
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

## 19. Open questions

### 19.1 The app

- The breakpoint: `md` (1024 px) or `sm` (600 px)?
- Search in encrypted rooms (§6.5): none, in memory, or an encrypted index?
- SIM texts (§10): phone only, or synced to the server?
- One-to-one calls peer to peer (§8.3), or always through the media server?
- Notifications without Google Play (UnifiedPush): needed in the first release, or later?

### 19.2 Other networks

- §13.4: number on Telnyx only (A), or keep a SIM line for personal RCS (B or C)?
- iMessage: is there a Mac that can stay on? Without one, iPhone users get SMS.
- Signal: accept that the server can read bridged Signal messages? The alternative is to keep the Signal app for those
  chats.
- Sidecars (signal-cli, BlueBubbles, mautrix): on the same server, or on a separate machine?
- Stories: wanted?
- After the port, does the phone keep a SIM line (for carrier calls and emergency calls), or only data?

### 19.3 RCS

1. Do you accept that RCS from the server comes from a business agent name, not your own number?
2. Will you pay the Telnyx brand vetting ($100) and T-Mobile launch ($500) fees?
3. Which agent use case fits: Transactional or Multi-use?
4. Which countries do you need? Telnyx API onboarding is US only today.
5. Do you want RCS phase 3 (personal RCS bridge)? It has ToS risk and AGPL code.
6. If yes, do you accept a separate AGPL sidecar repo?
7. If yes, is it acceptable that the server stores bridged RCS as plaintext?
8. For `sim-sms` users, should the app warn and turn off RCS, or should it keep Google Messages as default and use only the hand-off?
9. Is a Twilio or Sinch adapter needed, or is Telnyx enough?

## 20. Sources

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
