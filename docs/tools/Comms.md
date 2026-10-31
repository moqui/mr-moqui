# Comms

BasicComms is the framework's Comms app at `/qapps/BasicComms`: the Inbox and Compose over the user's mail accounts,
MessageDetail, Home, chat (rooms, direct messages, threads, the tray's Chat tab), calls, and the mail, chat and call
settings. It works with only the framework. An app that has more (IronStone BOSS: internal messages, mailboxes, contact books,
records) mounts the same screens under its own root and adds its parts, without a copy of them. The design and its
order of work are in [plans/Comms-Framework.md](../../plans/Comms-Framework.md); voice and chat bridges come later
there.

The code: `runtime/base-component/basic-comms` (screens, services, data, specs); the data model is in
`framework/entity` (`EmailEntities` in `BasicEntities.xml`, `ChatEntities.xml`, `CallEntities.xml`); the mail client is
`org.moqui.impl.EmailClientServices` and `moqui-mail`; chat is `org.moqui.impl.ChatServices`; calls are
`org.moqui.impl.CallServices`, with the media in `moqui-rtc` ([WebRTC](../interfaces/WebRTC.md)).

## For users

- **Inbox.** Every mail account's Inbox together, or one account, folder or label; search with Gmail's operators;
  flag, snooze, archive, move, label and spam; rules, identities and signatures, out of office and text blocks in
  Settings. Add an account in Settings, Mail accounts (Gmail and Microsoft sign in with the provider where the server
  is set up for it: [Mail-OAuth](../interfaces/Mail-OAuth.md)).
- **Compose.** A new message, a reply or a forward from an account the user may send from, with undo send and
  scheduled sending.
- **Attach from Files and Save to Files.** Compose's Attach from Files opens the drive picker: the user's drives (My
  files first), the recent files, a folder or a search by name. A picked file is copied onto the message; a file over
  25 MB, or a folder, goes in as a view link instead. An attachment in the Inbox has Save to Files, which copies it into
  the folder shown, as "name (2)" when the name is taken; the message keeps its own copy. The picker shows only what
  the drives let the user open (`org.moqui.comms.FilePickerServices` over `FileServices`), and a user with no drive
  sees neither button.
- **Chat.** Rooms (private, public, direct), threads, mentions, reactions, polls, pinned messages and files; presence
  and status in the account menu. The tray's Chat tab is the same view, smaller.
- <a id="calls"></a>**Calls.** Video and audio calls with one person or several, and a chat room's call that its
  members join. New call in Calls, or Call from a room. A call rings in every tab of the callee, with Answer, Answer with
  audio only and Decline in the ring bar, and by Web Push on a device whose page is closed. In a call: mute, the camera,
  share the screen, raise a hand, reactions, the participants, the grid or the speaker view (the one speaking, or a
  screen share, large), and Leave; the host of a group call can end it for everyone. A person whose bandwidth is too low
  gets audio only, and their tile says so. Going to another screen keeps the call in the small window at the bottom
  right. Calls lists the calls ringing now and the history (All, Missed, Placed, Answered) with Call back; Home counts the
  missed calls until the Missed tab is opened.
- **Call links.** A link to a meeting for people with no account (Calls, Call links): they open it in a browser and give
  their name, then wait in the lobby until someone in the call lets them in (or come straight in, as the link says).
  The link's address is shown once, when it is made; Open starts its meeting with you as host, and Revoke takes its
  guests out. Calls also lists your meetings of the next week from an app that has them (IronStone: your calendar),
  with Join. Settings, Calls has the microphone, camera and speaker, the ringtone, and whether calls start muted
  or with the camera off, for this browser. Calls need a media server (`rtc_media_port`); the Calls screen says when
  there is none. In the terminal a call is not live: its screen shows who is in it, with Answer, Decline and Leave.
- **Recording.** When the server allows it (`call_recording_enabled`), the host of a call has a Record button: start,
  pause, resume and stop. Everyone in the call sees that it is recorded, before they join too, and can withdraw their
  consent, which pauses the recording. Where the policy asks for it, each person acknowledges the notice, and the
  recording waits until all have. Calls, Recordings lists the recordings you made and those of calls you host: play one
  part with everyone's audio and video together, download each file, and delete your own. The terminal lists them with
  Delete. Recordings are deleted after the server's retention period ([Call recording](../interfaces/Call-Recording.md)).
- **Captions.** Where the administrator turned them on (`call_captions_enabled`) and a speech to text model is set up,
  anyone in a call can press CC. Everyone in the call, including people who join later, sees that captions are on and
  which service hears what they say; each line names its speaker. When the service is slow or down, the captions say
  they are unavailable and the call goes on. Captions are not kept unless the administrator asks for that
  (`call_captions_store`). An end-to-end encrypted call has no captions. See [WebRTC, Captions](../interfaces/WebRTC.md#captions).
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
settings index's cards), `HomeRooms` and `HomeShortcuts` (Home's columns). A transition of the same name replaces
the screen's: IronStone replaces the drive picker's `pickerFolder` in `Inbox/Compose` and `Inbox/Mail` to name its
records drive's folders by their records.

**Provider hooks.** The app root's `shell-providers` names services the framework calls, as the app, through
`org.moqui.impl.ShellServices.call#Providers` (each hook's contract is a `type="interface"` service in
`org.moqui.comms.ProviderInterfaces`):

| Hook | BasicComms asks it for |
| --- | --- |
| `inbox-source` | Every Inbox, Compose, MessageDetail and Home operation (`run#Operation`): its answer is the whole page, so an app's own messages and mailboxes are one list with the mail. Without one, the framework answers over the user's mail |
| `record-lookup` | A record room's title and link, a message's record cards, the records to file a message on |
| `record-link` | Filing a chat message on a record |
| `create-from` | Making a record (a task) from a chat message |

| `meetings-source` | The user's meetings in a range, for the Calls screen's Meetings (IronStone: the calendar's) |

The other hooks (`recipient-source`, `invitation`, `contact-card`, `call-context`, `home-source`,
`external-conversation`) are declared for the work after this one. A provider runs with the app's authorization and
entity filters; a read that fails is left out and named, and a mutation that fails is an error.

**Declared actions and panels.** `<ext-action>` and `<ext-panel>` in a screen-extend add items to a slot of a screen
([Qvt-Client](../ui/Qvt-Client.md#declared-actions-and-panels)). Chat's slot is `chat-message-actions`, whose item is
`{chatRoomId, eventSeq}`; IronStone adds Create a task, request and event there, each with `hook="create-from"` so a
user without that provider is not offered it.

IronStone BOSS is the worked example: `runtime/component/mr-boss/screen/Comms.xml` (the mounts and providers) and
`screen-extend/basic-comms/`.

## Tests

`./moq test basic-comms`: every screen in qvt and tui, the mail settings and Inbox of a user with no party, the drive
picker (`BasicCommsFiles`), chat, the call screens and recordings (`BasicCommsCalls`), the provider hooks and the app's parts through both mounts. IronStone's own parts are in its specs (`Inbox`, `Compose`,
`Chat`, `CommsHome`, `InboxMailboxes`, `MailAccounts`, `MailSettings`).
