# basic-comms

BasicComms, the framework's Comms app at `/qapps/BasicComms`: the Inbox, Compose, Home, chat, calls and their recordings, and the mail and call settings, then voice,
usable with only the framework ([plans/Comms-Framework.md](../../../plans/Comms-Framework.md)). The data model is in
`framework/entity`; this component has the screens and their services.

| Path | What |
| --- | --- |
| `screen/BasicComms.xml`, `screen/BasicComms/` | The app root and its screens (`.qvue` with `.tui.json` twins beside their XML) |
| `screen/System/` | Admin: Voice (trunks, numbers, caller IDs, fraud limits, emergency setup, desk and soft phones, ring groups, schedules, IVR menus, queues) and Chat Bridges ([docs/interfaces/Chat-Bridges.md](../../../docs/interfaces/Chat-Bridges.md)), mounted into the tools System app by `MoquiConf.xml` |
| `service/org/moqui/comms/` | Screen services (`InboxServices` with `run#Operation`, `ComposeServices`, `CommsHomeServices`, `ChatScreenServices`, `MailSettingsServices`, `CommsServices`, `FilePickerServices` for Attach from Files and Save to Files) and the provider interfaces (`ProviderInterfaces`) |
| `template/` | Transitions shared by screens (`FilePickerTransitions.xml`: the drive picker; `VoiceTransitions.xml`: the Voice screens' user and target search; `DialerTransitions.xml`: the Dialer's, for Calls/Dialer and the tray's Panel/Dialer; `UploadTransitions.xml`: files sent in chunks, for Compose and Chat) |
| `data/` | The `BASIC_COMMS` artifact group and the do-not-disturb job |
| `test/` | Server specs; `test/client/` the browser specs (a call in two browsers), `test/MoquiConf.xml` turns call media on for their server |

An app adds to these screens without copying them: it mounts them under its own root, adds widgets and subscreens with
`screen-extend`, answers the provider hooks (`inbox-source`, `record-link`, ...) from its root's `shell-providers`, and adds
`ext-action` and `ext-panel` items ([docs/ui/Qvt-Client.md](../../../docs/ui/Qvt-Client.md#declared-actions-and-panels)).
IronStone BOSS does this for its Comms; it hides BasicComms from the launcher and sets `comms_app_root` to `/qapps/Comms`.

The desktop client bundles the chat panel, Chat, Inbox (Mail), Compose, Calls/Call and Calls/Dialer in its shell,
precompiled from these `.qvue` files, and shows them under the server's comms app root; it owns the calls (its app
windows open them there); the rest of the app opens in its app windows. Their offline presentation (queued and failed
sends, the Inbox's Outbox, "needs a connection", the gaps of what is kept) shows only there
([docs/ui/Desktop-Client.md](../../../docs/ui/Desktop-Client.md#bundled-screens-and-app-windows)). After a change to one of them:
`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`.

Test: `./moq test basic-comms`; the bundled screens in the page (no server) `./moq test-client basic-comms --offline`; the browser specs `./moq test-client basic-comms --serve` with a binary that has the
`rtc` feature (calls: [docs/interfaces/WebRTC.md](../../../docs/interfaces/WebRTC.md); voicemail, the Voicemail tab, Calls/Voicemail and Settings/Phone:
[docs/interfaces/Voicemail.md](../../../docs/interfaces/Voicemail.md); the Dialer and Settings/Call handling:
[docs/interfaces/SIP.md](../../../docs/interfaces/SIP.md#call-handling); Calls/Queues and transfer in the call:
[SIP, Queues](../../../docs/interfaces/SIP.md#queues); recordings, Calls/Recordings:
[docs/interfaces/Call-Recording.md](../../../docs/interfaces/Call-Recording.md)).
