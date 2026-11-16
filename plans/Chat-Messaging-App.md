# Chat as a Messaging App

Plan, 2026-11-16. Nothing built.

The goal: the Comms chat works as a daily messaging app.
- On a phone it works like a phone messaging app: room list, bubbles, reactions, media.
- On a wide screen it works like a team chat desktop app: sidebar, threads pane, quick switcher.
- It is the user's phone too: their own number for texts, calls and voicemail, through the server's SMS and SIP provider.
- It keeps end-to-end encryption (E2EE) wherever the network allows. It says plainly when a chat or call is not E2EE.

| Part | Status |
|---|---|
| §2–§5: phone layout, desktop gaps, reactions, details tabs | Ready to build |
| §6: E2EE policy and labels | Labels ready to build; E2EE by default needs a design (§6.5) |
| §7: notifications | Ready to build; payload encryption reuses the Web Push scheme |
| §8: voice and video calls | Ready to build; the network fixes are ordered by the §8.4 test results |
| §9: the user's own phone number | Most of it is built; the gaps are listed |
| §10: the phone's SIM texts (`sim-sms`) | Needs a design |
| RCS | Research and plan in [RCS.md](RCS.md) |
| Other apps: GroupMe, iMessage, Google Messages, Signal, Cheogram | Reach and feature equivalents in [Messaging-Replacement.md](Messaging-Replacement.md) |

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
| RCS | See [RCS.md](RCS.md) | Depends on the path |
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

## 11. Phases

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
| 11 | Other networks and app equivalents | [Messaging-Replacement.md](Messaging-Replacement.md), [RCS.md](RCS.md) |

After phases 1 to 3: a pull request and a new APK for a test on a real phone. Again after phase 6.

**Test harness:** a static page that loads the bundled screens with a fake `moquiDesktop` host, pointed at a dev server
with two test users. A headless browser opens it at 390×844 and 1440×900 and runs each phase's "done when" steps.

## 12. Open questions

- The breakpoint: `md` (1024 px) or `sm` (600 px)?
- Search in encrypted rooms (§6.5): none, in memory, or an encrypted index?
- SIM texts (§10): phone only, or synced to the server?
- One-to-one calls peer to peer (§8.3), or always through the media server?
- Notifications without Google Play (UnifiedPush): needed in the first release, or later?
