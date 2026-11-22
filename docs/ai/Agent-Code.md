# Agent Code

Agent Code runs coding agents on `AgentConversation` threads: MR Moqui's own engine, and later Codex, Claude and the
other harnesses. It is a port of T3 Code's features ([NOTICE](../../runtime/components/base/agent-code/NOTICE)). The
plan is [plans/agent-code](../../plans/agent-code/README.md); phase P0, the base, is built. The app screens come in
phase P1.

The code: the component [runtime/components/base/agent-code](../../runtime/components/base/agent-code/README.md), the
crate [crates/moqui-code](../../crates/moqui-code), and the socket in `crates/moqui-host/src/agentws.rs` (the host's
`code` feature, in every package).

## Turn it on

The socket and the descriptor are on where the `/agentws` endpoint of the webroot webapp is on:

```xml
<webapp name="webroot">
    <endpoint path="/agentws" enabled="true"/>
</webapp>
```

| `enabled` | Effect |
|---|---|
| `dev` (default) | On only on a dev instance (`<instance purpose="dev"/>`) |
| `true` | On |
| `false` | Off; `/agentws` and the descriptor are 404 |

Give each user a group:

| Group | Permissions | For |
|---|---|---|
| `AGENT_CODE_USERS` | `AgentCodeUse` | Own threads, own settings |
| `AGENT_CODE_HOSTS` | `AgentCodeUse`, `AgentCodeHost` | Run as the server's OS user (terminals, scripts, full access) |
| `AGENT_CODE_ADMINS` | `AgentCodeUse`, `AgentCodeAdmin` | Server settings and policy |

`ADMIN` has `AgentCodeUse` and `AgentCodeAdmin`. `AgentCodeHost` and `AgentCodeAdmin` are sealed: only a holder grants
them.

## Commands

A client changes state only through a command: `org.moqui.code.CommandServices.dispatch#Command`, or the method
`orchestration.dispatchCommand` on the socket. A command is a map with `type`, a `commandId` that the client makes
(1 to 40 letters, digits, `-`, `_` or `.`), and the fields of its type.

- The rows, the changes, the effects and a receipt are written in one transaction.
- The same `commandId` again answers the stored receipt (`replayed` true) and writes nothing.
- A refusal has an error code: `invalid_command`, `unknown_command`, `internal_only`, `not_found`, `conflict`,
  `forbidden` or `failed`.

The thread commands of P0: `thread.create`, `archive`, `unarchive`, `delete`, `settle`, `unsettle`, `snooze`,
`unsnooze`, `auto-settle.set`, `pin`, `unpin`, `pin.reorder`, `active.reorder`, `tag.set`, `visit`, `mark-unread`,
`notify.set`, `metadata.update`, `runtime-mode.set`, `interaction-mode.set` and `model-selection.set` (each is
`thread.<name>`).

Work that must not run in the transaction (a title from a model, later a git push) is an effect: a row the effect worker
runs after the commit. The worker runs each minute (`run_AgentCodeEffects_frequent`) and when a command commits. A
failed effect tries again after 5 seconds, doubled each time, up to 8 tries.

## The socket

`GET /agentws?protocol=1` is a WebSocket of JSON text frames. Sign in with the session cookie or a full-account
`login_key` header. The first frame is the hello:

```json
{"hello": {"environmentId": "...", "protocol": 1, "serverVersion": "...", "capabilities": {...}, "user": {"userId": "..."}}}
```

| From | Frame |
|---|---|
| client | `{"id":7,"m":"<method>","p":{...}}`; with `"s":true` it opens a stream |
| server | `{"id":7,"r":{...}}` answer, or `{"id":7,"e":{"code","message","requiredScope","requiredPermission"}}` |
| server | `{"id":8,"i":{...}}` stream item, `{"id":8,"end":true}` stream end |
| client | `{"id":8,"cancel":true}` ends a stream |
| both | `{"ping":n}`, `{"pong":n}` |

Methods: `server.getConfig`, `server.getDescriptor`, `orchestration.dispatchCommand`,
`orchestration.getShellSnapshot`, `orchestration.subscribeShell` (a stream), `settings.getClient`,
`settings.updateClient`, `settings.getServer`, `settings.updateServer` and `settings.getSchema`.

`orchestration.subscribeShell` sends a snapshot item (`{"kind":"snapshot"}`) and then a `{"kind":"change"}` item for each
change. With `afterSequence` it sends only the changes after that number.

Limits: a frame is at most 1 MiB, a socket holds at most 64 streams, and a socket silent for 60 seconds is closed. The
server sends a ping each 25 seconds.

`GET /.well-known/moqui/agent-code` answers the environment descriptor: the environment id, label, platform, server
version, protocol and capabilities. It has no user data.

## Settings

`org.moqui.code.SettingsServices.get#Settings` and `update#Settings`, with `kind`:

| `kind` | Stored in | Needs |
|---|---|---|
| `client` | `UserPreference` `code.<name>` (device settings stay on the device) | `AgentCodeUse` |
| `server` | `AgentCodeSetting` | `AgentCodeAdmin` to change |
| `policy` | `AgentCodePolicy` | `AgentCodeAdmin` |
| `schema` (read only) | `settings-schema.json` | `AgentCodeUse` |

An unknown name or a value of the wrong type is refused before anything is stored. A null value restores the default.

## Test

`./moq test agent-code` runs the regression specs; `cargo test -p moqui-code` runs the unit tests.
