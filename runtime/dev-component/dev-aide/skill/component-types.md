---
name: component-types
title: Kinds of component
description: Use when the user wants a new component: a site, an app, a REST API, data only, native or a skill pack.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [new component, site component, app component]
sections:
  kinds: The kinds and what each starts with
---
## Kinds
- Site: what one site adds and changes (extend-entity, screen-extend, seed data, rules); Home creates one.
- App: a root under apps with its menu, artifact group, entity file and screens.
- REST API, data only, native (Rust), theme, skill pack (skills and prompts), dev tool (in runtime/dev-component).
A new component loads after a restart.
