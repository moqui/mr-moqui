# basic-comms

BasicComms, the framework's Comms app at `/qapps/BasicComms`: the Inbox, Compose, Home, chat, calls and their recordings, and the mail and call settings, then voice,
usable with only the framework ([plans/Comms-Framework.md](../../../plans/Comms-Framework.md)). The data model is in
`framework/entity`; this component has the screens and their services.

| Path | What |
| --- | --- |
| `screen/BasicComms.xml`, `screen/BasicComms/` | The app root and its screens (`.qvue` with `.tui.json` twins beside their XML) |
| `screen/System/` | Admin: Voice (trunks, numbers, caller IDs, fraud limits, emergency setup, desk and soft phones, ring groups, schedules), mounted into the tools System app by `MoquiConf.xml` |
| `service/org/moqui/comms/` | Screen services (`InboxServices` with `run#Operation`, `ComposeServices`, `CommsHomeServices`, `ChatScreenServices`, `MailSettingsServices`, `CommsServices`, `FilePickerServices` for Attach from Files and Save to Files) and the provider interfaces (`ProviderInterfaces`) |
| `template/` | Transitions shared by screens (`FilePickerTransitions.xml`: the drive picker; `VoiceTransitions.xml`: the Voice screens' user search; `DialerTransitions.xml`: the Dialer's, for Calls/Dialer and the tray's Panel/Dialer) |
| `data/` | The `BASIC_COMMS` artifact group and the do-not-disturb job |
| `test/` | Server specs; `test/client/` the browser specs (a call in two browsers), `test/MoquiConf.xml` turns call media on for their server |

An app adds to these screens without copying them: it mounts them under its own root, adds widgets and subscreens with
`screen-extend`, answers the provider hooks (`inbox-source`, `record-link`, ...) from its root's `shell-providers`, and adds
`ext-action` and `ext-panel` items ([docs/ui/Qvt-Client.md](../../../docs/ui/Qvt-Client.md#declared-actions-and-panels)).
IronStone BOSS does this for its Comms; it hides BasicComms from the launcher and sets `comms_app_root` to `/qapps/Comms`.

Test: `./moq test basic-comms`; the browser specs `./moq test-client basic-comms --serve` with a binary that has the
`rtc` feature (calls: [docs/interfaces/WebRTC.md](../../../docs/interfaces/WebRTC.md); voicemail, the Voicemail tab, Calls/Voicemail and Settings/Phone:
[docs/interfaces/Voicemail.md](../../../docs/interfaces/Voicemail.md); the Dialer and Settings/Call handling:
[docs/interfaces/SIP.md](../../../docs/interfaces/SIP.md#call-handling); recordings, Calls/Recordings:
[docs/interfaces/Call-Recording.md](../../../docs/interfaces/Call-Recording.md)).
