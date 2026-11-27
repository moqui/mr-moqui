# basic-comms

BasicComms, the framework's Comms app at `/qapps/BasicComms`: the Inbox, Compose, Home, chat, calls and their recordings, and the mail and call settings, then voice,
usable with only the framework ([plans/archive/Comms-Framework.md](../../../../plans/archive/Comms-Framework.md)). The data model is in
`framework/entity`; this component has the screens and their services.

| Path | What |
| --- | --- |
| `screen/BasicComms.xml`, `screen/BasicComms/` | The app root and its screens (`.qvue` with `.tui.json` twins beside their XML) |
| `screen/BasicComms/Boards/` | Personal and room whiteboards, version history and exports. A meeting can share a board with admitted guests. [Whiteboards](../../../../docs/tools/Comms.md#whiteboards) |
| `screen/ChatJoin.xml`, `ChatJoin.qvue` | A room's invite link page, `/join/<token>` (the token-route `join` of `MoquiConf.xml`): sign in, Join or Ask to join ([Comms](../../../../docs/tools/Comms.md)) |
| `screen/System/` | Admin: Voice (trunks, numbers and which carry rooms' text members, caller IDs, fraud limits, emergency setup, desk and soft phones, ring groups, schedules, IVR menus, queues, the spam filter of texts and its rules), Chat Bridges ([docs/interfaces/Chat-Bridges.md](../../../../docs/interfaces/Chat-Bridges.md)), Chat Stickers (the server's sticker packs) and Push Health (every user's push devices and their counts; [docs/ui/Qvt-Client.md](../../../../docs/ui/Qvt-Client.md#install-and-web-push)), mounted into the tools System app by `MoquiConf.xml` |
| `screen-extend/tools/screen/System/Home.xml` | Voice, Chat Bridges and Push Health in the Integration group of the System home's navigation |
| `service/org/moqui/comms/` | Screen services (`InboxServices` with `run#Operation`, `ComposeServices`, `CommsHomeServices`, `ChatScreenServices`, `MailSettingsServices`, `CommsServices`, `FilePickerServices` for Attach from Files and Save to Files) and the provider interfaces (`ProviderInterfaces`) |
| `template/` | Transitions shared by screens (`FilePickerTransitions.xml`: the drive picker; `VoiceTransitions.xml`: the Voice screens' user and target search; `DialerTransitions.xml`: the Dialer's, for Calls/Dialer and the tray's Panel/Dialer; `UploadTransitions.xml`: files sent in chunks, for Compose and Chat; `ChatScheduleTransitions.xml`: Chat's Send later and reminders; `ChatCalTransitions.xml`: Chat's events with RSVP; `ChatStickerTransitions.xml` and `StickerPacks.xml`: stickers, sticker packs, locations and live location; `ChatPrivacyTransitions.xml`: one-time codes and screen security, [Comms](../../../../docs/tools/Comms.md)) |
| `data/` | The `BASIC_COMMS` artifact group, granted to the `BASIC_COMMS_USERS` group and `ADMIN` (not `ALL_USERS`: an account needs the group to open BasicComms), and the do-not-disturb job |
| `test/` | Server specs; `test/client/` the browser specs (a call in two browsers), `test/MoquiConf.xml` turns call media on for their server |

An app adds to these screens without copying them: it mounts them under its own root, adds widgets and subscreens with
`screen-extend`, answers the provider hooks (`inbox-source`, `record-link`, ...) from its root's `shell-providers`, and adds
`ext-action` and `ext-panel` items ([docs/ui/Qvt-Client.md](../../../../docs/ui/Qvt-Client.md#declared-actions-and-panels)).
IronStone BOSS does this for its Comms; it hides BasicComms from the launcher and sets `comms@app-root` to `/qapps/Comms`.

The desktop client bundles the chat panel, Chat, Inbox (Mail), Compose, Calls/Call, Calls/Dialer and Settings/Devices
(My devices) in its shell,
precompiled from these `.qvue` files, and shows them under the server's comms app root; it owns the calls (its app
windows open them there); the rest of the app opens in its app windows. Their offline presentation (queued and failed
sends and message changes, conflicts and the copy of a draft changed elsewhere, the Inbox's Outbox, "needs a
connection", the gaps of what is kept) shows only there
([docs/ui/Desktop-Client.md](../../../../docs/ui/Desktop-Client.md#bundled-screens-and-app-windows)). A room's details (people, media,
files, voice notes, links, reactions, pinned) and voice notes: [Comms](../../../../docs/tools/Comms.md#room-details); the palette's Chat rooms, Activity,
Threads, the rail's groups and the mail line on a person's row: [Comms](../../../../docs/tools/Comms.md#chat-desktop); disappearing messages and
view-once: [Comms](../../../../docs/tools/Comms.md#disappearing-messages); a person's rooms across networks, the network button and the person
card: [Comms](../../../../docs/tools/Comms.md#people-across-networks). After a change to one of them:
`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`.

Test ([the two suites](../../../../docs/Build-Test.md#the-two-suites)): the regression suite is `./moq test basic-comms`,
the shell's sync states (no server) `./moq test-client basic-comms --offline` and the crypto walk
`./moq test-client basic-comms --serve`; the acceptance suite, the call and encrypted chat walks, the direct path, the
phone's chat in the bundled shell at 390 × 844 (and reactions, who reacted and Info there by touch), the E2EE labels and a room's details tabs and a voice note there and at a desktop's width, the palette, Activity, Threads, the groups and the mail line in the shell (`ChatDesktop`) and the palette on the web (`ChatPalette`)
(`test/acceptance/client/`), is `./moq accept basic-comms` with a binary that has the `rtc` feature; its calls on shaped
networks (`CallNetwork`) run only under `sudo scripts/call-network.sh` and skip otherwise
([WebRTC, The network tests](../../../../docs/interfaces/WebRTC.md#the-network-tests)). See also
(calls: [docs/interfaces/WebRTC.md](../../../../docs/interfaces/WebRTC.md); voicemail, the Voicemail tab, Calls/Voicemail and Settings/Phone:
[docs/interfaces/Voicemail.md](../../../../docs/interfaces/Voicemail.md); the Dialer and Settings/Call handling:
[docs/interfaces/SIP.md](../../../../docs/interfaces/SIP.md#call-handling); Calls/Queues and transfer in the call:
[SIP, Queues](../../../../docs/interfaces/SIP.md#queues); recordings, Calls/Recordings:
[docs/interfaces/Call-Recording.md](../../../../docs/interfaces/Call-Recording.md)).
