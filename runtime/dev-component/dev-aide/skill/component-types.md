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
- App: a root under apps with its menu, artifact group, entity file and screens, and a REST API; Home's New component
  (or ./moq component-create) makes one from the component template, runtime/dev-component/template. Kind "Screens
  only" leaves out the REST API, "REST API only" the screens; "Dev only" puts it in runtime/dev-component.
- Data only, native (Rust), theme, skill pack (skills and prompts), dev tool (in runtime/dev-component): start from an
  app and remove what it does not need.
A new component loads after a restart.
