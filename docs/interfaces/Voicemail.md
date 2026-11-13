# Voicemail

A call to a user's number that nobody answers goes to their voicemail box: the SIP engine answers it with no one else
in it, plays the greeting and a beep, and records the caller. Its owner plays it in the browser, reads its transcript,
and can listen by phone with a PIN. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.3
(Zimbra's voicemail actions and preferences), §5.4 and §5.5, milestone V2. Phone calls themselves are in
[SIP](SIP.md); the user guide is [Comms](../tools/Comms.md#voicemail).

- **Engine:** `crates/moqui-host/src/sip/voicemail.rs` (the call's state and its prompts, `Machine`, with no I/O) and
  `audio.rs` (Ogg Opus read and written with moqui-rtc's `record/ogg.rs` writer and the M0 Opus codec;
  `convert#GreetingAudio`); the leg's thread plays and records (`leg.rs`).
- **Services:** `org.moqui.call.VoicemailServices` and `Voicemail.js` (framework), with `VoicemailBox`, `Voicemail` and
  `VoicemailPinFailure` in the V2 block of `framework/entity/VoiceEntities.xml` and the transcript a `CallTranscript`
  (`voicemailId`). `store#VoicemailFile` is Rust (moqui-core `service/call_media.rs`).
- **Screens:** BasicComms Calls, History's Voicemail tab; `Calls/Voicemail` (`Voicemail.qvue`, `VoicemailTui.xml`);
  `Settings/Phone` (`Phone.qvue`, `PhoneTui.xml`); the Voicemail card on Home.

## When a call goes to voicemail

An inbound call to a user's number (`PhoneNumber` routed `PnrUser`, from `receive#PhoneCall`) goes to their box:

- **No answer.** The engine starts a timer when the call rings: the box's `rings` (6 seconds each, 1 to 10), else
  `call_ring_seconds`. Unanswered by then, it goes. The call's ring expiry is set 30 seconds later, so `expire#Calls`
  neither ends it nor stops the user's ringing first when the box rings longer than `call_ring_seconds`.
- **Busy.** The user is in another call (`UserStatus.inCall`) with call waiting off, or in do not disturb: the call is
  not rung at all and goes at once (V3: unless their busy rule sends it elsewhere).
- **Declined.** The user declines it and nobody else is rung for it (`decline#Call`, or the reject key of one of their
  desk or soft phones, sends the engine the control `voicemail`).

The user's registered phones ([SIP, Registrar](SIP.md#registrar)) ring with their tabs for the whole time: until the
box's rings, or the ring expiry. A decline on one of them is a decline on all.

Each time the engine asks `begin#Voicemail` first, which moves the call there in one transaction (`Calls.js`
`phoneToVoicemail`): the user's ringing legs time out (the engine cancels each phone still ringing), their `CallUserLog` is missed (a decline stays declined), the
ring bars close, and the call is active with the phone alone. Someone who answered first wins: `begin#Voicemail`
refuses (`answered`) and the call goes on as answered. A user's box is made, with the defaults, the first time it is
needed.

With V3 the box is the last step of the call's handling plan ([SIP, Call handling](SIP.md#call-handling)): a no-answer
rule, a forward or a ring group's members ring first, and `begin#Voicemail` rings the plan's next step instead
(`mode: ring`) until none is left. A ring group's fallback is the group's own box (`VoicemailBox.ringGroupId`, made the
first time), which every member reads: its messages are in each member's Voicemail tab (with the group's name), count in
their unheard total and Home card, and are told to each of them; one heard by a member is heard for all. A voice admin
sets the group box's greeting and its email notice in System, Voice, Ring groups (Voicemail): the settings and
greeting services take `ringGroupId` (`get#` and `update#VoicemailSettings`, `store#`, `delete#` and
`get#VoicemailGreeting`), for `VoiceAdmin` only; anyone else gets "not found" for the same group. A group's box has no
rings of its own: the group rings by its seconds.

## Leaving a message

1. The engine answers the call (200 with the SDP it offered when it rang).
2. The greeting: the user's own, else the default (a tone pattern, then `greeting.wav` when the deployment has one).
3. A beep (1000 Hz, half a second).
4. The caller's audio, decoded by the leg (G.711 or Opus to 48 kHz), each 20 ms written as Opus in Ogg to a spool
   file under `runtime/tmp/voicemail` (a missing packet is silence, so the length is the call's).
5. It ends when the caller hangs up, presses `#`, or `voicemail_max_seconds` passes; then goodbye and the hang-up.
6. `store#Voicemail` moves the file (`store#VoicemailFile`, which takes only that spool and writes only under
   `dbresource://moqui/call/voicemail/`) and makes the `Voicemail` in the inbox. A message shorter than a second is
   dropped. Audio that does not store keeps the message without it (`VmsAudioFailed`, with why), so its owner still
   knows of the call.

Then the owner is told, each part on its own:

- a `chatUser` push (`{kind: 'voicemail', action: 'new', voicemailId, from, unheardCount}`) and Web Push "New voicemail
  from …", linking to the message (`comms_app_root` `/Calls/Voicemail?voicemailId=`);
- an email to the box's `notifyEmail`, when there is one, through the framework's mail (the `SYSTEM` email server):
  who, when, how long and the link (absolute when `webapp_http_host` is set). The audio is attached only with
  `attachAudio`, and the transcript is in it only with `attachTranscript`. Both are off by default, are only allowed
  with an address, and turning one on answers a warning: mail cannot be recalled. With the transcript in it, the email
  waits for the transcription. It is sent once.

## Transcription

Off unless `voicemail_transcribe` is `true`: the deployment's recording and transcription policy decides whether
voicemail audio goes to the speech provider. When on, after the message commits, `transcribe#Voicemail` sends the
Ogg file to `SpeechServices.transcribe#Audio` (the model bound to `transcribe`; the owner's quota; purpose
`voicemail:<id>`; `voicemail_transcribe_language`) and writes its `CallTranscript`: `CtsPending`, then `CtsDone` with
the text or `CtsFailed` with why. A failure never touches the audio. The owner sees the state and retries a failed one
(`retry#VoicemailTranscript`, Retry on the screen).

## Phone access

With a PIN set (Settings, Phone), the owner listens by phone:

- **Their own number.** Press `*` during the greeting.
- **The access number.** A `PhoneNumber` routed `PnrVoicemail` (System, Voice, Numbers): enter the box's number (the
  user's number, as many of its last digits as make it one number) and `#`.

Then the PIN and `#`. The new messages play oldest first, each after a short tone; then the menu tone. Keys, during a
message or after it:

| Key | |
| --- | --- |
| `7` | Delete: to the trash |
| `9` | Save: marked heard |
| `#` | Next, leaving it new |

After the last one, a tone and goodbye. With no key, the menu plays three times and the call ends. `*` while entering
a number or a PIN starts it again. With no key for 10 seconds after the number or PIN prompt, the prompt plays again;
the third time the call ends, so a silent caller does not hold a call.

**The PIN** is `voicemail_pin_min_digits` (6 by default) to 12 digits, not one digit repeated or a run like 123456,
kept as an argon2id hash (`pinHash`, the parameters of `hashPassword`). A PIN of 4 or more digits set before the
minimum was raised still works. Over the phone, `check#VoicemailPin` answers accepted or not, never an error, so its
counts commit:

- **Per box:** 5 failures in 15 minutes block the box's PINs for 15 minutes, from any caller.
- **Per source:** the caller's number, which a caller can fake, so the box's limit is the real one: 10 failures in an
  hour block it for an hour, for every box.
- A box number with no box, a box with no PIN, and a blocked box are refused the same way, after the same work (a hash
  check against a stand-in), and counted the same way: nothing tells a caller which boxes exist.
- Three refusals end the call. Setting a new PIN clears the box's count. The daily trash job deletes failure counts a
  day old.

## Prompts

Moqui ships no recorded speech. Each prompt is a tone pattern, then the deployment's recording when
`voicemail_prompt_location` names a folder (any resource location, such as `dbresource://voicemail/prompts`,
uploaded through the Resources screen, or `component://my-component/prompts`). The files are 16-bit PCM WAV, mono or
stereo, 8 to 48 kHz, at most a minute. A file that is missing plays the tones only.

| File | Played | Tones |
| --- | --- | --- |
| `greeting.wav` | The default greeting (a user's own replaces it, tones and all) | 660, 880 Hz |
| (none) | The beep before the recording | 1000 Hz, 500 ms |
| `enter-box.wav` | The access number: "enter your number, then #" | 880 Hz three times |
| `enter-pin.wav` | "Enter your PIN, then #" | 880 Hz twice |
| `pin-refused.wav` | A refused PIN | 480, 380 Hz |
| `menu.wav` | After each message: "7 delete, 9 save, # next" | 700, 900 Hz |
| `no-messages.wav` | No new messages, or no more | 600 Hz |
| `goodbye.wav` | Before the engine hangs up | 880, 660, 440 Hz |

## Greetings

Recorded in the browser (Settings, Phone, Record: MediaRecorder, then the page decodes what it recorded and sends a
48 kHz mono WAV, which works the same in every browser) or uploaded (WAV or Ogg Opus, at most 10 MB; a minute is
kept). `store#VoicemailGreeting` checks the file's kind by its first bytes, then `convert#GreetingAudio` (the `sip`
feature) decodes it and keeps it as Ogg Opus under `dbresource://moqui/call/voicemail/greeting/<box>/`, which the
browser plays and the engine decodes for callers. A build without phone calls keeps the upload as it is. Use the
default greeting removes it.

## Access

- Only the box's owner lists, plays, downloads, marks heard or unheard, deletes, puts back, empties the trash, and
  retries a transcript (a ring group's box is its members', V3). Anything else answers "not found", the same as an
  id that does not exist.
- The audio is read only through `get#VoicemailAudio`, from the Calls Voicemail screen's `audio` transition; no route
  serves the `dbresource://moqui/call/voicemail/` locations. A record link from an app goes through the same service.
- The engine's services (`begin#`, `store#Voicemail`, `store#VoicemailFile`, `check#VoicemailPin`,
  `act#PhoneVoicemail`, `transcribe#Voicemail`, `send#VoicemailNotice`, `convert#GreetingAudio`) are internal
  (`allow-remote="false"`).

## Retention

Delete moves a message to the trash; deleting it there, Empty trash, or `expire_VoicemailTrash_daily` (messages in a
trash longer than `voicemail_trash_days`, 30 by default; 0 keeps them) delete it for good: its audio file, its
transcripts and its row.

## While the local node is away

Behind an edge node ([SIP](SIP.md#behind-an-edge-node), plans/Edge-Node.md §5.4, decision 4, E8) the edge node's
engine records every message and queues it to the local node with its audio (kind `voicemail`), which stores it
(`receive#EdgeVoicemail`) and tells its owner as above. The box is the one `begin#Voicemail` chose for the call, or,
when the local node did not route it, the called number's own: the user's (made when they have none), or the ring
group's fallback box.

While the local node is away the edge node takes the message itself, from the number's replicated fallback: the box's
greeting (copied to the edge node ahead, else the default), the beep and the recording; `*` opens no phone access, and
the access number is refused (480). The message waits on the edge node's queue, at most `queue-retention-days` (14),
and arrives once, with its notices, when the link is back; its `receivedDate` is when the caller left it. A call the
local node routed whose voicemail starts after it went away takes a message the same way. A message that cannot be
queued at once is kept in the spool with its details and tried again, from 2 seconds doubling to 5 minutes, and the
engine's next start queues what the spool still holds.

## Properties

| Property | Default | |
| --- | --- | --- |
| `voicemail_max_seconds` | `120` | The longest message, 10 to 600 |
| `voicemail_prompt_location` | empty | The folder of the prompts' recorded speech ([Prompts](#prompts)) |
| `voicemail_trash_days` | `30` | Days in the trash before a message goes; 0 keeps them |
| `voicemail_transcribe` | `false` | Send each message to the speech provider for a transcript |
| `voicemail_transcribe_language` | empty | An ISO 639-1 code; empty lets the model tell |
| `voicemail_pin_min_digits` | `6` | The fewest digits a new PIN has, 4 to 12 |
| `call_ring_seconds` | `45` | How long a call rings before voicemail when the box has no `rings` |

## Not yet

- **Speech from text.** The default prompts are tones and the deployment's recordings; no text-to-speech endpoint is
  called (moqui-llm has none yet).
- **The `external` room.** A voicemail is not yet an event in the number pair's `external` room: those rooms come with
  C1.
- **Ring groups.** A group's box has no PIN and no phone access.
- **Forward and reply by email**, Zimbra's other voicemail actions, are not here; download and Call back are.
- **Behind an edge node,** the fallback follows no call handling rules or schedules, and phone access waits for the
  local node.

## Tests

- `cargo test -p moqui-host --features sip --lib sip::voicemail`: an unanswered call's greeting, beep and recording to
  Ogg Opus until `#`; a recording's limit; the PIN wrong then right and the menu keys; a block or three refusals ending
  the call; the access number; a silent caller asked again, then ended; the prompts' audio; a WAV greeting converted to
  Ogg Opus.
- `cargo test -p moqui-host --features sip --test it -- sip::`: a fake trunk calls a busy user on a real server and
  leaves a message, stored as Ogg Opus in their inbox (`a_busy…`); a caller on the access number keys the box number and
  PIN, hears a message and deletes it with `7` (`the_access_number…`).
- `./moq test framework --spec SipDevice`: a user with registered phones and a box: the tab and both phones still ring
  past `call_ring_seconds`, voicemail times them all out, and a decline on one phone stops the others and goes to
  voicemail.
- `cargo test -p moqui-host --test it edge_sip::`: a message routed by the local node and one left while it was away,
  both recorded on the edge node and stored on the local node ([SIP, Tests](SIP.md#tests)); `./moq test framework
  --spec EdgeVoice`: `receive#EdgeVoicemail` only from the queue, in the number's box.
- `./moq test framework --spec Voicemail`: busy and unanswered routing, begin and decline, private audio refused for
  another user with the same id, the PIN with its refusal and its limits per box and per source, the notices
  (attachments off by default), a failed transcription and its retry, the trash's retention, greeting uploads.
- `./moq test basic-comms --spec BasicCommsCalls`: the Voicemail tab and its audio transition for its owner only;
  `BasicComms`: the System Voice screens through the System app, and History's Call a number.
