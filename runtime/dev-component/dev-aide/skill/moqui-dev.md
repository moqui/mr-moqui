---
name: moqui-dev
title: Changing Moqui components with the dev tools
description: Use when you change or extend a Moqui component's files with propose_edit, or need how a save reloads.
kind: guide
unlocks: [propose_edit]
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [propose_edit, change set, component, dev assist, artifact, reload]
sections:
  components: Components, the site component and upstream components
  artifacts: The kinds of artifact files and where they live
  change-sets: How propose_edit takes a change set
  reloading: What a save reloads, and when a restart is needed
---
The dev tools change the files of the components this dev instance runs. The user accepts each change set you propose.

## Components
A component is a directory with `component.xml`. The site component (Settings names it; Home creates one) holds what
this site adds and changes. Every other component is upstream: the framework, the apps, the libraries. The `artifact`
and `source` tools say `side: site` or `side: upstream` for each artifact and file.

## Artifacts
- `entity/*.xml`: entities and view entities; `extend-entity` adds fields, relationships and indexes to another's.
- `entity/*.eecas.xml`: EECA rules. `service/**.xml`: services, by their path (`service/a/B.xml` is `a.B.verb#Noun`).
- `service/*.secas.xml`: SECA rules; `service/*.rest.xml`: REST resources.
- `screen/**.xml`: screens, mounted by their directory; `screen-extend/<component>/screen/<path>.xml`: an extension of
  that screen of another component.
- `data/*.xml`: data files (`type="seed"` loads with every load; `demo` only with demo data).

## Change sets
`propose_edit` takes `summary` and `files`, each `{file, ops | edits | text}`:
- `ops`: XML operations of the lossless layer: `{op: "insert", at: "entities/extend-entity", position: "last", xml: "<field .../>"}`,
  `set-attribute`, `remove-attribute`, `rename-attribute`, `remove`, `move`, `replace`, `set-text`, `wrap`, `unwrap`.
  `at` is a selector of element names with `[attr=value]` (`services/service[verb=run]`) or a path of child indexes.
  Nothing else in the file changes, so its diff is the change.
- `edits`: exact replacements `{old, new}`; `old` must be in the file once (give more of it when it is not).
- `text`: the whole text, for a new file.
Name a file as `component://<component>/<path>`. Each file is checked before the card shows; a problem comes back with
its rule and skill section and nothing is proposed.

## Reloading
A service, SECA, EECA, screen, screen-extend, REST or skill file is live a moment after it is saved. An entity change
needs a restart, which the user starts from the banner; the restart adds the new columns. A data file is loaded, not
reloaded.
