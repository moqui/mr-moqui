# basic-comms

BasicComms, the framework's Comms app at `/qapps/BasicComms`: chat and the mail settings now, then the Inbox, calls and voice,
usable with only the framework ([plans/Comms-Framework.md](../../../plans/Comms-Framework.md)). The data model is in
`framework/entity`; this component has the screens and their services.

| Path | What |
| --- | --- |
| `screen/BasicComms.xml`, `screen/BasicComms/` | The app root and its screens (`.qvue` with `.tui.json` twins beside their XML) |
| `service/org/moqui/comms/` | Screen services (`ChatScreenServices`, `MailSettingsServices`, `CommsServices`) and the provider interfaces (`ProviderInterfaces`) |
| `data/` | The `BASIC_COMMS` artifact group and the do-not-disturb job |
| `test/` | Server specs |

An app adds to these screens without copying them: it mounts them under its own root, adds widgets and subscreens with
`screen-extend`, answers the provider hooks (`inbox-source`, `record-link`, ...) from its root's `shell-providers`, and adds
`ext-action` and `ext-panel` items ([docs/ui/Qvt-Client.md](../../../docs/ui/Qvt-Client.md#declared-actions-and-panels)).
IronStone BOSS does this for its Comms; it hides BasicComms from the launcher and sets `comms_app_root` to `/qapps/Comms`.

Test: `./moq test basic-comms`.
