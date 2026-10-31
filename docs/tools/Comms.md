# Comms

BasicComms is the framework's Comms app at `/qapps/BasicComms`: the Inbox and Compose over the user's mail accounts,
MessageDetail, Home, chat (rooms, direct messages, threads, the tray's Chat tab) and the mail and chat settings. It
works with only the framework. An app that has more (IronStone BOSS: internal messages, mailboxes, contact books,
records) mounts the same screens under its own root and adds its parts, without a copy of them. The design and its
order of work are in [plans/Comms-Framework.md](../../plans/Comms-Framework.md); calls, voice and chat bridges come
later there.

The code: `runtime/base-component/basic-comms` (screens, services, data, specs); the data model is in
`framework/entity` (`EmailEntities` in `BasicEntities.xml`, `ChatEntities.xml`); the mail client is
`org.moqui.impl.EmailClientServices` and `moqui-mail`; chat is `org.moqui.impl.ChatServices`.

## For users

- **Inbox.** Every mail account's Inbox together, or one account, folder or label; search with Gmail's operators;
  flag, snooze, archive, move, label and spam; rules, identities and signatures, out of office and text blocks in
  Settings. Add an account in Settings, Mail accounts (Gmail and Microsoft sign in with the provider where the server
  is set up for it: [Mail-OAuth](../interfaces/Mail-OAuth.md)).
- **Compose.** A new message, a reply or a forward from an account the user may send from, with undo send and
  scheduled sending.
- **Chat.** Rooms (private, public, direct), threads, mentions, reactions, polls, pinned messages and files; presence
  and status in the account menu. The tray's Chat tab is the same view, smaller.
- **Home.** Unread by account, flagged, snoozed, mentions, mail that was not sent, and rooms with unread.
- **Notifications on this device.** The account menu turns on desktop notifications and Web Push for the browser or
  the installed app ([Qvt-Client](../ui/Qvt-Client.md#install-and-web-push)).

## For app authors

An app adds to BasicComms through four seams. None needs a copy of a BasicComms screen.

**Mount the screens.** List them under the app's root:

```xml
<subscreens-item name="Inbox" location="component://basic-comms/screen/BasicComms/Inbox.xml" menu-index="2"/>
<subscreens-item name="Chat" location="component://basic-comms/screen/BasicComms/Chat.xml" menu-index="3"/>
```

To keep one Comms in the launcher, the app's `MoquiConf.xml` hides BasicComms
(`<subscreens-item name="BasicComms" menu-include="false"/>` under `apps.xml`; `/qapps/BasicComms` still works) and sets
`comms_app_root` to its own root, for the links in notifications and mail. A screen's own links are relative to the
mount (the `appRoot` its root sets, `mScreen.path`).

**screen-extend.** A file at `<app>/screen-extend/basic-comms/screen/BasicComms/<path>.xml` adds transitions,
subscreens, actions and widgets to that screen, under every mount. Named places to add to: `SettingsCards` (the
settings index's cards), `HomeRooms` and `HomeShortcuts` (Home's columns).

**Provider hooks.** The app root's `shell-providers` names services the framework calls, as the app, through
`org.moqui.impl.ShellServices.call#Providers` (each hook's contract is a `type="interface"` service in
`org.moqui.comms.ProviderInterfaces`):

| Hook | BasicComms asks it for |
| --- | --- |
| `inbox-source` | Every Inbox, Compose, MessageDetail and Home operation (`run#Operation`): its answer is the whole page, so an app's own messages and mailboxes are one list with the mail. Without one, the framework answers over the user's mail |
| `record-lookup` | A record room's title and link, a message's record cards, the records to file a message on |
| `record-link` | Filing a chat message on a record |
| `create-from` | Making a record (a task) from a chat message |

The other hooks (`recipient-source`, `invitation`, `contact-card`, `meetings-source`, `call-context`, `home-source`,
`external-conversation`) are declared for the work after this one. A provider runs with the app's authorization and
entity filters; a read that fails is left out and named, and a mutation that fails is an error.

**Declared actions and panels.** `<ext-action>` and `<ext-panel>` in a screen-extend add items to a slot of a screen
([Qvt-Client](../ui/Qvt-Client.md#declared-actions-and-panels)). Chat's slot is `chat-message-actions`, whose item is
`{chatRoomId, eventSeq}`; IronStone adds Create a task, request and event there, each with `hook="create-from"` so a
user without that provider is not offered it.

IronStone BOSS is the worked example: `runtime/component/mr-boss/screen/Comms.xml` (the mounts and providers) and
`screen-extend/basic-comms/`.

## Tests

`./moq test basic-comms`: every screen in qvt and tui, the mail settings and Inbox of a user with no party, chat, the
provider hooks and the app's parts through both mounts. IronStone's own parts are in its specs (`Inbox`, `Compose`,
`Chat`, `CommsHome`, `InboxMailboxes`, `MailAccounts`, `MailSettings`).
