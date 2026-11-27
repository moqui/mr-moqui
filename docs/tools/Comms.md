# Comms

BasicComms is the framework's Comms app at `/qapps/BasicComms`: the Inbox and Compose over the user's mail accounts,
MessageDetail, Home, chat (rooms, direct messages, threads, the tray's Chat tab), calls, and the mail, chat and call
settings. It works with only the framework. An app that has more (IronStone BOSS: internal messages, mailboxes, contact books,
records) mounts the same screens under its own root and adds its parts, without a copy of them. The design and its
order of work are in [plans/archive/Comms-Framework.md](../../plans/archive/Comms-Framework.md); phone calls through SIP trunks are there
(V1), voicemail (V2), call handling, ring groups and the dialer (V3), the IVR, queues, transfer and merge (V5); chat
bridges come later.

The code: `runtime/components/base/basic-comms` (screens, services, data, specs); the data model is in
`framework/entity` (`EmailEntities` in `BasicEntities.xml`, `ChatEntities.xml`, `CallEntities.xml`); the mail client is
`org.moqui.mail.EmailClientServices` and `moqui-mail`; chat is `org.moqui.chat.ChatServices`; calls are
`org.moqui.call.CallServices`, with the media in `moqui-rtc` ([WebRTC](../interfaces/WebRTC.md)); phone calls are
`org.moqui.call.VoiceServices` (`VoiceEntities.xml`), with the SIP engine in `moqui-sip` and the host's `sip` feature
([SIP](../interfaces/SIP.md)); XMPP clients reach chat through `moqui-xmpp`, the host's `xmpp` feature and
`org.moqui.chat.XmppServices` (`XmppEntities.xml`, [XMPP](../interfaces/XMPP.md)); call handling, ring groups and schedules are `org.moqui.call.CallHandlingServices`; IVR
menus, queues and transfers are `org.moqui.call.IvrServices`, `CallQueueServices` and `CallTransferServices`.

## For users

- **Inbox.** Every mail account's Inbox together, or one account, folder or label; search with Gmail's operators;
  flag, snooze (and Unsnooze), archive, move, label and spam; in Trash, Delete forever asks first; rules, identities
  and signatures, out of office and text blocks in Settings. A member who may only read a shared account sees only Mark
  read and unread on its mail. An automatic reply for "my contacts and people inside" reaches people inside only when
  no address book (contact hook) is installed, and the screen says so. Add an account in Settings, Mail accounts (Gmail and Microsoft sign in with the provider where the server
  is set up for it: [Mail-OAuth](../interfaces/Mail-OAuth.md)).
- **Compose.** A new message, a reply or a forward from an account the user may send from, with undo send and
  scheduled sending. A message may go to Bcc only (its To is `undisclosed-recipients:;`). A send refused because the
  right to send from the account was taken away says so, and the message stays open. Compose's `saveDraft`, `upload` and `send` take a `clientRef`, the client's own reference of the
  draft, so a client that queued the mail offline retries them after a lost answer safely: one draft is saved, a file is
  attached once, and a send that went through answers `duplicate` true (`EmailMessage.clientRef`, found only among the
  user's own messages; `clientApi` 3). A send that failed is not sent again by its ref (it may have gone out); the user
  sends it. A client runs its queued operations of one ref one after another; a late retry after the draft was deleted
  makes a new one. `composeData` answers a draft's `clientRef`, so a client that reopens a draft offline queues its
  send by the ref the server keeps. An internal message is not queued: its send is not taken once by a `clientRef` yet,
  so a client keeps it as a draft until it has a connection ([Desktop-Client](../ui/Desktop-Client.md#the-outbox-and-drafts)).
  Since `clientApi` 4: `saveDraft` and `send` take `baseStamp` and answer `conflict` with the server's draft when it was
  saved elsewhere since, a file may come in chunks (`uploadChunk`, `uploadStatus`, then `upload` by `uploadId`), the
  Inbox has the change cursor `mailChanges`, its `state` takes a `clientRef` and each target's `seen`, and Chat's edit,
  delete, reaction, vote and mark-read take a `clientRef` ([Desktop-Client](../ui/Desktop-Client.md#the-servers-sync-contracts)).
- **Attach from Files and Save to Files.** Compose's Attach from Files opens the drive picker: the user's drives (My
  files first), the recent files, a folder or a search by name. A picked file is copied onto the message; a file over
  25 MB, or a folder, goes in as a view link instead. An attachment in the Inbox has Save to Files, which copies it into
  the folder shown, as "name (2)" when the name is taken; the message keeps its own copy. The picker shows only what
  the drives let the user open (`org.moqui.comms.FilePickerServices` over `FileServices`), and a user with no drive
  sees neither button.
- **Chat.** Rooms (private, public, direct), threads, mentions, reactions, polls, pinned messages and files; presence
  and status in the account menu. The tray's Chat tab is the same view, smaller. New direct message (on a phone, Start
  chat) takes one person, several (with a name they become a private room), or, for a user with a number that texts, a
  phone number typed in its search, which opens that number's room as Calls' Text a number does. On a phone a swipe to
  the right on a message replies to it, Attach offers the camera, the gallery or a file, and Back closes a menu or sheet,
  then a pane or thread, then the room. A message's actions are one list wherever they show: a long press on a phone
  opens them as a sheet, a right click on a desktop row (not on a link, an editor or selected text, which keep the
  browser's menu) opens them at the pointer, and the hover toolbar's More has them too. The sheet and the right-click
  menu start with the quick reactions and "+", which opens the emoji picker (a search by name, the groups, and your
  recent emoji, kept in this browser); then Reply, Reply in thread, Copy text, Copy link, Forward, Edit, Pin, Mark
  unread from here, the apps' actions, File on a record, Info, Who reacted (with reactions) and Delete. A tap on a
  reaction adds or takes back yours; a long press on it (or a hover, or Shift+Enter or the menu key on it) lists who
  reacted, and the same on a poll option lists who voted. On a phone the picker's search waits for a tap, so the
  keyboard does not cover the sheet. **Info** says only
  what the server knows: when the message was sent, for a message that went to another network (SMS, a bridge) its
  delivery there per network (with the bridge's name when two share a network), and who has read up to it (the
  members' read markers, "and N more"; not one who joined after it). A member with read
  receipts off is not listed, and with your own off you see no names: Info says so, and never "not read". A thread has
  no read markers, so its Info shows sent and delivery only; delivery to someone's devices is not known and is never
  shown. The terminal's room has Info for each message, as text. When the last owner leaves, the
  longest-standing moderator (else member) becomes an owner: Leave names them, and the room says they became an owner
  when the owner left.
- <a id="room-details"></a>**A room's details.** The room's header (Media, files and links; on a phone its menu, or the
  members' faces) opens its details: full screen on a phone, the right pane on a desktop, with tabs. **People** (the
  members, Add people, Verify in an encrypted room), **Media** (the pictures and videos as a grid; a tap opens the
  viewer, where a swipe, the arrows or Left and Right go to the next and the previous), **Files** (all but pictures,
  videos and sounds), **Voice** (the voice notes and other sounds, each with its player), **Links** (every link of the
  messages, the newest first, with the title of its preview when it had one), **Reactions** (the messages with
  reactions, the latest reaction first, or only those to your messages) and **Pinned**. Each loads more as it scrolls;
  Show in chat opens a message (in its thread when it is a reply there). Threads and Search stay panes of their own. In
  an encrypted room the server knows no file's type and no link, so this browser sorts the files into the tabs and lists
  the links of the messages it has opened. **Voice notes:** with no text in the message box, its Send button is a
  microphone; a tap records (the time shows, with Cancel and Send; ten minutes at most, then it is sent), and Send sends
  it alone as a sound file (text typed meanwhile stays in the box), which plays in the message and in Voice. **Activity** across rooms (your mentions, replies to you,
  replies in your threads and reactions to your messages) is the `activity` transition, which the Activity and Threads
  views build on (below). The terminal has a room's Links, your Activity and your Threads as lists; the media grid, voice and reactions are
  the browser's. The lists are the services `find#RoomFiles`, `find#RoomLinks`, `find#RoomReactions` and
  `find#ChatActivity` of `org.moqui.chat.ChatServices`, which page by a cursor that never cuts a message in two.
- <a id="chat-desktop"></a>**Finding your way in Chat** (Comms-Messaging §3). **The palette** (Search, Ctrl+K or ⌘K, on
  the web and in the desktop and mobile app) has Chat rooms: your rooms and the public ones by name, and a phone
  number's room by its digits. While Comms is the current app (always in the app's shell) the rooms come first with no
  `#`, so two letters and Enter open a room; `#` keeps the rooms alone anywhere. People to message, the people you have
  no direct chat with, come after Screens and never first: choosing one opens a message to them, and the direct chat is
  made only when you send it. Chat adds no key of its own. **Activity** and **Threads** are entries beside the
  filters: Activity lists your mentions, replies to you and reactions to your messages across rooms, newest first;
  Threads the threads you started or wrote in (`find#ChatThreads`), one row each, the latest reply first. Each loads more
  as it scrolls, keeps what it loaded when something new comes in at the top, and opens the message, or the thread; an
  encrypted room's text is decrypted in the page. **All** groups the rooms: Pinned, Recent, Phone (a phone number's rooms) and Bridged (rooms a bridge carries
  to another network); a group's header folds it, kept per user in this browser. Phone is a filter too, and the phone's
  home has All and Phone. **Mail on a person's row:** a person's rooms (below) show their latest mail thread of the last
  180 days in the mail you may read (your accounts and those shared with you), as a line "Email: subject", which says
  "Email is not end-to-end encrypted" and opens the thread in the Inbox (a link: Tab and Enter reach it). A user of this
  server matches by their user account's mail address (not a mail account they own or share) and the addresses you
  added to them; an outside person by their contact's addresses and those you added. Mail stays in the Inbox: there are
  no mail-only rows (`find#DirectChatMail` of `org.moqui.comms.ChatScreenServices`, the `mailLines` transition, one query
  for the list).
- <a id="people-across-networks"></a>**People across networks** (plans/Comms-Messaging.md §11.1, phase 22). Your rooms
  with one person on different networks are that one person: a direct chat with a user of this server, a phone number's
  SMS and RCS rooms, a GroupMe direct conversation. They are grouped by your own changes and by the contacts you can see
  in the apps (with Mantle: your personal contacts, your books and those shared with you, your organizations'
  directory). Each room stays a row of its own, with its network's icon and label, and shows the person's name. In a
  person's room the composer starts with a **network button**: it lists their ways, the best first (MR Moqui chat for a
  user of this server, end to end encrypted when they have a device; else the network they last wrote on; else their
  text room with the latest message from either side, or a new text from your number; else mail), each with what it
  encrypts. Choosing one opens that room, or starts it, with your draft added after the draft it already has; a draft
  moved out of an encrypted chat says so, and a draft with files attached does not move (send or remove the files
  first). A text or GroupMe room of a user of this server offers "chat there" above the composer. The People tab starts
  with the person's **card**: Rename, Add (another conversation of theirs, a number or a mail address; a user of this
  server takes only a number or address of one of your conversations), Not this person on each address, Undo my
  changes, Open the contact. Your people are your own: nobody else sees them or is told. A user's phone number is never
  shown through chat: of a user of this server you see only the numbers of your own rooms with them. A number on your
  Blocked list, one that replied STOP or a bridge that is not connected shows dimmed with its reason and is never the
  best; a person you blocked has no ways, and a user who blocked you, or whom an app keeps from you, is as one that does
  not exist (no name or addresses from the contacts, no new chat). While the contacts cannot be asked (an hour's 60
  asks used, or the app's hook failing), Rename and Add say to try again shortly
  (`org.moqui.chat.ChatPeopleServices`, the `people` transition).
- <a id="encrypted-rooms"></a>**Encrypted rooms** (C7). A new direct chat or private room is encrypted when everyone in it
  has a crypto device (Chat makes your browser's when it opens); otherwise it is plain, with the plain note. A direct chat
  you already have keeps opening; when it is plain and the other person has a device, its menu and its start offer Start
  an encrypted chat, which opens a new encrypted chat that links back to the plain one, and from then on the encrypted
  one opens. New room's Encrypted follows the same rule until you set it. Its messages, reactions and files are encrypted in your browser and read only on the
  devices of its members; the room shows a lock, and the room list says "Encrypted message". It cannot be turned off,
  and it gives up what needs the server to read it: search, link previews, agents, polls, bridges, forwards, filing on
  records and Create from; the terminal lists it but cannot read or write it. Set up the key backup in Settings, My
  devices, and keep the recovery key it shows once: a new browser reads older messages only after Restore with it (or
  when another of your devices shares the keys). My devices also verifies your devices (compare emoji, or scan a QR
  code), and says when it waits for the other device. Search says an encrypted room is not searched. Open the room in any tab: the tab that holds this browser's keys does the work for the others. Every call in an
  encrypted room is end-to-end encrypted, a direct one too: its Room settings show the calls' setting on, and a phone
  or a guest cannot join. With only verified devices on, your message says who cannot read it, with Verify; and a
  message you cannot read says when its sender has not verified your device. See [Chat
  keys](../interfaces/Chat-Keys.md#encrypted-rooms).
- <a id="e2ee-labels"></a>**What a room's lock means.** A lock means end-to-end encrypted, and only that: an encrypted
  room has it in its header and in the room list, with a check when every device in it is verified, else Verify in its
  note. Every other room has the open lock, and a note under its header (close it with its x) says what can read its
  messages: the server, for a plain private room or a direct chat; the phone network and the SMS provider, for a
  phone number's texts (its composer says Text message); the bridge, for a room bridged to another network (its composer
  names the network). A public room has the open lock and no note. A private room's own icon is people, not a lock. The
  members pane and Room settings say the same for every room, and the terminal shows it as one line. Mail never has a
  lock: a private mailbox or list shows people, a closed thread a crossed-out comment.
- <a id="verify-people"></a>**Verifying people.** In an encrypted room, Members has Verify by each person who is not
  verified (the room's note and a message that someone cannot read have it too): pick their device and compare emoji
  with them; when the emoji match on both sides their device is verified, and with cross-signing every device they sign.
  A verified person has a check by their name, in Members and on their messages. When someone you verified adds a device
  you have not verified, or gets a new identity, your next message, edit or reaction waits: Verify them (by each name),
  or Send anyway, which lets that device read this room's messages from then on, or uses that identity here, without
  verifying them: "Send my room keys only to verified devices" still leaves such a device out. A request from another
  person to verify shows in the room you have open (closing it says no) and on My devices. A message from a verified
  person's device that is not verified has a warning mark instead of the check.
- <a id="bridged-rooms"></a>**Bridged rooms** (C1). A room an administrator connected to another network (SMS, WhatsApp,
  XMPP, Matrix and others, as their adapters come) has the network's mark in the room list and its name in the room's
  header ("Messages here also go to ..."). People from that network are members with the network's badge, and their
  messages carry it. Each of your messages says whether it was delivered there, is being sent, was not delivered (with
  why) or may not have arrived; the sender or a moderator can Send again. When the bridge is not connected the room says
  so and keeps its messages, which go out once it is back. An edit, a delete or a reaction the network cannot carry is
  not offered. The terminal shows the same, with Send again. Room settings, Bridges lists where the room goes; a bridge
  administrator connects it to another network there (the bridge, the room or conversation there, which way messages
  go), changes or ends it. See [Chat bridges](../interfaces/Chat-Bridges.md).
- <a id="groupme"></a>**GroupMe** (not yet tested with GroupMe). Settings, Chat, GroupMe connects your own GroupMe
  account with its access token (from dev.groupme.com): each of your groups, Topics ("Group › Topic") and direct
  conversations becomes a room you own, under Bridged. A heart is a like there and back, `@room` is `@all`, and pictures
  go both ways; other files and reactions are not sent. Only your own messages go out, from your account. Disconnect
  stops the syncing and forgets the token; the rooms keep their history. GroupMe has no end-to-end encryption, and the
  room says so. See [GroupMe](../interfaces/GroupMe.md).
- <a id="texts"></a>**Texts** (C4-SMS). A number of ours with messaging has one room for each outside number it texts
  with (the SMS icon, named their number): its texts, pictures, calls and voicemail are there, and everyone our number
  reaches is in it. Calls, Text a number opens it for a number you are granted; a reply goes from that number. A person
  who texts STOP gets nothing more from it until they text START; international and premium numbers stay off until an
  administrator allows them. See [SMS](../interfaces/SMS.md). A number on an RCS bridge texts as the brand's RCS
  agent, with SMS from the number when the phone has no RCS; each message's caption and Info say when it went as SMS
  or MMS ([RCS](../interfaces/RCS.md)). Its header says "RCS Business - not end-to-end encrypted" and its banner who can
  read it; a message the phone read says "Read on RCS"; the person sees the agent typing while you type.
- <a id="cards"></a>**Cards and buttons.** The composer's Card (beside Poll, not in an encrypted room) sends a card: a
  title, words, a picture and up to 4 buttons, each a reply, a link or a call; with no title and no picture it is a
  message with buttons under it. The dialog shows the text a phone without RCS gets. Apps send cards and carousels too
  (`ChatRichServices.send#Rich`). A card goes as RCS cards on an RCS bridge and as that text (and its picture) on SMS
  and every other bridge. Tap a reply on someone else's card to answer with its words (in a bridged room the buttons
  are the other person's to tap, on their phone); a link opens, a call dials. On a phone the buttons of a message are
  one row that scrolls sideways; on a desktop they wrap. A card is not edited.
- <a id="xmpp"></a>**XMPP apps.** When the server has XMPP on (`xmpp-server@domain`), the people of the server can use an XMPP
  app (Conversations, Monal, Dino, Gajim, a web client over `/xmpp-websocket`) with their chat. Settings, Chat apps
  shows the address to sign in as (`username@domain`; an email-style username has its `@` written as `\40`), the
  server and its ports, and the app passwords, each saying whether it works with XMPP apps (one made before XMPP
  sign-in has no SCRAM verifiers, and most apps refuse it), with New app password for XMPP. Calls between XMPP apps get
  a TURN relay only when the administrator turned it on (`xmpp-server@turn-enabled`), and never for an external user. The app sees the rooms as group chats on `rooms.<domain>` (joined from its bookmarks, which list the rooms),
  the people of direct messages as contacts, the history of both, reactions, replies, edits and deletes, files, typing
  and read markers, and the same presence as the web. A room that is end-to-end encrypted in Moqui does not open in an
  XMPP app. Messages two XMPP apps encrypt with OMEMO stay between them: the web shows "Encrypted message (OMEMO)". See
  [XMPP](../interfaces/XMPP.md).
- <a id="calls"></a>**Calls.** Video and audio calls with one person or several, and a chat room's call that its
  members join. New call in Calls, or Voice call and Video call in a room's header (in the tray's Chat tab, its menu): a
  direct room calls the person, a group room starts a call its members join from the room, and an encrypted room's call
  is end-to-end encrypted. A call rings in every tab of the callee, with Answer, Answer with
  audio only, Decline and Decline with a message (a few ready texts, or your own, sent to the caller in your direct room)
  in the ring bar, and by Web Push on a device whose page is closed. On a phone the call takes the whole screen: the
  other person's picture for a voice call, their video with yours in a corner for a video call, and mute, speaker (where
  the browser can choose), video, switch camera, add people and hang up; on a desktop the call shows in a panel at the
  right of the screen you are on, with Open full size for the grid. Turn on video in a voice call, or turn it off in a
  video call: it is the same call, and the others are asked whether to do the same. Picture in picture puts the call in
  a small window over your other windows (Chrome and Edge), else in the small window at the bottom right of the page.
  In a call: mute, video,
  share the screen, raise a hand, reactions, the participants, the grid or the speaker view (the one speaking, or a
  screen share, large), and Leave; the host of a group call can end it for everyone. A person whose bandwidth is too low
  gets audio only, and their tile says so. Going to another screen keeps the call in the small window at the bottom
  right. **Move to this device:** while you are in a call on another device (or tab), every other page of yours shows it
  with Move to this device; the call moves without a gap for the others, and the first device leaves once this one is
  connected (a call going straight between two devices first moves to the server, a short gap). **Call waiting:** a
  call that rings while you are in one offers Hold and answer (the first call waits on hold, and the others in it see
  that) and End and answer; the call on hold shows at the top with Swap (or Resume) and Leave. Calls lists the calls ringing now and the history (All, Missed, Placed, Answered) with Call back (an end-to-end
  encrypted call is called back encrypted); Home counts the missed calls until the Missed tab is opened, and so does the
  header's Missed calls badge (the theme's `STRT_CALLS_SCREEN`, Calls, Missed, and the shell counts' `calls`: basic-comms
  seeds both, with the tray's Chat and Dialer tabs and the Mail button, and IronStone's theme rows replace them).
- **Block and spam** (plans/Comms-Messaging.md §18.3 phase 11). The rail's **Spam & blocked** chip lists the rooms out
  of your sight: a phone room the spam filter took, and a room you blocked. They notify nothing and count nothing; the
  room opens with a banner in place of the composer: Not spam and Report spam, or Unblock.
  - *A number.* Block in a phone room's menu, or Settings, Call handling, Blocked numbers (a number or a prefix ending
    in `*`): its calls are refused and its texts go to Spam & blocked. In a group text Block blocks that conversation
    only; one number's block does not hide its texts there. Report spam blocks the number for you; your teammates on the
    same number get the conversation in Spam only if nobody here ever replied in it, and your Unblock takes the report
    back. In a group text Report spam hides it for you alone and reports only the numbers you pick. One person's report
    is not spam for others (two people's, or an admin's, is), and your Not spam trusts the number for your own rooms and
    keeps that conversation out of Spam for good. Filter spam texts (Settings, Call handling) turns the filter off for you
    ([SMS, Blocked numbers and spam](../interfaces/SMS.md#blocked-numbers-and-spam)).
  - *A person.* Block in a direct chat's menu, a member's menu, or Settings, Chat, Blocked people (Unblock there). Their
    messages are hidden in shared rooms (one quiet line says so, and a reply to one shows nothing of it) and notify
    nothing, mention you in nothing, count nothing and are in none of your threads, Activity or search; their typing
    does not show and their calls do not ring you (a call to you alone is refused as if you were not there). They cannot
    start a direct chat with you or add you to a private room, and are told only that you are not found. A block does not
    keep you out of a room someone else adds you to, and never refuses a public or a record room: it cannot refuse a
    manager's team rooms. Your direct chat with them goes to Spam & blocked, where it shows everything. Your XMPP
    sessions follow it for new messages and history. A chat block is not a mail block (the Inbox's Block), and neither
    stops the other.
  - *What they still see.* A blocked person still sees you in the rooms you share and can still read your messages
    there; nothing tells them they are blocked.
  - *Unblock.* Their messages show again, but the mentions and Activity they made during the block do not come back.
  - *Not covered.* A blocked person's reactions and poll votes still count in the totals; the XMPP blocklist of your
    XMPP app (XEP-0191) is its own.
- <a id="disappearing-messages"></a>**Disappearing messages and view-once** (plans/Comms-Messaging.md §18.3 phase 16).
  - *The timer.* A room's menu, its Room settings or the timer chip in its header opens Disappearing messages: Off, 30
    seconds, 5 minutes, 1 hour, 8 hours, 1 day, 1 week or 4 weeks. Any member of a direct chat sets it, and a moderator or
    owner of another room. A new message then goes that long after it is sent, read or not, on the server and on every
    member's device: it leaves the timeline, its thread, the details tabs, search results, Activity and the room list, and
    its files, links and link previews go with it. A change is a line in the room ("Ana set disappearing messages to 1
    hour"), and messages sent before keep their own time. An expiring message has a timer by its time with the time it has
    left. A bridged room, a phone room and a record's room have no timer: a bridge cannot delete a message that went to its
    network, and a record keeps its history; a room with a timer cannot be bridged until it is off. XMPP clients are not
    served a room with a timer, an expiring message is never put in the search index (without OpenSearch the database
    search finds one while it stands), it is not handed to an agent, it cannot be forwarded, and its notifications and
    pushes say only who sent it and where. In an encrypted room the server also drops the key backup of a session once no
    message of the room uses it. Anyone in the room can still copy or photograph a message before it goes; a database
    backup made while it stood keeps it until that backup is removed (a restored one deletes what is past due at once).
    Chat has no export; an administrator's entity export or data transfer holds what stood at that moment.
  - *View once.* With one photo or video waiting and no text, its chip has View once (not in a bridged room). Each
    person in the room opens it once, full screen, with no download or forward; closing it ends it, and the message
    then reads Opened. The sender sees Opened, or "Opened by 2 of 4" (people with read receipts off are not counted). The
    server deletes the file once everyone has opened it, and a view-once message nobody opened goes after 14 days
    (`chat@view-once-days`). While it is open the Android app blocks screenshots and screen recording; a browser and the
    desktop app cannot, and the viewer says so.
- **Send later and reminders** (plans/Comms-Messaging.md §18.3 phase 15; `org.moqui.chat.ChatScheduleServices`).
  - *Send later.* A long press on Send on a phone, or the arrow beside Send or a right click on it on a desktop, opens
    Send later: in an hour, tomorrow morning or afternoon, Monday morning (on a Friday or Saturday), or a date and time
    you pick (a minute to a year ahead). A line above the composer says what is scheduled in the room ("Scheduled:
    tomorrow at 8:00", or how many, and how many were not sent); it opens the list, where each message can be edited
    (its text and time), sent now or cancelled. Nobody else sees a scheduled message until it is sent.
  - *At its time* the server sends it as you, with your rights then: if your account was closed, you no longer have the
    app, you have left the room, it was archived, or you blocked the conversation since, it is not sent, the list says why
    and you get a notice. A sign-in lock (too many wrong passwords) or a password you must change does not stop it: a lock
    only makes it wait until the lock's time is over. At most 20 of your messages go in one minute; the rest go in the next
    minutes. In a phone room (SMS, RCS) or a bridged room it goes out through the network as any message does at that
    moment, with its checks then (an opt-out, the day's limits). If the other person blocked you, it is sent as any message
    would be; nothing tells you. Once a message is sent or cancelled the server keeps no copy of its text; one that was not
    sent keeps it for 7 days, so you can see and resend it.
  - *In an encrypted room* the server cannot encrypt for you, so the device you scheduled it on keeps it, sealed in that
    browser under its encryption keys, and sends it at its time. The limits:
    - That device sends it only while its Chat screen is open (a browser tab with Chat, the desktop app's Chat window, or
      the phone app's Chat while Android lets it run). If Chat is not open at the time, it sends the message when Chat
      next opens there, and the list says "Sent late".
    - If that device's Chat does not open within 7 days of the time, the message is not sent ("the device that keeps it
      has not been open since it was due"). A device not seen for 7 days shows as "can't be sent from" it before then.
    - A device removed from My devices, or removed by the daily cleanup of devices Chat made and nobody used for 30 days,
      takes its messages with it: they are not sent, and the list says the draft was on a device that is gone. A browser
      whose data was cleared is a new device: its old drafts are gone, and they end the same way.
    - Only text: a message with files is sent now.
    - It can be edited and sent now only on that device; your other devices list it ("Held on Firefox on Linux") and can
      cancel it. A message that could not be sent is not sent again from the list.
    - If someone in the room has a new device or identity at the time, it waits for you: the list shows "Review and send",
      which asks you as the composer would. A message the device fails to encrypt or send five times is not sent, and you
      get a notice.
  - *Remind me* on any message (its menu): in 20 minutes, in an hour, tomorrow at 9:00, or a time you pick. At that time
    you get a notification on each of your devices (held during do not disturb, and sent when it ends) and an Activity
    row. The reminders that wait are at the top of Activity, each with a × to cancel; the message's menu changes its time
    or cancels it, and an alarm by its time shows that it has one. A reminder keeps only the room and the message, never
    its text: a plain room's notification shows the message's start (unless message previews are off), an encrypted
    room's does not. A reminder of a message deleted since, or of a room you can no longer read, does not come, nor one
    of an account closed or without the app; a sign-in lock only makes it wait.
- **Events** (plans/Comms-Messaging.md §18.3 phase 19; `org.moqui.chat.ChatCalServices`). The composer's More menu (+)
  has Event: a title, all day or a start and end, the time zone (yours first), a place and details. The event is a card
  in the room: its date, the time in your zone (and the event's own when yours differs), the place (a tap searches
  OpenStreetMap for it), Going, Maybe and No (a tap answers, the same tap takes it back), and how many answered; a tap on
  the counts shows who. Once you answer Going or Maybe you can pick a reminder (15 minutes, an hour or a day before; an
  hour by default for Going), which comes as a message reminder does, and Add to calendar:
  - with an app that keeps calendars (IronStone: the event on your default calendar in Commons, Calendar, changed and
    cancelled with the chat event, taken off when you answer No);
  - in the Android app, the phone's calendar app opened on a new event filled in;
  - else the event's `.ics` file, which the card's menu also downloads. Imported again after a change, it updates the
    same entry.

  Its creator and the room's moderators change it (at most 20 times) or cancel it (for good); each change or
  cancellation is a message in the room, a reply to the event, so everyone sees it, and the reminders move or go with
  it. Everyone else (the terminal, XMPP, SMS, bridges, notifications, search) gets the event as text: "Event: Picnic",
  its time in its zone, "Where: …" and the details. An encrypted room has no events, as it has no polls: the server keeps
  the answers and writes the text.
- **Invite links and join requests** (plans/Comms-Messaging.md §18.3 phase 17; `org.moqui.chat.ChatInviteServices`).
  - *Invite links.* The room menu's Invite links, in a private room for its moderators and owners, in a public room for
    any member (there the link is a shortcut: anyone with an account may join a public room from Browse). A link is
    `https://<server>/join/<token>`, with a label, an end (an hour, a day, a week by default, 30 days, or none) and a number
    of people it takes (1 to 100, or no limit); in a private room it may ask a moderator to approve each person. The dialog
    lists the links in force with Copy, QR code (large, for a phone's camera; Download as SVG), Share (a phone's share
    sheet) and Revoke, which stops a link at once. A link also stops when its maker is no longer a moderator (or member),
    when the room is archived, and at most 20 are in force a room. Direct, record and phone rooms take none.
  - *The join page.* A link opens the room's name and member count. Signed out, it asks you to sign in and comes back
    after; there are no guests: you need an account on the server. Signed in, Join puts you in the room ("joined by an
    invite link"), or Ask to join with a note sends a request. Someone a member took out of the room comes back only by a
    request. In an encrypted room you read what is sent after you join, on a device set up for encrypted chat. On Android
    the page offers Open in the MR Moqui app, which opens the same page in the app's account on that server.
  - *Join requests.* A moderator hears of a new request by the notification bell, and the room shows "2 people ask to
    join · Review" above the composer; the dialog lists each person with their note and where they came from, with Approve,
    Deny and Approve all. The person is told either way. A request waits 30 days; after a deny the person may ask again
    in 7 days. Limits: 10 waiting requests a person, 100 a room, 30 joins and requests an hour.
  - *List in Browse.* A private room's owner lists it in Browse (the Invite links dialog): its name, topic and member count
    show to people who are not in it, under "Private rooms you can ask to join", and Ask to join sends a request. Its
    messages stay its members'.
  - *Who a link takes.* A link takes only people its maker could add to the room: someone the maker blocked, or an app keeps
    from the maker (its chat-target provider, as for Add people), gets the page of a link that does not exist, before and
    after signing in; nothing says why. A moderator who blocked you does not see your request, and their count leaves it
    out; another moderator does. An approval is checked as the approving moderator's add. An outside user of an app
    (`EXTERNAL_USER`) makes no links and gets "not found" for a public room's link. A disabled maker's links take no one.
- **Text members** (plans/Comms-Messaging.md §18.3 phase 18; `org.moqui.call.SmsRoomServices`;
  [SMS](../interfaces/SMS.md#text-members-of-a-room)). In a private room that is not encrypted and that not every user may read, a moderator's Add by phone
  number (People) adds someone without the app: they get each message as a text, "Ann: …", from one of our numbers
  marked for room texts, and their replies post to the room as them. The dialog asks for the number, their name in this
  room (only this room shows it; elsewhere the number, or its name in your phone book, shows), and your confirmation that
  they agreed to get the room's texts; it says what it costs ("With 3 text members, one message is 3 texts"). The room's
  texts to them go on you: your permission for the number's destination, your grant of our number and your day's limit;
  if you lose them, their texts stop and the room's moderators are told. A person
  is a text member of one room on each of our numbers: for a second room the dialog picks another number, and when none
  is free it says "This person is already a text member of <room> on this number; another number is needed." While they
  are a member, their texts to that number go to the room, and your own texts to them from that number are not sent:
  write in the room. They get a welcome text that says how to leave; STOP takes them out, HELP gets a short answer.
  People shows them with a phone icon and their state (replied STOP, the welcome or the last text not sent); Rename and
  Take out are a moderator's. Edits, reactions, deletes and polls stay in the room (the delete confirm says text members
  already got the message), photos go as MMS, and other files as a line. A room with text members cannot get
  disappearing messages, and an encrypted room cannot have text members.
- **Stickers** (plans/Comms-Messaging.md §18.3 phase 20; `org.moqui.chat.ChatStickerServices`). The sticker button
  beside the emoji button opens the picker: Recent, then each pack (yours first), and a search by emoji or words; a
  tap sends the sticker, which shows large with no bubble. Your own packs are in Settings, Stickers (a pack, its
  pictures with an emoji and a description each, Hide and Delete); the server's packs, offered to everyone, are in
  System, Chat Stickers for a holder of `ChatStickerAdmin` (ADMIN has it). A sticker is a PNG, WebP or GIF picture,
  animated ones too, of at most 512 KB (`chat/stickers@max-bytes`) and 512 px a side: its type is read from its bytes,
  whatever its name says, and its sides from its header before it is decoded (a file claiming huge sides is refused
  unread); at most 60 stickers a person an hour (`chat/stickers@uploads-per-hour`). A sent sticker is a copy in
  its room, so deleting a pack removes no sticker already sent, and a pack of yours is seen by no one else; in an
  encrypted room your device seals the picture as it seals any file (its size may still tell the server which known
  sticker it is). Texts and other networks get the picture (MMS), or "Sticker 👍" where no picture can go. GIF search
  waits for a provider.
- **Locations** (phase 20; `org.moqui.chat.ChatLocationServices`). The More menu's (+) Share a location, and Location
  in a phone's attach menu: Use my current location (the browser's or the phone's location permission, asked at first
  use; the position is read only then), or enter it: coordinates, degrees and minutes, a `geo:` link, a pasted
  OpenStreetMap, Google Maps or Apple Maps link (its coordinates read from the link, nothing fetched), or an address.
  There is no map and no geocoding. The message is a card with the label, the address and the coordinates, Open in maps
  (the phone's maps app in the Android app, OpenStreetMap elsewhere) and Copy. Everyone else gets "Location: …" and an
  OpenStreetMap link; Sinch RCS its location message. A location someone shares with you over RCS shows the same way.
  In an encrypted room the place is only in the ciphertext. Live location is below.
- <a id="live-location"></a>**Live location** ([plans/Location-Sharing.md](../../plans/Location-Sharing.md) §3.5;
  `ChatLocationServices.send#LiveLocation` and `get#LiveLocation` over `org.moqui.maps.LocationServices`, [Maps, Live
  location](Maps.md#live-location)). Share a location has **Share live location** beside Send, with its end (15 minutes,
  an hour or 8 hours, within `location@max-share-hours`) and Approximate (about 1 km). It starts a location share whose
  audience is the room's members as they are at that moment, as people: its local users but you, its agents and members
  who put the room out of sight, less those the chat directory keeps from you or who blocked you (each told who shares
  with them and until when, as any share's audience is). Then it posts a message whose `content.location` is
  `{liveShareId, label, thruDate, approximate, source: "live"}`: the share's id, never a position. Its words are "Live
  location: Ann, until 15:30" and "Live updates are in the app" with the room's link, so the room list, the push, the
  terminal and every bridge (SMS, XMPP, Matrix and the others: a place without coordinates goes to no adapter's
  location message) get those words and no position. In the Android app the app's own dialog confirms the share and its
  location service sends it, also in the background; in a browser the page sends it while it is open, with a banner
  above the room ("Sharing your live location until 15:30 · sent from this page while it is open", Stop), and stops when
  no chat view stays open; a share no page sends goes stale and its age says so. The card shows the position with its
  accuracy, "Updated 2 min ago" and the end while the share is on, moved by the share's `locationShare:<shareId>` pushes
  (which the server sends to its audience only), with Open in maps and Copy, and "Live location ended" after. It reads
  the share through `get#Share`, which checks the audience: a member who joined the room after it started, or whom its
  owner took out, sees "Not shared with you" and no position, and a position received before a person was reached never
  reaches them. **Membership changes do not change the share**: a person who leaves the room stays in its audience until
  you take them out in Who sees my location (Maps, or My location shares), and a new member is never added; to share
  with them, share again. The owner's card says so ("Shared with the members of the room when you started it"), where it
  is sent from, and has Send from here (Send from this device in the app), Stop and Who sees my location.
  **Not in an encrypted room** (the decision of LL4): the picker says "Live location is not offered in an encrypted room:
  the server would see where you are", and `send#LiveLocation` refuses it (`encryptedRoom`). Sealing each report to the
  room's devices instead (Megolm, as a message is sealed) would keep the server out, but the Android app's location
  service sends from Rust with no page, and the room's keys live only in a page's crypto store (matrix-sdk-crypto in
  IndexedDB), so the phone would send nothing in the background; the server could not apply the audience rule (who was
  reached when), the rate, the approximate rounding or the end, each of which would move to every reader's device; and
  a member who joined later would hold the room key. Sealed live location is a later step: the room's devices sealing
  per report (or a session per share shared only with its audience's devices), with the phone's service given a crypto
  store of its own.
- **Photos: compressed or original** (phase 21). A photo you attach (JPEG, PNG or WebP) is now drawn again in your
  browser before it goes: a JPEG or WebP becomes a JPEG of at most 2048 px on its long side, also when that is no
  smaller, and a PNG stays a PNG (made smaller only past 2048 px, so a screenshot keeps its sharp text); either way its
  metadata (EXIF, a PNG's text chunks, the GPS location among them) is gone. **This changes how Chat worked before, when
  every photo went at full size.** The file's chip in the composer has Original, which sends that photo as it is,
  details and location included. A GIF, a video, and a photo the browser cannot read (HEIC on most) go as they are. In a
  room that goes out by text, the message's files share the text's total limit (Telnyx 1 MiB, Twilio 5 MB, Sinch 1 MiB,
  or the bridge's `maxMediaBytes`): each photo is fitted under its share, Original is offered only when the original
  fits, and a message whose files are still over the limit is not sent and says to send fewer at a time.
- **One-time codes** (phase 21; `org.moqui.chat.ChatPrivacyServices`). A code in a text that came in (SMS, MMS, RCS)
  shows a "Code 482913" chip with Copy. Its notification has Copy too: on the web where the browser shows notification
  actions (it opens the message and copies the code there, or lights Copy for one tap where the browser asks for one),
  and in the Android app (its next APK) a Copy code button that puts it on the clipboard marked sensitive, with the lock
  screen saying only "A code from …". The code goes in a notification only where the message's words do: message
  previews on, a plain room, and a message that does not disappear. Settings, Chat, Privacy can delete a text's code a
  day after it came, when everyone in its room asks the same, and only for a code that is the text's only one and within
  about 20 characters of "code", "OTP", "verification", "passcode" or "security code" ("confirm", "login" and "PIN"
  find a code for Copy but never delete it); the chip then says when, with Keep. Codes in chat between people are never
  looked for.
- **Screen security** (phase 21). Settings, Chat, Privacy: Off, Block screenshots in encrypted rooms, or everywhere in the
  app (applied at once, on and off); an encrypted room's menu also has Block screenshots here, your own choice for that
  room. With several accounts on one phone, any account set to everywhere keeps it on, and turning it off in that
  account takes away only its own. It is Android's
  `FLAG_SECURE`: no screenshot, no screen recording and a blank recent-apps thumbnail while it holds. Only the Android app
  can do this; the web and the desktop app cannot stop a screenshot, and no app can stop a camera.
- **Call links.** A link to a meeting for people with no account (Calls, Call links): they open it in a browser and give
  their name, then wait in the lobby until someone in the call lets them in (or come straight in, as the link says).
  The link's address is shown once, when it is made; Open starts its meeting with you as host, and Revoke takes its
  guests out. Calls also lists your meetings of the next week from an app that has them (IronStone: your calendar),
  with Join. Settings, Calls has the microphone, camera and speaker, the ringtone, and whether calls start muted
  or with the camera off, for this browser, and Always relay my calls, for all your devices: a call with one other
  person here goes straight between your devices when it can, and relayed it goes through the server, so they never
  see your network address ([WebRTC, The direct path](../interfaces/WebRTC.md#the-direct-path)). The call's header shows
  the connection's quality, "Reconnecting…" while it comes back, and "Your network is weak" when your side is the cause.
  Calls need a media server (`media-server@port`); the Calls screen says when there is none. In the terminal a call is not live: its screen shows who is in it (and who is on hold), with Answer, Decline and Leave.
- <a id="encrypted-calls"></a>**End-to-end encrypted calls.** New call and a call link have End-to-end encrypted, and a
  room's owner or moderator can make all of the room's calls so (Room settings, Calls in this room are end-to-end
  encrypted). In such a call only the people in it hear and see each other: each browser encrypts what it sends with a
  key of its own, which goes only to the others' browsers, and a new key is made whenever someone joins, leaves or comes
  back, so someone who left does not hear what is said after. The call shows End-to-end encrypted beside its state (other
  calls show Encrypted to the server, which can record or caption them), and Participants shows each person's key state
  (keys received, waiting for keys) and that their device is not verified yet. This is apart from the encryption of the
  room's messages. Such a call cannot be recorded or captioned, takes no phone number, desk phone, forwarded number or
  guest, and is not merged with another call; once encrypted it stays so. It needs a current Chrome, Edge, Firefox or
  Safari: a browser that cannot encrypt call media says so and does not join, and the terminal does not answer one. The
  keys use this browser's encryption device (Settings, My devices), which one tab holds at a time: answer the call in the
  tab that is not using it for something else, or close the other one. See [WebRTC, Encrypted
  calls](../interfaces/WebRTC.md#encrypted-calls).
- **Recording.** When the server allows it (`calls/recording@enabled`), the host of a call has a Record button: start,
  pause, resume and stop. Everyone in the call sees that it is recorded, before they join too, and can withdraw their
  consent, which pauses the recording. Where the policy asks for it, each person acknowledges the notice, and the
  recording waits until all have. A phone in the call hears a tone instead, and presses 1 to acknowledge or hangs up to
  withdraw; it hears another tone when captions turn on. Calls, Recordings lists the recordings you made and those of calls you host: play one
  part with everyone's audio and video together, download each file, and delete your own. The terminal lists them with
  Delete. Recordings are deleted after the server's retention period ([Call recording](../interfaces/Call-Recording.md)).
- **Captions.** Where the administrator turned them on (`calls/captions@enabled`) and a speech to text model is set up,
  anyone in a call can press CC. Everyone in the call, including people who join later, sees that captions are on and
  which service hears what they say; each line names its speaker. When the service is slow or down, the captions say
  they are unavailable and the call goes on. Captions are not kept unless the administrator asks for that
  (`calls/captions@store`). An end-to-end encrypted call has no captions. In the terminal a call shows whether captions
  are on, who turned them on and which service hears them, with Turn on and Turn off; the lines show in the browser. See
  [WebRTC, Captions](../interfaces/WebRTC.md#captions).
- <a id="phone"></a>**Phone calls.** Calls, Dialer (also the tray's Dialer tab): type a number or
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
- <a id="in-call"></a>**In a call** (V5). Hold puts the phone on hold (it hears hold music, and nobody hears it). More
  has Transfer (to a person, a number or a queue; Now hands the caller over and you leave, Talk to them first holds the
  caller while you talk to the person, then Complete transfer or Cancel transfer), Add a participant (a person, or a
  number), Merge another call (another call you are in joins this one as a meeting; both must be on one media server unless
  cascading media servers are on, `media-server@cascade-enabled`)
  and Make it a meeting. A number you transfer to or add is a call you place, with your permissions and limits. A
  transfer needs the call answered, and if the call changed since you looked, try again. When a phone call rings you,
  the ring bar and the call show who is calling when an app knows the number (in IronStone, the contact and their open
  requests). See [SIP, Transfer and merge](../interfaces/SIP.md#transfer-and-merge).
- <a id="queues"></a>**Queues** (Calls, Queues; V5). The call queues you take calls for: log in, pause, resume, log out;
  each with its calls waiting, the longest wait and the agents free. Logged in and free, a queue's calls ring you as
  any call does; after one you have the queue's wrap-up time before the next. See [SIP, Queues](../interfaces/SIP.md#queues).
- <a id="voicemail"></a>**Voicemail.** A call to your number goes to your voicemail when you don't answer within your
  rings, when you are in another call, or when you decline it. The caller hears your greeting (or the default one), a
  beep, and leaves a message of up to two minutes; `#` ends it. You are told at once (here, and on your devices by Web
  Push), and by email when you ask for it. Calls, History, Voicemail lists them, unheard first in bold, with the Inbox
  and the Trash; open one to play it, read its transcript (when the server transcribes voicemail), mark it heard or
  unheard, call back, download or delete it. Deleted messages wait in the Trash for 30 days (`voicemail@trash-days`),
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
  ends the phone. Phones register only when the administrator has turned the registrar on (`sip-server/registrar@enabled`);
  until then the page says so. A phone that has no SHA-256 sign-in takes the legacy profile (MD5). See
  [SIP, Registrar](../interfaces/SIP.md#registrar).
- <a id="my-devices"></a>**My devices.** Settings, My devices lists the browsers that hold your encryption keys, for
  encrypted calls. A browser gets its device the first time it needs one; this browser is marked. Each shows when it was
  added and last seen, its browser and its key's fingerprint. Rename names one; Remove revokes it: it gets no new keys,
  and others stop encrypting to it (removing this browser's own also deletes its keys here, and it gets a new device next
  time). Removing a device does not sign that browser out. See [Chat keys](../interfaces/Chat-Keys.md).
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
  hours and holidays (once or each year) with their time zone, and whether each is open now. IVR menus (V5): each menu's
  prompt (an uploaded recording; without one, tones), how long it waits and how often it plays again, how many keys,
  its options and where they go (a person, a ring group, a queue, another menu, a voicemail box, an outside number,
  the menu again, the end), and where no key, a wrong key and its closed hours go. Queues (V5): each queue's calls
  waiting now, its agents and their state, its strategy (the agent idle longest, round robin, all at once), the
  limits and where calls go past them, wrap-up, how often callers hear their place, and its hold music and position
  prompt (uploaded; without them, tones: Moqui ships no music or recorded speech). Numbers route to a menu or a queue too.
  The screens are basic-comms' (`screen/System/`), mounted
  into the tools System app; in the terminal they are the same forms.
- **Chat bridges** (System, Chat Bridges; the ChatBridgeAdmin permission, which ADMIN has). Each bridge with its
  network, its status (connected, not connected, disabled) and last error, its rooms and its deliveries waiting, failed
  and uncertain; the deliveries that did not go, with Send again; Disable and Enable. A network's bridge is set up with
  its adapter ([Chat bridges](../interfaces/Chat-Bridges.md#administration)).
- **Home.** Unread by account, flagged, snoozed, mentions, mail that was not sent, missed calls, unheard voicemail (once
  you have a voicemail box), Needs reply (mail a day old or more that is not answered and has no reply sent after it;
  `find#InboxRows` `needsReply`), and rooms with unread.
- **Push health** (System, Push Health; the PushHealthAdmin permission, which ADMIN has). Every user's push devices (a
  browser, the app through Firebase or a UnifiedPush distributor) with the last registration, push and acknowledgement,
  the pushes sent, acknowledged, failed and waiting, and first, marked, the devices that acknowledge pushes whose pushes
  of the last day were not acknowledged (a device that never acknowledged one is not marked).
- **Notifications on this device.** The account menu turns on desktop notifications and Web Push for the browser or
  the installed app ([Qvt-Client](../ui/Qvt-Client.md#install-and-web-push)). A chat message is pushed to every device
  of each member it is for, by the room's level (a muted room as Mentions), not once it is read on another device (a
  read there removes the notification), held on do not disturb and sent as one summary per room when it ends, and
  encrypted to the device: the push service sees no text. A missed call and a voicemail push as messages in their
  room. A reaction to your message shows in the room, never as a push. On do not disturb a call still rings, your
  apps and your desk and soft phones, from the people in Settings, Chat, Calls that ring through, and as the same
  person's or number's second call within 3 minutes ([Qvt-Client, Chat push](../ui/Qvt-Client.md#install-and-web-push)).

### Whiteboards

Open **Comms → Boards** to create a drawing in your private notes space or reopen one of your boards. A board
is an Excalidraw wiki page. People with update access can draw together and see one another’s pointers.
**Save version** records the scene with a note. Open **History** to inspect a previous version or **Download**
to export the drawing as Excalidraw JSON, SVG or PNG.

**Insert Mermaid** turns a flowchart into shapes inside a frame. Select its frame and choose **Edit as Mermaid**
to revise its source. Replacing the diagram replaces hand edits to shapes inside that frame. The wireframe
and flowchart shapes in **Library** and the editor’s fonts load from the server.

In a room, open **Room → Boards** to list or create boards. Room boards follow current membership. Archiving
the room keeps its drawings available to read. In a call, **Whiteboard** opens a board on the stage. Viewers
can follow the presenter’s viewport or pan independently and choose **Follow again**. Hosts can change who draws.
Admitted guests draw by default and lose access when removed or when the board closes. The recording’s details
link to boards shown during the meeting. For a recurring meeting, choose a board on the call link. The linked board opens when its owner opens the meeting and the stage is free. An earlier guest or view-only member waits for the owner.

In the Android app, **Share the screen** shares the phone's whole screen after Android asks for your consent; a
notification shows while it is shared and has **Stop sharing**. In an end-to-end encrypted call the phone's screen is
encrypted as the call is. Others cannot draw over a phone's share.

**Who can present** offers **Anyone can present** (the default), **Hosts can present**, and **Ask a host to present**. Only a host can take over from a host. **Present this screen** lets signed-in members follow the presenter through the application, using their own accounts. Guests can watch when the presenter shares the tab with them. Following pauses when a member has changed a form, opened a dialog of their own, or navigated elsewhere. **Follow again** catches up. Reloading a page that was sharing a screen releases its stage. Choose the screen again to resume.

While sharing, **Annotation permissions** controls who may draw; the presenter always may. Unpinned marks fade after five seconds. Use the laser for
pointing or pin a stroke to keep it visible. **Snapshot to board** creates a board with the captured frame locked and
each stroke available to edit. Only someone who may draw on the share can ask for one, and the presenter is asked first
when someone else does, since the board then takes the share's place on the stage. The one taken over is told who took
the stage. In chat, open an image and choose **Mark up** to save a new PNG as a reply to the original.

The readers of a room open to all users, or of a record's room, see its boards as its members do; only members draw on
them. In IronStone, Knowledge and Notes show a board's preview with **Open the board**, and Knowledge's **New page** can
make a whiteboard. The desktop app shares screens and windows on every OS ([Desktop-Client, Screen
share](../ui/Desktop-Client.md#screen-share)).

#### How boards are built

The design record is [Comms-Whiteboard](../../plans/archive/Comms-Whiteboard.md); what stays true of it:

- **Libraries.** Boards are [Excalidraw](https://github.com/excalidraw/excalidraw) (MIT), bundled with React and loaded
  only on a board (`webroot/build/whiteboard`, `libs/excalidraw`); its Mermaid import is mermaid-to-excalidraw, on the
  shell's Mermaid. Annotation over a share or an image is [drauu](https://github.com/antfu/drauu) (MIT). tldraw was
  refused (a production license key, a watermark without one) and draw.io (an iframe app, not a framework). Excalidraw's
  embeddables, AI, public library site, own collaboration UI and anything fetched from outside the server are off; an
  element that runs a page (`iframe`, `embeddable`, a magic frame, generated HTML) is refused by the server and the page.
- **A board is a wiki page** of type `excalidraw` (`<Page>.excalidraw`, Excalidraw's scene JSON with images named by
  attachment), co-edited through `moqui-collab`'s board codec
  ([Revision-Management, Whiteboard revisions](../data/Revision-Management.md#whiteboard-revisions)). Only the background
  and grid settings are shared between editors.
- **A room's space.** `ChatRoom.wikiSpaceId`, made on first use (`org.moqui.chat.RoomBoardServices.ensure#ChatRoomSpace`):
  `get#WikiPageAccess` answers for it by the room (members edit, readers view, an archived room is read only), with no
  `WikiSpaceUser` rows, so membership has one source. Leaving the room or archiving it asks its open editors again at
  the next co-editing tick (`collab-recheck`). A call with no room uses the starter's notes space.
- **A call's boards.** `moqui.call.CallBoard` {callSessionId, wikiPageId, opened and closed} is what a call has shown,
  for its history and its guests' grant; `CallLink.wikiPageId` is a recurring meeting's board.
- **The stage.** `CallSession.shareModeEnumId` (`CsmAnyone`, `CsmHosts`, `CsmAsk`; a call link's own copied when a
  meeting opens) and one stage per call: a screen, a board or a followed screen. The frames are in
  [WebRTC, Presenting in a call](../interfaces/WebRTC.md#presenting-in-a-call).

## For app authors

An app adds to BasicComms through four seams. None needs a copy of a BasicComms screen.

**Mount the screens.** List them under the app's root:

```xml
<subscreens-item name="Inbox" location="component://basic-comms/screen/BasicComms/Inbox.xml" menu-index="2"/>
<subscreens-item name="Chat" location="component://basic-comms/screen/BasicComms/Chat.xml" menu-index="3"/>
```

To keep one Comms in the launcher, the app's `MoquiConf.xml` hides BasicComms
(`<subscreens-item name="BasicComms" menu-include="false"/>` under `apps.xml`; `/qapps/BasicComms` still works) and sets
`comms@app-root` to its own root, for the links in notifications and mail. A screen's own links are relative to the
mount (the `appRoot` its root sets, `mScreen.path`).

**screen-extend.** A file at `<app>/screen-extend/basic-comms/screen/BasicComms/<path>.xml` adds transitions,
subscreens, actions and widgets to that screen, under every mount. Named places to add to: `SettingsCards` (the
settings index's cards), `HomeRooms` and `HomeShortcuts` (Home's columns). A transition of the same name replaces
the screen's: IronStone replaces the drive picker's `pickerFolder` in `Inbox/Compose` and `Inbox/Mail` to name its
records drive's folders by their records.

**Provider hooks.** The app root's `shell-providers` names services the framework calls, as the app, through
`org.moqui.ui.ShellServices.call#Providers` (each hook's contract is a `type="interface"` service in
`org.moqui.comms.ProviderInterfaces`):

| Hook | BasicComms asks it for |
| --- | --- |
| `inbox-source` | Every Inbox, Compose, MessageDetail and Home operation (`run#Operation`): its answer is the whole page, so an app's own messages and mailboxes are one list with the mail. Without one, the framework answers over the user's mail. A provider that does not answer `get#MailChanges` makes `mailChanges` answer `unsupported`; `set#ThreadState` and `apply#ViewAction` with a `clientRef` run once whoever answers them |
| `record-lookup` | A record room's title and link, a message's record cards, the records to file a message on |
| `record-link` | Filing a chat message on a record |
| `create-from` | Making a record (a task) from a chat message, or from a mail or internal message through Mail's `createRecord` (`kind`, `emailMessageId`, `communicationEventId`) |
| `invitation` | Answering the calendar invitation a message carries (`emailMessageId`, `response`; answers `workEffortId`, `statusId`, `replySent`, `eventUrl`). The framework reads the card itself (`InboxServices.get#InvitationCard`); without a provider it shows no answers (IronStone: the event on the user's calendar, the organizer told) |
| `contact-card` | Who a sender or recipient is, for the reading pane's contact card (`address`, `partyId`, `name`; answers `card`); without one the card is the address alone (IronStone: the party with the address and its threads; the example gallery has one for its addresses) |
| `home-source` | Boxes of its own on Home (answers `cardList`, each with its rows and a link to more), after Home's parts whoever answered them (`CommsHomeServices.get#HomeSources`; IronStone: its mailboxes with unread; the example gallery has two, one failing on purpose) |
| `external-conversation` | Filing the conversation of a new outside party on a record: the first text of a new number's pair room (`chatRoomId`, `chatBridgeId`, `chatActorId`, `eventSeq`, `idempotencyKey`; answers `filed`, `recordUrl`), as the room's first local member; tried again on the next text until one filed it (`ChatRoomBridge.filedDate`; IronStone: on the one party with that number) |
| `meetings-source` | The user's meetings in a range, for the Calls screen's Meetings (IronStone: the calendar's) |
| `recipient-source` | People with phone numbers for the Dialer's search (`kinds` phone; IronStone: the parties and contacts the user may see) |
| `sms-consent` | Whether a promotional text may go to a number (`fromNumber`, `toNumber`; answers `consent`); one no refuses it, and without a yes it is refused ([SMS](../interfaces/SMS.md#texts-that-go-out)). Also answerable from a component's conf (below): Mantle registers its consent records' answer there, so a campaign's text, sent with no user, is asked too |
| `call-context` | Who a phone caller is, for the ring bar and the call (V5; `number`; IronStone: the party with that number the user may see, and its open requests; the example gallery has two, one failing on purpose) |
| `call-target` | Which users the current user may not call, transfer a call to or add to one (`userIdList`; answers `refusedUserIdList`): a refused user is answered as one that does not exist, a desk phone's REFER to one gets 403; with no provider any enabled user may be reached, and one that fails refuses (IronStone: the users of the parties the user may see, under the organization filter of `recipient-source`; a user with no party is in no organization and may be reached; the example gallery refuses users named `*.offlimits`) |
| `chat-calendar` | A chat event's entry on the current user's calendar (`ProviderInterfaces.chat#CalendarEntry`: `chatRoomId`, `eventSeq`, `action` answer, change or cancel; answers `eventUrl`). Called by `ChatCalServices` after its writes are committed, each provider in a transaction of its own; one that fails is logged and the answer stands. It is given ids only and reads `ChatCalEvent` and the user's `ChatCalAnswer` itself. Without a provider Add to calendar is the `.ics` file (IronStone: one Mantle event per chat event, its `universalId` the chat event's UID, changed only by one who can read the chat room; it is shared, so an edit of it in Commons, Calendar is overwritten by the next change of the chat event). A change or cancel a provider failed is kept (`ChatCalEvent.calendarSyncPending`) and tried again at the event's next answer or change, after a wait that doubles from a minute to a day, ten times at most |
| `chat-target` | Which users the current user may not find in the people search (`CommsServices.search#Users`), start a direct room with, add to a room or mention in a public room they are not in (`userIdList`; answers `refusedUserIdList`, the interface `chat#Target`, whose parameters are `call#Target`'s): a refused user is left out of the search and answered as one that does not exist; with no provider any enabled user may be reached, and one that fails refuses (`ChatServices.get#RefusedUsers`). The search leaves out every external user (`EXTERNAL_USER`), and an external user reaches only the people of their own rooms. The example gallery refuses users named `*.offlimits`; one service may answer both hooks |
| `shell-indicator` | Not a Comms hook: the status chips of the shell's app bar for the current user (called with no parameters; answers `indicators` and `refreshOn`), asked by the shell's `/shellIndicators` ([Qvt-Client, Shell indicators](../ui/Qvt-Client.md)). Maps answers "Sharing location" while the user shares theirs |

A provider runs with the app's authorization and entity filters; a read that fails is left out and named, and a mutation that fails is an error.

A hook the framework asks for whoever acts, the system with no user included, may also be answered by a component
with no app root, in its `MoquiConf.xml` (only `sms-consent` today; another element there is logged and left out):

```xml
<screen-facade><shell-providers>
    <sms-consent name="mantle.consent" service="mantle.party.MessageSmsServices.check#SmsConsent"/>
</shell-providers></screen-facade>
```

`call#Providers` asks such a provider after the apps' ones, for every caller, with authorization disabled (the conf
that names it is trusted, so its service reads only what the hook asks), even when an app's provider has the same
name, and `get#Providers` lists it with no app. For `sms-consent` a provider that fails refuses the text, as a no does.

**The contacts hook of chat's people.** Which of a user's rooms are one person is asked of one service (not a provider:
the framework keeps no contacts), named by the `chat@contact-service` property, else the theme value
`STRT_CHAT_CONTACT_SERVICE` of the user's theme, as the mail contact hook (`STRT_EMAIL_CONTACT_SERVICE`) is.
`ChatPeopleServices.find#ContactPeople` (Rust, `crates/moqui-core/src/service/contacts.rs`) calls it with authorization
disabled, in its own transaction, with `userId` and `addressList` (`tel:+E.164`, `email:address`, `user:userId`; only
the addresses of the user's own rooms, at most 300, at most 60 times an hour a user, its answer kept three minutes for
the user's tabs, devices and mail lines), and it answers `contactList`, the contacts the user can see with one of them,
each `{contactRef, name, url (a path on this server), addressList}` with its own addresses. The hook
enforces the app's visibility itself. Without one, a user's people come from their rooms and their own changes. Mantle
answers it (`mantle.party.ContactBookServices.find#ContactPeople`, the rules of `filter#ContactAddresses`) and IronStone
names it. A person who is a user of this server shows none of the hook's numbers or addresses.

**Declared actions and panels.** `<ext-action>` and `<ext-panel>` in a screen-extend add items to a slot of a screen
([Qvt-Client](../ui/Qvt-Client.md#declared-actions-and-panels)). Chat's slot is `chat-message-actions`, whose item is
`{chatRoomId, eventSeq}`; IronStone adds Create a task, request and event there, each with `hook="create-from"` so a
user without that provider is not offered it. Calls, Voicemail's slot is `voicemail-actions`, whose item is
`{voicemailId, fromNumber, callSessionId}`, for actions such as Create from and Add to contacts. Inbox, Mail's slot is
`message-actions`, in the More menus of a message and of its thread (for the thread's last message), whose item is
`{store, emailMessageId, communicationEventId}` (`store` `email` with the mail message's id, or `internal` with the internal
message's communication event); an action there with `hook="create-from"` posts Mail's `createRecord` transition, which
hands the message's ids and the action's `kind` to the `create-from` providers as Chat's `createFrom` does (IronStone's
Sales adds Create lead there).

IronStone BOSS is the worked example: `runtime/components/addon/mr-boss/screen/Comms.xml` (the mounts and providers) and
`screen-extend/basic-comms/`.

## Tests

`./moq test basic-comms`: the user search, the System Voice screens and no call into Mantle or an app component
(`BasicComms`), a bridged room and System, Chat Bridges (`BasicCommsBridges`), the mail settings and Inbox of a
user with no party, Compose retried by `clientRef` (`BasicCommsComposeRef`), the change cursor, requests once by `clientRef`, conflicts and chunked uploads (`BasicCommsSync`), the drive picker (`BasicCommsFiles`), chat, the call screens and recordings (`BasicCommsCalls`), the
provider hooks and the app's parts through both mounts, Text a number (`BasicCommsTexts`), people across networks (`BasicCommsPeople`). `cargo test -p moqui-channels`:
texts and MMS against a fake Telnyx ([SMS, Tests](../interfaces/SMS.md#tests)). `./moq test framework --spec Voice`: caller ID, international
and premium, the limits, failed sign-ins blocked per source, emergency readiness and routing, calls coming in and the
phone's events. `./moq test framework --spec Voicemail`: busy and unanswered calls to voicemail, private audio, the PIN
and its limits, notices, transcription and its retry, and the trash. `./moq test framework --spec SipDevice`: desk and
soft phones (credentials once, rotation, revocation, ringing beside the tabs with the first answer winning, voicemail
after the tabs and phones ring or a phone declines, a phone's own call). `./moq test framework --spec CallHandling,RingGroup`:
the rules, schedules across a time zone, forward loops and permissions, do not disturb and call waiting, simultaneous ring,
the strategies, the shared box, and the first answer winning. `./moq test framework --spec ChatSchedule`: scheduled
sends and their refusals at the send time, a bridged room's delivery, an encrypted room's message held by its device,
and reminders; `cargo test -p moqui-core --lib chat_push`: a reminder's push. `./moq test framework --spec ChatKeys`: crypto devices,
their keys and to-device messages; `./moq test-client basic-comms --serve --spec CryptoFlow`: two browsers exchanging an
Olm-encrypted message ([Chat keys](../interfaces/Chat-Keys.md#tests)). `./moq test framework --spec CallEncryption`: the
policy and every refusal of end-to-end encrypted calls, an encrypted room's calls among them; `./moq test-client basic-comms --serve --spec EncryptedCallFlow`:
an encrypted call in three browsers ([WebRTC, Encrypted calls](../interfaces/WebRTC.md#encrypted-calls)). The lock and
the notes: `./moq test-client webroot --offline --spec WebrootChat` (the rules of `moqui.chat.e2eeLabel`), and the
acceptance walk `./moq accept basic-comms --spec ChatLabels` (each room's label on a phone and a desktop). Live location:
`./moq test framework --spec ChatLocation` (the room's members see it and nobody else, the message's share id and words,
a member added later and one who left, an encrypted room refused, a bridged room's words), `./moq test-client webroot
--offline --spec WebrootChat` (`moqui.chat.live`: the pushes and the page's sender) and `./moq test-client basic-comms
--offline --spec ShellSync` (the card's live update and its end). `cargo test -p moqui-xmpp` and
`cargo test -p moqui-host --test it -- xmpp::`: XMPP clients against recorded stanzas and on a real server ([XMPP,
Tests](../interfaces/XMPP.md#tests)). IronStone's own parts are in its specs (`Inbox`, `Compose`,
`Chat`, `CommsHome`, `InboxMailboxes`, `MailAccounts`, `MailSettings`).
