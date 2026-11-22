# 10. Web UI and TUI

Part of [Agent Code](README.md). T3 sources: `t3:apps/web/src/**`, `t3:packages/contracts/src/{settings,keybindings}.ts`,
`t3:packages/shared/src/keybindings.ts`, `t3:docs/user/{thread-sidebar,composer,keybindings,keyboard-focus,
appearance,welcome-page}.md`, `t3:docs/internals/web-ui.md`.

## 1. Platform

| T3 | MR Moqui |
|---|---|
| React, TanStack Router, Effect atoms, React Compiler; one bundle for browser and Electron | Vue 3.5 and Quasar 2.35 in the `/qapps` shell, no build step ([docs/ui/Qvt-Client.md](../../docs/ui/Qvt-Client.md)). The same server pages run in the browser, the Tauri desktop and the Android app. |
| Route files | XML screens under `runtime/components/base/agent-code/screen/AgentCode/`, mounted at `/qapps/code` by a `subscreens-item` in the component `MoquiConf.xml` under `apps.xml`. |
| Components | `.qvue` files beside the XML screens where XML widgets cannot do the job ([Practices-App-Dev.md §6](../../docs/dev/Practices-App-Dev.md)). The XML screen keeps transitions, parameters and authz. |
| Client stores | `moqui.code` JS module (`screen/AgentCode/js/AgentCode.js`, served static) with Vue `reactive` stores; persisted ones in IndexedDB (browser) or the app store (apps). |
| Diff workers | Web workers from `/codestatic/js/diff-worker.js`. |
| Markdown, Mermaid, math | The Assist markdown renderer plus Mermaid and KaTeX loaded on demand from `webroot/libs`. |

Rules from [Practices-App-Dev.md](../../docs/dev/Practices-App-Dev.md) apply: every screen works in qvt and in tui;
lists use `columns type="mobile"`; one primary action; danger actions ask; states are chips with qvt and tui texts.

### 1.1 Screens

| Screen | Kind | Purpose |
|---|---|---|
| `AgentCode.xml` | XML app root | Menu, authz (`AgentCodeUse`), subscreens. |
| `Workbench.xml` + `Workbench.qvue` | `.qvue` | The shell: sidebar, thread view, composer, right panel, terminal drawer. Handles `/qapps/code`, `/new`, `/draft/{id}`, `/t/{id}`, `/linked/{linkId}/{id}`. |
| `Workbench.tui.json` | tui twin | Sidebar list + thread view + composer for `moqui tui` (§13). |
| `PullRequests.xml` + `PullRequests.qvue` | `.qvue` | List and detail ([§6](#6-diff-and-pull-request-pages)). tui twin: XML list + detail sections. |
| `Issues.xml` | XML (list, detail) | Issues ([04 §11](04-Git.md#11-issues)). |
| `Usage.xml` + `Usage.qvue` | `.qvue` (charts) | Usage ([07 §5](07-Secrets-Guest-Voice-Usage.md#5-usage)). tui twin: XML tables. |
| `Settings/*.xml` | XML forms; `.qvue` only for the keybindings editor, the theme editor and the connections map | [§9](#9-settings). |
| `Welcome.xml` | XML landing (`nav-card`) | Agents setup, phone pairing, projects. |
| `Pair.xml` | XML | `/qapps/code/pair#token=...` pairing a browser. |
| `ApproveAgent.xml` | XML | MCP OAuth approval page ([08 §1.4](08-MCP-Tools.md#14-outside-agents-oauth)). |
| `Guest.xml` + `Guest.qvue` | outside `/qapps`, bare page at `/guest/{shareId}` | Guest viewer ([07 §3](07-Secrets-Guest-Voice-Usage.md#3-guest-share)). |

## 2. Routes

| T3 route | MR Moqui path |
|---|---|
| `_chat.index` (home) | `/qapps/code` |
| `_chat.new` | `/qapps/code/new` |
| `_chat.draft.$draftId` | `/qapps/code/draft/{draftId}` |
| `_chat.$environmentId.$threadId` | `/qapps/code/t/{threadId}` on its own environment; `/qapps/code/e/{environmentId}/t/{threadId}` for another connected environment (apps only, §3) |
| `_chat.linked.$via.$linked.$thread` | `/qapps/code/linked/{linkId}/t/{remoteThreadId}` |
| `_chat.pull-requests` | `/qapps/code/PullRequests` |
| `_chat.issues` | `/qapps/code/Issues` |
| `_chat.usage` | `/qapps/code/Usage` |
| `_chat.settings.*` | `/qapps/code/Settings/{Page}` |
| `threads.$env.$thread` (deep link) | `/qapps/code/t/{threadId}` and app link `moqui://code/t/{environmentId}/{threadId}` |
| `projects.$projectKey` | `/qapps/code/p/{projectId}` |
| `welcome`, `pair`, `connect`, `connect-agent`, `approve-agent`, `guest` | `Welcome`, `Pair`, (edge pairing is in Settings > Connections), `ApproveAgent`, `/guest/{shareId}` |

Thread URLs never carry secrets. The pairing token is in the URL fragment and the page strips it from history.

## 3. Environments in one page

T3 web and desktop show threads of several environments in one sidebar. MR Moqui does it this way:

| Surface | Environments in the sidebar | How |
|---|---|---|
| Browser | This server, and every linked environment (peer links) | The page uses the primary `/agentws`. Linked threads come through the server's peer links ([13 §6](13-Servers-Linking.md#6-peer-links)). |
| Desktop and Android | Every account in the app (each is an environment), plus their peer links | The host holds one `/agentws` per account (`crates/moqui-client/src/code/`). The page reaches them through `moqui.host.code.environments()`. |

- `moqui.code.environments` is one JS API for all three sources: `primary`, `account:<id>`, `link:<id>`.
- Each sidebar row shows its machine by artwork, pill or none (`environmentIdentificationMode`).
- Load balancing (client setting, off by default): Normal, Prefer, Less often, Manual only per environment. It picks
  the environment for new drafts in projects grouped across environments.
- A browser can also add a direct connection to another environment by pairing (bearer key in IndexedDB, CORS for
  `/agentws` tickets only). Open decision O12 ([16](16-Phases.md)). Until then, use a peer link.

## 4. Shell and sidebar

### 4.1 Shell

- The Workbench runs inside the `/qapps` shell (`m-qapps-root`). The shell header stays. Agent Code hides the Assist
  drawer button on its screens (the right side is the Agent Code right panel).
- The shell palette (`Ctrl+K`) gets Agent Code providers: `paletteActions`, `paletteTypes` (thread, project, PR,
  issue), `paletteSources` (§7.2). There is one palette, not two.
- `quickCreate` gets **New thread**. `todaySources` gets **Waiting for you** (approvals, questions).
- `notificationTopics` gets the four Agent Code topics ([07 §2](07-Secrets-Guest-Voice-Usage.md#2-notifications)).
- `shellCounts` gets the Agent Code attention count.

### 4.2 Sidebar

| T3 | MR Moqui |
|---|---|
| Sections: **Pinned**, active list, **Working** (folded, beta, on by default, `sidebarWorkingShelfEnabled`), **Snoozed**, **Settled** | Same, in `m-code-sidebar`. |
| "Boring sidebar" (Labs): one folder per project | Same setting. |
| Project grouping `repository` (default), `repository_path`, `separate`; per-project overrides | Same. Grouping uses `AgentProject.repositoryKey`. |
| Row actions: pin, unpin, settle, un-settle, snooze (presets, **Custom...**), wake, archive, rename (`F2`), regenerate title, **Auto-settle behavior** (Never, Follow settings), **Turn into project**, copy reference, copy thread id, Move up/down | Same. Each is a command ([02 §1](02-Orchestration.md#1-the-command-pipeline)). |
| Undo toast 5 s for unpin, settle, snooze, archive and draft discard; `mod+z` | Same. Undo sends the reverse command. |
| Drag between sections to pin, settle, wake; drag to reorder (saved on the server, synced); drag files onto a row; press **Settle** and drag across rows; `Escape` cancels | Same. Order in `AgentConversationUser.pinOrderKey` / `activeOrderKey`. |
| Multi-select snooze | Same. |
| Auto-settle rules on the server | [02 §3](02-Orchestration.md#3-settle-snooze-pin-tags-unread). |
| Start in background `mod+enter` in a draft; Shift-click several models for one thread and worktree per model | Same. |
| `F6`/`Shift+F6` move between sidebar, conversation, composer and panels; arrows browse; `Shift+F10` row menu | Same, through `moqui.keymap` scope `code`. |
| Swipe for history (touch) | Same on touch devices. |

## 5. Thread view and composer

### 5.1 Header

Title, branch toolbar, environment selector, workspace mode selector, git actions (stacked actions,
[04 §4](04-Git.md#4-vcs)), open-in-editor (desktop host only, else hidden), project scripts.

### 5.2 Timeline

| T3 | MR Moqui |
|---|---|
| User, assistant, work entry, work group, lifecycle, pending approval, system divider rows | `m-code-timeline` with row components per turn item kind ([01 §3.1](01-Data-Mapping.md#31-turn-items)). |
| Minimap | Same. |
| `toolCallVisibility` showAll, hideSuccessful, hideAll, zen | Same. |
| Markdown with Mermaid, math (opt-in), tables, code blocks, file links, thread links, citation chips | Same. |
| Cards: proposed plan, secret request, link request, worktree setup, MCP App frame, HTML render frame, command output panel, shell command block | One component each. MCP App and HTML frames are sandboxed iframes ([06 §3-4](06-Terminal-Preview-Devices.md#3-html-renders-visual-replies)). |
| Find (`mod+f`) searches the full thread on the server | `orchestration.searchThread`. |
| Rewind and edit an earlier prompt | Command `thread.rewind` (checkpoint restore, [02 §7](02-Orchestration.md#7-checkpoints-and-rollback)). |
| Selection toolbar on assistant text (quote into composer) | Same. |
| Virtual scroll | Yes; threads can have thousands of items. History pages from `/rest/s1/code/threads/{id}/history`. |

### 5.3 Banners, details, subagents, side chats

- Banners above the composer: provider status, usage-limit recovery, thread errors, server update, held send, fresh
  session, project clone.
- Thread details card and panel: repositories, PRs, issues, usage, side chats, subagents, automations, servers.
  Rows chosen by `threadDetailsItems`.
- Subagents: subagent bar, child approvals, native child work dialog. Subagents, side chats and linked threads open
  as right panel tabs; `mod`-click opens them in the main view (`sideThreadOpenTarget`).
- Side chats: `/side` or `/btw`. They share the thread's folder and start from its context.
- Guest share controls.

### 5.4 Right panel

- Tab kinds: files, diffs, pull requests, browsers, devices, side threads.
- `view.reopenClosed` (`mod+shift+t`) reopens closed tabs. `rightPanel.toggleMaximized`.
- At 980 px or less, the panel is a sheet (§10).

### 5.5 Composer

| T3 | MR Moqui |
|---|---|
| Tiptap rich text (`composerRichTextEnabled`, default true), code blocks with a language picker, list continuation, RTL detection, undo grouping | A ProseMirror editor (Tiptap core is MIT, no React needed) in `webroot/libs`, loaded by `m-code-composer`. Plain textarea when off. |
| `/` commands (provider commands plus `/model`, `/side`, `/btw`, `/clear`, `/compact`, `/goal`), skills unless `showSkillsInSlashMenu` off | Same. Provider commands come from the instance capabilities. Skills from `AgentSkill`. |
| `$` skill, `@` file / thread / side chat, `#` PR picker | Same. |
| Context chips: terminal excerpt, diff or file review comment, preview annotation or picked element, file, PR, issue, thread, model mention; optional context strip | Same. Chips become `AgentMessagePart` rows of kind context. |
| Attachments: files, images, video; upload URL | `attachments.createUploadUrl` ([09 §5](09-Wire-API.md#5-http-routes)). |
| Footer: model picker (`mod+shift+m`, favorites, per-provider sidebar), effort (`mod+shift+e`, label or bars), mode (`mod+shift+a`), host (`mod+shift+h`), workspace mode (`mod+shift+x`, `mod+shift+l`), branch (`mod+shift+g`), dictation (`mod+shift+d`), usage limits | Same. Host = the work root host node in a cluster, or the environment. |
| `sendShortcut` enter, mod-enter-multiline, mod-enter | Same. |
| While a turn runs: `followUpBehavior` steer (default) or queue; `mod+enter` does the other | Same. |
| Empty composer while running shows **Interrupt**; after a stop **Resume** | Same. |
| `mod+alt+enter` send and open a new thread | Same. |
| Queue as dashed messages: Send now, edit (`alt+arrowup`), remove; `mod+shift+enter` steers the oldest | Same. |
| Prompt stash `mod+s`; history recall; suggested next prompt (`promptSuggestionsEnabled`) | Same. Stash in `UserPreference` `code.stash` (synced), drafts per thread in the client store. |
| Approvals and questions replace the editor area | Same. Approval decision uses the existing approval services, so the browser-session rule holds ([14 §4](14-Security.md#4-approvals)). |

## 6. Diff and pull request pages

### 6.1 Diff pane

- `diff.toggle` (`mod+d`). Sources: turn diff, full thread diff, branch against the base, uncommitted changes, single
  commits (newest 100). Repository picker in multi-repo projects.
- Turn diffs show only the turn's own work ([04 §4](04-Git.md#4-vcs)).
- `diffLayout` stacked or split; `diffIgnoreWhitespace` (true); `diffFilesCollapsed` (true); `diffColorScheme`
  red-green or blue-orange; `wordWrap`.
- Rendering in a web worker pool. Changed-files tree.
- Line review comments become composer chips.
- Large diffs load by file from `/rest/s1/code/pullRequests/diff` or `review.getDiffFileContents`.

### 6.2 Pull requests page

- List: filters, groups, saved Views (`UserPreference` `code.prViews`, so they follow the user; T3 keeps them per
  browser), Shift quick actions, drag to close several.
- Detail tabs: Summary, Timeline, Code, Guide (beta).
- Actions: edit title and body, tick checkboxes, comment, reply, resolve, react, request reviewers, labels, submit
  review, merge (method, auto-merge), checks popover, stack popover, files viewed, change breakdown.
- **Work on PR** prepares a worktree thread.
- PR sharing per environment: **Read PRs** or **Read and act**.
- Hosts: [04 §1](04-Git.md#1-code-hosts).
- tui: the list is an XML form-list; the detail is XML sections (summary, checks, comments). The Code tab in tui shows
  a unified diff text.

## 7. Terminal drawer, palette and other pages

### 7.1 Terminal drawer

- The existing terminal view (`m-terminal-view`) in a drawer per thread. `terminal.toggle` (`mod+j`), `terminal.new`,
  `terminal.split`, `terminal.splitVertical`, `terminal.close` as §8.
- Terminal layout per thread in the client store. Links in output open in the file viewer.
- `confirmTerminalClose` (true). Idle terminals close when the thread settles (`stopOnSettle`).

### 7.2 Palette

- Shell palette, `mod+k`. Searches commands, projects and threads across connected environments (title, branch, PR,
  project). Ranking by frequency, recency and the last pick per query for three weeks (`UserPreference`
  `code.paletteHistory`).
- **In messages** group: threads whose messages hold all query words (min 3 letters, 2 for CJK). Server search
  `orchestration.searchThreads`.
- A leading `>` shows only actions.
- Items: threads, projects (add project), themes, port forwards (desktop).
- Actions include **Restart agent session**, **New thread in...**, focus next or previous area, browser find.
- File picker `mod+p`; project content search `mod+shift+f`.

### 7.3 Other pages

- Usage: cost, tokens, limits views; day, week, month, quarter; model prices; model filter; subscription limits.
- Issues: list and detail.
- Welcome: agents setup, phone pairing (QR of a `ClientPairingCode`), projects.
- Guest: read-only shared thread.

## 8. Keybindings

- Rule store: `UserPreference` `code.keybindings`, a JSON array of `{key, command, when?}`. T3 keeps it per machine in
  a file; MR Moqui keeps it per user, so it follows the user to every device. A device can add local rules in the
  client store (`isDesktop` rules, for example).
- Limits: 256 rules, key max 64 chars, `when` max 256 chars, depth 64. The last matching rule wins.
- `mod` = Cmd on macOS, Ctrl elsewhere.
- Context keys: `terminalFocus`, `terminalOpen`, `previewFocus`, `previewOpen`, `rightPanelOpen`, `modelPickerOpen`,
  `usagePageOpen`, `composerFocus`, `draftThreadRoute`, `turnRunning`, `composerEmpty`, `editableFocus`, `isWeb`,
  `isDesktop`, plus `isPhone` (new). Operators `!`, `&&`, `||`, parentheses. Unknown keys are false.
- Engine: a `when` evaluator in `AgentCode.js` on top of `moqui.keymap.registerScope('code', ...)`.
- Editor: Settings > Keybindings writes `server.upsertKeybinding` / `removeKeybinding`.
- Script commands: `script.<id>.run` (id `^[a-z0-9][a-z0-9-]*$`, max 24).

Defaults (kept from T3 `DEFAULT_KEYBINDINGS`):

| Key | Command | When |
|---|---|---|
| `mod+b` | `sidebar.toggle` | |
| `f6` / `mod+f6` | `workspace.focusNext` | |
| `shift+f6` / `mod+shift+f6` | `workspace.focusPrevious` | |
| `mod+[` | `navigation.back` | `!terminalFocus` |
| `mod+]` | `navigation.forward` | `!terminalFocus` |
| `mod+j` | `terminal.toggle` | |
| `mod+alt+b` | `rightPanel.toggle` | |
| `mod+shift+b` | `threadPanel.toggle` | `!terminalFocus` |
| `mod+t` | `rightPanel.new` | `rightPanelOpen && !terminalFocus` |
| `mod+d` | `terminal.split` | `terminalFocus` |
| `mod+shift+d` | `terminal.splitVertical` | `terminalFocus` |
| `mod+shift+d` | `composer.dictation` | `!terminalFocus` |
| `mod+n` | `terminal.new` | `terminalFocus` |
| `mod+w` | `terminal.close` | `terminalFocus` |
| `mod+w` | `rightPanel.close` | `!terminalFocus` |
| `mod+shift+t` | `view.reopenClosed` | |
| `mod+d` | `diff.toggle` | `!terminalFocus` |
| `mod+shift+j` | `preview.toggle` | |
| `mod+r` | `preview.refresh` | `previewFocus` |
| `mod+l` | `preview.focusUrl` | `previewFocus` |
| `mod+=` / `mod++` | `preview.zoomIn` | `previewFocus` |
| `mod+-` | `preview.zoomOut` | `previewFocus` |
| `mod+0` | `preview.resetZoom` | `previewFocus` |
| `mod+f` | `preview.find` | `previewFocus` |
| `mod+k` | `commandPalette.toggle` | `!terminalFocus` |
| `mod+p` | `filePicker.toggle` | `!terminalFocus` |
| `mod+shift+f` | `projectSearch.toggle` | `!terminalFocus` |
| `mod+u` | `usage.open` | `!terminalFocus` |
| `mod+alt+a` | `theme.select` | `!terminalFocus` |
| `mod+alt+shift+a` | `appearance.cycle` | `!terminalFocus` |
| `mod+alt+shift+t` | `themeEditor.toggle` | |
| `mod+s` | `composer.stash` | `!terminalFocus` |
| `mod+shift+enter` | `thread.steerQueuedMessage` | `!terminalFocus` |
| `alt+arrowup` | `thread.editQueuedMessage` | `composerFocus` |
| `mod+enter` | `composer.sendAlternate` | `composerFocus && turnRunning` |
| `mod+enter` | `composer.sendBackground` | `composerFocus && draftThreadRoute` |
| `mod+alt+enter` | `composer.sendBackground` | `composerFocus && draftThreadRoute` |
| `mod+alt+enter` | `composer.sendAndNewThread` | `composerFocus && !draftThreadRoute` |
| `mod+n` | `chat.new` | `!terminalFocus` |
| `mod+shift+o` | `chat.new` | `!terminalFocus` |
| `mod+shift+n` | `chat.newLocal` | `!terminalFocus` |
| `mod+alt+n` | `chat.newWithoutProject` | `!terminalFocus` |
| `mod+f` | `chat.find` | `!terminalFocus && !previewFocus` |
| `mod+shift+m` | `modelPicker.toggle` | `!terminalFocus` |
| `mod+shift+h` | `composer.host` | `!terminalFocus` |
| `mod+shift+e` | `composer.effort` | `!terminalFocus` |
| `mod+shift+a` | `composer.mode` | `!terminalFocus` |
| `mod+shift+x` | `composer.workspace` | `!terminalFocus` |
| `mod+shift+g` | `composer.branch` | `!terminalFocus` |
| `mod+shift+l` | `composer.previousWorktree` | `!terminalFocus` |
| `mod+shift+k` | `pullRequest.copyNumber` | `!terminalFocus` |
| `mod+shift+arrowup` | `modelPicker.previousProvider` | `modelPickerOpen` |
| `mod+shift+arrowdown` | `modelPicker.nextProvider` | `modelPickerOpen` |
| `mod+o` | `editor.openFavorite` | |
| `mod+shift+[` | `thread.previous` | |
| `mod+shift+]` | `thread.next` | |
| `mod+shift+c` | `thread.copyReference` | `!terminalFocus` |
| `mod+shift+s` | `thread.settle` | `!terminalFocus` |
| `mod+shift+p` | `thread.pin` | `!terminalFocus` |
| `mod+z` | `thread.undo` | `!terminalFocus && !editableFocus` |
| `mod+1` ... `mod+9` | `thread.jump.1` ... `.9` | `!modelPickerOpen && isDesktop` |
| `mod+1` ... `mod+9` | `modelPicker.jump.1` ... `.9` | `modelPickerOpen && isDesktop` |
| `c` | `usage.cost` | `usagePageOpen` |
| `t` | `usage.tokens` | `usagePageOpen` |
| `l` | `usage.limits` | `usagePageOpen` |
| `mod+shift+1` ... `mod+shift+4` | `usage.period.day`, `.week`, `.month`, `.quarter` | `usagePageOpen` |

No default key: `thread.stop`, `thread.regenerateTitle`, `rightPanel.toggleMaximized`, `composer.cycleHost`,
`composer.increaseEffort`, `composer.decreaseEffort`, `script.<id>.run`.

Conflicts with MR Moqui:

| Key | MR Moqui use today | Rule on Agent Code screens |
|---|---|---|
| `mod+k` | Shell palette | Same palette (§4.1). No conflict. |
| `` Ctrl+` `` | Quick terminal | Kept. |
| Terminal Ctrl+Shift chords | Terminal view keys | Inside `terminalFocus` the terminal keymap wins. |
| `mod+b` | none | `sidebar.toggle` does not take a Ctrl chord from a focused terminal (tmux `Ctrl+B` works). |
| Browser keys (`mod+t`, `mod+w`, `mod+n`, `mod+shift+t`, `mod+1-9` in a browser tab) | Browser | In a browser tab these stay with the browser. Defaults with those keys apply only when `isDesktop` or the page is an installed PWA. |

Other rules: `mod+w` never closes the desktop window. Desktop quit `Cmd+Q`/`Ctrl+Q`; in Hold mode hold 1.2 s or press
twice within 500 ms (`confirmQuit` direct, hold, double-click). iPad keyboard (MD7): `Cmd+1`-`9`, `Cmd+K`,
`Cmd+Shift+H`, Return sends. Android uses Ctrl.

## 9. Settings

### 9.1 Where each kind lives

| T3 kind | MR Moqui store | Scope |
|---|---|---|
| Client settings (per device) | `UserPreference` `code.<name>` for most (they follow the user); the client store for the ones marked **device** below | user or device |
| Server settings (`settings.json`) | `AgentCodeSetting` | environment |
| Project overrides | `AgentProjectSetting` | project |
| Managed policy | `AgentCodePolicy` | environment, `AgentCodeAdmin` only |
| Provider instances | `AgentProviderInstance` | environment |
| Keybindings | `UserPreference` `code.keybindings` | user |

The settings service (`get#Settings`, `update#Settings`) validates each name and type against one schema file,
`service/org/moqui/code/settings-schema.json`, a port of `t3:settings.ts`. Unknown names are refused.

### 9.2 Client settings

All names, types and defaults are kept from `ClientSettingsSchema`:

| Name | Values | Default | Store |
|---|---|---|---|
| notificationMode | off, notifications, sound, notifications-and-sound | off | device |
| inAppNotificationsEnabled | boolean | false | user |
| diffColorScheme | red-green, blue-orange | red-green | user |
| chatWidth | comfortable, wide, full | comfortable | user |
| effortDisplay | label, bars | label | user |
| loadBalancingEnabled, loadBalancingWeights | boolean, record | false, {} | device |
| appearanceContrast | range | 100 | user |
| panelAnimationDurationMs | range | 0 | user |
| browserDefaultViewport, browserDefaultZoomFactor, browserDefaultAppearance | viewport, number, appearance | fill, 1.0, system | user |
| browserRecordingFrameRate, browserRecordingShowKeyPresses, browserRecordingShowMousePresses | number, boolean, boolean | 30, false, false | user |
| browserLinkTarget | system, app | system | device |
| browserSearchEngine, browserCustomSearchUrl (max 2048) | duckduckgo, google, bing, brave, kagi, ecosia, custom | duckduckgo, "" | user |
| browserAutoShowFloatingPreview | boolean | true | user |
| browserProfiles, browserDefaultProfileId | list, id | [], default | user (rows in `AgentBrowserProfile`) |
| confirmQuit | direct, hold, double-click | hold | device |
| confirmThreadArchive, confirmTerminalClose, confirmThreadDelete, confirmThreadUnpin | boolean | false, true, true, false | user |
| diffFilesCollapsed, diffIgnoreWhitespace, diffLayout | boolean, boolean, stacked/split | true, true, stacked | user |
| environmentIdentificationMode | artwork, nightly-artwork, dev-artwork, pill, none | artwork | user |
| glassOpacity | range | 80 | user |
| fontSizeInterface, fontSizePrompt, fontSizeCode, fontSizeTerminal | number | 16, 14, 13, 12 | device |
| fontFamilyCode, fontFamilyComposer, fontFamilySans, fontFamilyTerminal | string | "" | device |
| fontSmoothing | boolean | true | device |
| onboardingCompletedAt | string or null | null | user |
| persistComposerContextStrip | boolean | false | user |
| favorites, providerModelPreferences | list, record | [], {} | user |
| pullRequestMergeMethodOverrides, pullRequestLinksOpenInApp | record, boolean | {}, true | user |
| composerRichTextEnabled | boolean | true | user |
| sendShortcut | enter, mod-enter-multiline, mod-enter | enter | user |
| followUpBehavior | queue, steer | steer | user |
| sideThreadOpenTarget | panel, main | panel | user |
| proactivePanelsEnabled, showSkillsInSlashMenu | boolean | false, true | user |
| boringSidebarEnabled, boringSidebarMultilineRows | boolean | false, false | user |
| toolCallVisibility | showAll, hideSuccessful, hideAll, zen | showAll | user |
| screenReaderAnnouncements, screenReaderWindowTitle | status/messages/everything, boolean | status, false | user |
| threadDetailsUsageEnabled, threadDetailsItems | boolean, record | false, {} | user |
| sidebarWorkingShelfEnabled | boolean | true | user |
| sidebarProjectGroupingMode, sidebarProjectGroupingOverrides | repository, repository_path, separate; record | repository, {} | user |
| sidebarProjectSortOrder, sidebarThreadSortOrder | updated_at, created_at, manual; updated_at, created_at | updated_at | user |
| timestampFormat | locale, 12-hour, 24-hour | locale | user |
| voiceMicrophone, voiceMicrophones | string, list | "", [] | device |
| voiceShortcutMode | auto, hold, toggle | auto | user |
| speechLanguage, speechTranslateToEnglish, speechCustomWords, speechSilenceFiltering, speechStreaming, speechRemoveFillerWords, speechPostProcessingPrompt, speechCorrectionWord | as T3 | auto, false, [], true, true, true, {default}, "" | user |
| language | string | en | the user's Moqui locale (not a second setting) |
| snapShot* (enabled, includeAccessibility, shortcut, playSound, sound, flash, animations) | as T3 | false, true, default, true, soft-pop, true, true | device (desktop only) |
| wordWrap, mathRenderingEnabled | boolean | true, false | user |

### 9.3 Server settings

All names, types and defaults are kept from `ServerSettings`, with these MR Moqui changes:

| Name | Change |
|---|---|
| defaultRuntimeMode | T3 default `full-access`. MR Moqui default `approval-required`. The reason: a production server with many users. An admin can raise it up to `limits.maxRuntimeMode`. |
| textGenerationModelSelection | Default is the `LlmFeatureBinding` for `code-title` and the other features ([03 §8](03-Providers.md#8-text-generation)), not a fixed model. |
| branchNamePrefix | Default `moqui` (T3 `t3`). |
| cloudSpeechCredentials | Values are secret sources ([07 §1](07-Secrets-Guest-Voice-Usage.md#1-secrets)), never plain text in the setting. |
| providerInstances, sourceControlHosts, usageLimitSources, textGenerators, secretProviders | Rows in their entities, not a JSON setting. The settings service reads and writes the rows. |
| favorites, providerModelPreferences | Server-wide defaults; the user's own are client settings. |
| requireConnectDeviceApproval | Named `requireDeviceApproval`: applies to app devices and edge-paired devices. |
| keepAwakeWhileAgentsWork | Applies to the desktop in-process node only ([11 §7](11-Desktop.md#7-power-and-sleep)). |
| defaultTheme, defaultAppearance | Map to the instance theme settings. |

Unchanged names (as T3): worktreeCleanup, storageCleanup, worktreesDirectory, fastWorktreeCheckout,
previousWorktreesDirectories, responseStreamingMode (turn, paragraph; paragraph), enableProviderUpdateChecks (true),
continueThreadsAfterServerUpdate (true), enableAgentBrowserAccess (true), enablePreviewTunnel (true), speechModelId,
speechAcceleration (auto), speechModelUnloadTimeout (min_15), speechProjectCustomWords,
speechPostProcessingModelSelection, projectAgentBrowserAccessOverrides, defaultAutoPull (false),
defaultProjectScripts, projectScriptOverrides, projectAutoPullOverrides, defaultModelSelection,
fallbackModelSelection, projectSettingsOverrides, projectSettingsFolded, enableAgentDeviceAccess (false),
enableDeviceSupport (false), deviceOnboardingCompleted, deviceHosts, easDeviceHosts, simlockDeviceHosts,
simlockDeviceHostsEnabled, sidebarAutoSettleAfterDays (7), compactIdleThreads, idleCompaction,
snoozeLimitedThreads, autoResumeLimitedThreads, snoozePresets, hiddenThreadTags, hideTaggedThreads,
guestSharingEnabled (false), linkedEnvironmentsEnabled (false), promptSuggestionsEnabled (false),
sidebarAutoSettleKeepPullRequests (false), sidebarAutoSettleOnMerge (true), backgroundActivity (balanced),
automaticGitFetchInterval (30 s), providerHealthRefreshInterval (5 min), backgroundActivityProfile,
environmentIcon, defaultThreadEnvMode, newWorktreesStartFromOrigin (true), worktreeSubmodules,
addProjectBaseDirectory, ghqCloneLayout, threadTitleInstructions, branchNamingMode (static), branchNameInstructions,
removeAgentCreditsOnMerge (false), sourceControlWritingStyle, sourceControlWriterModelSelection, reviewGuidesEnabled
(false), reviewGuideModelSelection, reviewGuideInstructions, pullRequestMergeMethod, observability,
offeredSourceControlProviders, usageHistoryRetentionDays (92), usagePriceOverrides, usageModelAliases,
usageModelFilter.

Provider defaults as T3: cursor, grok, fx, kiro, muse, pi, opencode, antigravity are off unless set up. codex and
claude are on when found. The native engine is on when an `LlmEndpoint` exists.

### 9.4 Settings pages

| T3 page | MR Moqui screen | Needs |
|---|---|---|
| General | `Settings/General.xml` | AgentCodeUse (user part), AgentCodeAdmin (server part) |
| Chat | `Settings/Chat.xml` | AgentCodeUse |
| Appearance (themes, custom CSS, VS Code / Open VSX theme import, background image, environment themes, publishing) | `Settings/Appearance.xml` + theme editor `.qvue`. Themes map to MR Moqui theme rows. Custom CSS is per user and sanitized (no `@import`, no `url()` to other origins). | AgentCodeUse |
| Keybindings | `Settings/Keybindings.xml` + `.qvue` editor | AgentCodeUse |
| Providers | `Settings/Providers.xml` | AgentCodeAdmin; users see their own sign-ins |
| Threads, Worktrees, Projects, Diffs | XML forms | AgentCodeUse; server parts AgentCodeAdmin |
| Source control, Integrations | `Settings/SourceControl.xml` | AgentCodeUse (own accounts) |
| Browser | `Settings/Browser.xml` | AgentCodeUse |
| Devices | `Settings/Devices.xml` | AgentCodeHost |
| Voice | `Settings/Voice.xml` | AgentCodeUse; models AgentCodeAdmin |
| Secrets | `Settings/Secrets.xml` | AgentCodeUse (own), AgentCodeAdmin (environment) |
| Scheduled tasks | `Settings/ScheduledTasks.xml` | AgentCodeUse |
| Connections (environment map) | `Settings/Connections.xml` + map `.qvue` (shows at 768 px and up; a list below) | AgentCodeUse; linking AgentCodeAdmin |
| Access (paired clients, pairing links) | `Settings/Access.xml` | AgentCodeUse (own devices), AgentCodeAdmin (all) |
| Storage, Maintenance, Diagnostics | XML | AgentCodeAdmin |
| Labs, Beta | XML | AgentCodeAdmin |
| Snap Shot | desktop only | AgentCodeUse |
| Archived | XML list | AgentCodeUse |
| About, Open source licenses | XML; licenses list includes the T3 MIT notice | all |

- A settings search box searches names and help texts of all pages (`settingsSearch` port).
- Each page shows project overrides inline where a project is selected.
- tui: every settings page is an XML form, so tui gets them free. The `.qvue` editors have XML fallbacks (raw JSON
  field for keybindings, list for the map).

## 10. Responsive layout

| Width | Rule |
|---|---|
| ≤ 767 px | Sidebar is a sheet. |
| ≤ 980 px | Right panel is a sheet, not inline. |
| < 640 px | Phone rules in the thread view: compact header, composer controls in one menu, one pane at a time in the terminal (as MR Moqui terminal below 600 px). |
| ≥ 768 px | Connections map shows. |

- Breakpoints sm 640, md 768, lg 1024, xl 1280 (Quasar's are close; use these values in the Agent Code CSS).
- Coarse pointers change composer, model picker and row behaviour (bigger targets, long-press menus).
- Reduced motion is kept. Swipe for history on touch.
- The `/qapps` shell rule (header parts merge below 600 px) stays.

## 11. Client stores

| T3 store | MR Moqui (`moqui.code.stores`) | Persisted where |
|---|---|---|
| composerDraftStore | `drafts` (per thread) | IndexedDB / app store |
| promptStashStore | `stash` | `UserPreference` |
| rightPanelStore, closedViewStore | `rightPanel`, `closedViews` | IndexedDB |
| diffPanelStore | `diffPanel` | memory |
| terminalUiStateStore | `terminalLayout` | IndexedDB |
| previewStateStore, previewMiniPlayerStore, browserHistoryStore, browserFaviconStore | `preview*` | IndexedDB |
| threadSelectionStore, uiStateStore, commandPaletteStore, sidebarPendingFileDropStore | same names | memory |
| state/orchestration, threads, shell | `shell`, `threads` from `subscribeShell` / `subscribeThread` | memory; apps also in the offline store ([12 §6](12-Mobile.md#6-offline)) |
| state/environments, primaryEnvironment, peerLinks, relay | `environments` | apps: account store; browser: memory |
| state/pullRequests, issues, review, vcs, git | same names | memory, cache 5 min |
| state/desktop* | host API (`moqui.host.code.*`) | desktop host |

## 12. Accessibility and language

- `screenReaderAnnouncements` (status, messages, everything) through an ARIA live region.
- Every control has a label. Keyboard focus rules of §4.2.
- Text goes through `MoquiL10n` keys. The English texts are T3's where they fit.

## 13. TUI

- `crates/moqui-tui/src/code/`: a Code view for `moqui tui` and the SSH `agent-apps` session.
- Panes: thread list (sections as §4.2), thread view (messages, tool calls as one-line summaries with expand, plans,
  approvals and questions as forms), composer (multi-line, `/` commands, `@` files), status line (run state, model,
  mode, usage).
- Keys: the same command names as §8 where the terminal allows (`ctrl` for `mod`). `F6` cycles panes.
- Streams: the TUI client opens `/agentws` like the web page.
- Diff: unified diff with colors. PR pages and settings render from their XML screens in tui mode.
- Approvals in the TUI: the terminal counts as an interactive decider (as today in `Decider::from_request`).
- The `Workbench.tui.json` twin points the tui renderer at this view, so `/qapps/code` in tui mode opens it.
