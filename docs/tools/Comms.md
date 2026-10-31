# Comms

BasicComms is the framework's Comms app at `/qapps/BasicComms`: the Inbox and Compose over the user's mail accounts,
MessageDetail, Home, chat (rooms, direct messages, threads, the tray's Chat tab), calls, and the mail, chat and call
settings. It works with only the framework. An app that has more (IronStone BOSS: internal messages, mailboxes, contact books,
records) mounts the same screens under its own root and adds its parts, without a copy of them. The design and its
order of work are in [plans/Comms-Framework.md](../../plans/Comms-Framework.md); phone calls through SIP trunks are there
(V1), voicemail (V2), call handling, ring groups and the dialer (V3); chat bridges come later.

The code: `runtime/base-component/basic-comms` (screens, services, data, specs); the data model is in
`framework/entity` (`EmailEntities` in `BasicEntities.xml`, `ChatEntities.xml`, `CallEntities.xml`); the mail client is
`org.moqui.impl.EmailClientServices` and `moqui-mail`; chat is `org.moqui.impl.ChatServices`; calls are
`org.moqui.impl.CallServices`, with the media in `moqui-rtc` ([WebRTC](../interfaces/WebRTC.md)); phone calls are
`org.moqui.impl.VoiceServices` (`VoiceEntities.xml`), with the SIP engine in `moqui-sip` and the host's `sip` feature
([SIP](../interfaces/SIP.md)); call handling, ring groups and schedules are `org.moqui.impl.CallHandlingServices`.

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
  recording waits until all have. A phone in the call hears a tone instead, and presses 1 to acknowledge or hangs up to
  withdraw; it hears another tone when captions turn on. Calls, Recordings lists the recordings you made and those of calls you host: play one
  part with everyone's audio and video together, download each file, and delete your own. The terminal lists them with
  Delete. Recordings are deleted after the server's retention period ([Call recording](../interfaces/Call-Recording.md)).
- **Captions.** Where the administrator turned them on (`call_captions_enabled`) and a speech to text model is set up,
  anyone in a call can press CC. Everyone in the call, including people who join later, sees that captions are on and
  which service hears what they say; each line names its speaker. When the service is slow or down, the captions say
  they are unavailable and the call goes on. Captions are not kept unless the administrator asks for that
  (`call_captions_store`). An end-to-end encrypted call has no captions. See [WebRTC, Captions](../interfaces/WebRTC.md#captions).
- <a id="phone"></a>**Phone calls.** Calls, Dialer (also the tray's Dialer tab, in IronStone): type a number or
  use the keypad, or type a name to find a person with a number (the app's contacts, or here the people one of our
  numbers rings); the dialer shows the number as it will be called (with its country code), your recent numbers, and
  which of your numbers (or your ring groups') calls from; Hide my number withholds it. Calls, Call a number does the
  same from a form: a phone number (with its country code, or as dialed at home), from one of your numbers. The call opens like any other, audio only; the phone shows as a participant with the
  number, and the keypad sends tones (for a menu or a PIN). A call to one of your numbers rings you as a call does, with
  the caller's number. An administrator allows each person to call out and gives them the numbers they call from;
  international and premium numbers stay off until allowed for the server and for you, and there are limits on calls at
  once and in a day. An emergency number is always tried first: it goes through a trunk that takes emergency calls, from
  your registered location with its callback number, and tells the location's contact; when any of that is missing the
  call is refused and says what is missing, so call from another phone.
- <a id="voicemail"></a>**Voicemail.** A call to your number goes to your voicemail when you don't answer within your
  rings, when you are in another call, or when you decline it. The caller hears your greeting (or the default one), a
  beep, and leaves a message of up to two minutes; `#` ends it. You are told at once (here, and on your devices by Web
  Push), and by email when you ask for it. Calls, History, Voicemail lists them, unheard first in bold, with the Inbox
  and the Trash; open one to play it, read its transcript (when the server transcribes voicemail), mark it heard or
  unheard, call back, download or delete it. Deleted messages wait in the Trash for 30 days (`voicemail_trash_days`),
  then go with their audio. Settings, Phone has your greeting (record it in the browser or upload a WAV or Ogg file),
  your PIN, the email notice and the number of rings. With a PIN you can listen by phone: call your number and press `*`
  during your greeting (or call the access number and enter your number and `#`), then the PIN and `#`; each new
  message plays, then `7` deletes it, `9` saves it and `#` skips to the next. The email notice has only a link unless you
  turn on the audio or the transcript in it; turning one on warns that mail cannot be recalled. See
  [Voicemail](../interfaces/Voicemail.md).
- <a id="call-handling"></a>**Call handling.** Settings, Call handling: do not disturb (calls go straight to
  voicemail; it is your status, the same as the account menu's), call waiting (off: a call while you are in one goes to
  voicemail or your busy rule), ring my mobile too (with its number, and only while a schedule is open if you choose),
  Hide my number, and your rules, read in order: reject callers with no number; reject, accept only, or forward the
  callers on one of your lists; forward all calls; when busy; when not answered. Each goes to a number, a person, a ring
  group, voicemail or a refusal, and can apply only while a schedule is open or closed (business hours, holidays, in the
  schedule's time zone). Your caller lists are numbers, or prefixes ending in `*`. Calls go to outside numbers (your
  mobile, a forward) only when an administrator allows it for you; a forward back to the caller or round in a loop is
  skipped. See [SIP, Call handling](../interfaces/SIP.md#call-handling).
- <a id="desk-phones"></a>**Desk and soft phones.** Settings, Desk and soft phones: Add a phone gives a username,
  a password, the domain, the port and the transport, each with a copy button, shown only then; put them in the desk
  phone or in a phone app (Linphone, Zoiper, Groundwire), choosing TLS and SRTP where it has them. Once it registers,
  the list shows it with its address and how long the registration lasts. A call to you rings the phone beside your
  tabs, and whichever answers first takes it; declining on one declines on all (a phone busy on another call only stops
  ringing itself), and a call nobody answers goes to your voicemail after your rings. The phone calls out as you, to a number (with your caller ID and your
  limits) or to a colleague by username. New password makes new credentials and stops the old ones at once; Remove
  ends the phone. Phones register only when the administrator has turned the registrar on (`sip_registrar_enabled`);
  until then the page says so. A phone that has no SHA-256 sign-in takes the legacy profile (MD5). See
  [SIP, Registrar](../interfaces/SIP.md#registrar).
- <a id="voice-admin"></a>**Voice administration** (System, Voice; the VoiceAdmin permission, which ADMIN has). Status:
  whether phone calls are supported (the `sip` feature), configured and active, the SIP and media servers. Trunks: the
  provider's host and credentials (the password is never shown), TLS with SRTP unless the profile of a PBX on a private
  network allows UDP, TCP or plain RTP, whether it takes emergency calls, and its status and last error. Numbers: each
  number's trunk, the person or ring group it rings (or the voicemail access number) and its emergency location. Caller IDs: who may call from which number. Fraud: the
  limits, each person's permissions (with Forwards: whether their calls may go to outside numbers), and the sources blocked after failed SIP sign-ins (a block never locks the account;
  Clear lifts it). Emergency: the readiness checklist and the emergency locations (address, callback number, who is
  told, the provider's reference and whether it is validated). Phones: everyone's desk and soft phones with their
  registrations, whether the registrar is on, and Remove. Ring groups: each group's members in order, how it rings
  (all at once, in order, round robin), seconds each, its hours, and what happens when nobody answers (the group's
  voicemail, which its members share; another group; an outside number; the end). Schedules: the business hours, dated
  hours and holidays (once or each year) with their time zone, and whether each is open now. The screens are basic-comms' (`screen/System/`), mounted
  into the tools System app; in the terminal they are the same forms.
- **Home.** Unread by account, flagged, snoozed, mentions, mail that was not sent, missed calls, unheard voicemail (once
  you have a voicemail box), and rooms with unread.
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
| `recipient-source` | People with phone numbers for the Dialer's search (`kinds` phone; IronStone: the parties and contacts the user may see) |

The other hooks (`invitation`, `contact-card`, `call-context`, `home-source`,
`external-conversation`) are declared for the work after this one. A provider runs with the app's authorization and
entity filters; a read that fails is left out and named, and a mutation that fails is an error.

**Declared actions and panels.** `<ext-action>` and `<ext-panel>` in a screen-extend add items to a slot of a screen
([Qvt-Client](../ui/Qvt-Client.md#declared-actions-and-panels)). Chat's slot is `chat-message-actions`, whose item is
`{chatRoomId, eventSeq}`; IronStone adds Create a task, request and event there, each with `hook="create-from"` so a
user without that provider is not offered it. Calls, Voicemail's slot is `voicemail-actions`, whose item is
`{voicemailId, fromNumber, callSessionId}`, for actions such as Create from and Add to contacts.

IronStone BOSS is the worked example: `runtime/component/mr-boss/screen/Comms.xml` (the mounts and providers) and
`screen-extend/basic-comms/`.

## Tests

`./moq test basic-comms`: every screen in qvt and tui (the System Voice screens too), the mail settings and Inbox of a
user with no party, the drive picker (`BasicCommsFiles`), chat, the call screens and recordings (`BasicCommsCalls`), the
provider hooks and the app's parts through both mounts. `./moq test framework --spec Voice`: caller ID, international
and premium, the limits, failed sign-ins blocked per source, emergency readiness and routing, calls coming in and the
phone's events. `./moq test framework --spec Voicemail`: busy and unanswered calls to voicemail, private audio, the PIN
and its limits, notices, transcription and its retry, and the trash. `./moq test framework --spec SipDevice`: desk and
soft phones (credentials once, rotation, revocation, ringing beside the tabs with the first answer winning, voicemail
after the tabs and phones ring or a phone declines, a phone's own call). `./moq test framework --spec CallHandling,RingGroup`:
the rules, schedules across a time zone, forward loops and permissions, do not disturb and call waiting, simultaneous ring,
the strategies, the shared box, and the first answer winning. IronStone's own parts are in its specs (`Inbox`, `Compose`,
`Chat`, `CommsHome`, `InboxMailboxes`, `MailAccounts`, `MailSettings`).
