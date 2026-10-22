# Proposal: TUI Render Mode for XML Screens (mr-moqui + Revue)

Oct 18, 2026 · @David

Archived Oct 21, 2026: built through [Screen-TUI-Impl](Screen-TUI-Impl.md) Phases 1 to 4, with the findings there; the
user guide is [TUI](../../docs/TUI.md). The browser terminal and the other open parts are in
[Remaining-Work](../Remaining-Work.md).

## Summary

Add a `tui` render mode to mr-moqui that turns XML Screens into a structured screen model, and a Revue-based terminal client that renders that model as a full-screen app with forms, tables and navigation. The same client serves three entry points: a `moqui tui` subcommand on the local binary, an `apps` command in the SSH shell, and a thin remote client pointed at any mr-moqui server.

The pattern mirrors what Moqui already does for its Vue apps: the server renders screens into something a client-side runtime understands, and the client handles interaction. Here the runtime is Revue, a Vue-style TUI framework, so the mental model stays the same across web and terminal.

The existing `text` render mode stays as is. It is for column-printed output; `tui` is for interactive use.

*Style Note*: This should borrow from the UI design of the old school green screen Cobol/RPG/etc business applications with no-nonsense layouts, keyboard shortcuts, and full use and navigability by keyboard. It should also have flavors of the Lynx browser added for links and other browser-based elements. That said, we lean mostly on what Revue offers.

## Why Revue

Revue is the only Rust TUI framework found that ships forms, tables, routing and styling together, which covers most of what Moqui screens need without assembling a widget stack by hand.

- **Familiar model.** Signal / Computed / Effect reactive state and CSS stylesheets with variables and selectors. Anyone who knows the Vue side of Moqui can read it.
- **Forms built in.** `form` and `form_field` with declarative validation (required, length, matches) that re-validates as values change. Inputs include `input`, `textarea`, `select`, `checkbox`, `radio`, `switch`, `number_input`.
- **Screens and navigation.** `table`, `tabs`, `tree`, `modal`, `toast`, `command_palette`, plus routing, a central store and a query DSL for filtering and sorting.
- **Background work.** A worker pool keeps server calls (transitions, autocomplete, paging) off the UI thread.
- **Tooling.** Widget inspector, profiler and snapshot testing; CSS hot reload for theming work.
- **Fit with the binary.** Pure Rust, MIT licensed, crossterm-based, optional `tracing` integration.

The alternative, kept as a fallback, is Ratatui with the rat-salsa widget set (masked inputs, date and number inputs, a large-data table). It is lower level but more established. Keeping the screen model renderer-agnostic (next section) preserves that option.

## Architecture

The server renders screens into a model; the client turns the model into widgets and sends user actions back as transition calls.

&#91;embedded content: tui architecture · runtime, client, four channels\]

The `tui` render mode evaluates conditions, permissions and data on the server and emits a JSON screen model: widget tree, field definitions, current values, options and available actions. The Revue client interprets that model, so it never parses XML Screens and never needs updating when a screen changes. In local and SSH modes the model passes in-process; in remote mode it travels over HTTP.

## Mapping XML Screen elements to Revue

Most screen elements have a direct Revue counterpart; the gaps are dates, inline-editable lists and Moqui's field layouts, which need small custom widgets or layout code.

| XML Screen element | Revue rendering | Notes |
| --- | --- | --- |
| `subscreens` / app menu | Sidebar `list` or `tabs` + routing | `command_palette` for jump-to-screen by name |
| `form-single` | `form` + `form_field` | Validation rules come from field and entity definitions in the model |
| `field-layout`, `field-row`, `field-col-row` | `grid` / `hstack` | Custom mapping; collapse to one column on narrow terminals |
| `text-line` | `input` | `ac-transition` adds a popover of suggestions fetched by a worker |
| `text-area` | `textarea` |  |
| `drop-down` (static, entity, `dynamic-options`) | `select` | Needs type-to-filter for long lists; `depends-on` re-fetches options |
| `check`, `radio` | `checkbox`, `radio` |  |
| `date-time`, `date-find` | `input` with format check | Gap: no dedicated date picker confirmed yet |
| `display`, `display-entity`, `label` | `text` |  |
| `submit`, `link` | button or key binding | Transitions called through the client's server channel |
| `form-list` | `table` | Server-side paging and sorting; `header-field` find options as a filter row |
| Editable `form-list` (multi) | `table` + row edit `modal` | Gap: inline cell editing, start with a row edit dialog |
| `container-dialog`, `dynamic-dialog` | `modal` |  |
| `container-box`, `container` | bordered `vstack` |  |
| Messages and errors | `toast` / `alert` |  |
| `section` conditions, `section-iterate` | Resolved server-side | The client never sees the condition, only the result |

## Channels and clients

One client codebase, five ways to reach it. The local subcommand and the SSH `apps` command are the core; the rest come nearly free once the screen model travels over HTTP.

| Channel | How it runs | Server link | Main use | Effort |
| --- | --- | --- | --- | --- |
| `moqui tui` (local) | Same binary starts the runtime in the background, client in the foreground | In-process calls | Dev, single-user and on-prem installs; log pane included | Low |
| SSH shell `apps` | Server spawns the client on a pseudo-terminal and bridges it to the SSH channel | In-process | Admin and ops access with nothing installed on the user's side | Medium |
| Remote client (`moqui tui --server <url>`) | Client only, no local runtime | HTTP + auth to the `tui` render mode | Power users and support staff against shared servers | Medium |
| Browser terminal | xterm.js page over WebSocket to a server-side pseudo-terminal | In-process | Demos, locked-down machines, a "terminal" link inside the web apps | Medium |
| Headless | No terminal; render to snapshots | In-process | Screen regression tests in CI | Low |

**Why a pseudo-terminal for SSH.** Revue drives the terminal through crossterm, which expects a real TTY. Spawning the client on a server-side pseudo-terminal (for example with `portable-pty`) and piping its bytes to the SSH channel avoids needing a custom Revue backend. Resize events forward to the pseudo-terminal. If Revue later gains a pluggable writer and input source, the client can attach to the channel directly.

**Log pane.** In local mode the client registers a `tracing` subscriber layer that feeds a scrollable pane, toggled with a key. Over SSH and remote, logs stay server-side unless the user's permissions allow streaming them.

**Side benefit.** The structured screen model is also a clean, permission-filtered view of any screen for automation and AI agents, not just terminals.

## Risks and open questions

The biggest risk is Revue's pace of change, not its capability; the design contains it by keeping Revue behind one adapter layer.

- **API churn.** Revue 3.0.0 shipped on 2026-10-06 and 3.10.1 followed on 2026-10-15, with a migration guide from 2.x. Pin an exact version and upgrade deliberately.
- **Maintainer depth.** One owner on crates.io. Mitigation: renderer-agnostic screen model, so a Ratatui + rat-salsa client remains possible without touching the server side.
- **Toolchain.** Revue 3.x needs Rust 1.95 or newer. Confirm it matches mr-moqui's toolchain.
- **Terminal I/O.** Crossterm-only today, so SSH and browser channels rely on the pseudo-terminal bridge. Open question: is a pluggable backend on Revue's roadmap?
- **Widget gaps.** Date picker and inline table editing are unconfirmed. Open question: build them as local custom widgets or contribute upstream?
- **Large lists.** Open question: does Revue's `table` render only visible rows? If not, server-side paging must keep pages small.
- **Security over SSH.** Each session must run as the authenticated Moqui user with normal artifact authz; the client process gets no extra rights.
- **Keyboard conventions.** Tab order, submit key, escape behavior and screen jumping need one documented scheme before many screens are built.

## Phased plan

Four phases, each ending in something usable; no dates set yet.

1. **Spike.** Pin a Revue version. Hand-build one find screen and one edit screen against a stub model. Confirm the pseudo-terminal bridge works over the SSH shell. Exit: go or no-go on Revue.
2. **Core.** Define the screen model and the `tui` render mode for menus, `form-single`, `form-list` with paging, and the basic field types. Ship `moqui tui` with the log pane. Exit: browse and edit records in one real app.
3. **Shell and dynamics.** Add `apps` to the SSH shell. Add `dynamic-options`, `depends-on`, autocomplete, dialogs and messages. Exit: a full admin task done over SSH.
4. **Reach and polish.** Remote client mode, CSS themes, the documented keyboard scheme, snapshot tests in CI. Browser terminal if there is demand.

## Sources

- [Revue 3.10.1 on docs.rs](https://docs.rs/crate/revue/latest)
- [Revue repository](https://github.com/hawk90/revue)
- [rat-salsa (fallback widget set)](https://github.com/thscharler/rat-salsa)
- [Ratatui 0.30 highlights](https://ratatui.rs/highlights/v030/)
