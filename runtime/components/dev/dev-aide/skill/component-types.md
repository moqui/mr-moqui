---
name: component-types
title: Kinds of component
description: Use when the user wants a new component: a site, an app, a REST API, data only, native, a theme, a skill pack or a dev tool.
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
  (or ./moq component-create) makes one from the component template, runtime/components/dev/template. Kind "Screens
  only" leaves out the REST API, "REST API only" the screens; "Dev only" puts it in runtime/components/dev.
- The other kinds, from the template's kinds/ (Home's New component, or ./moq component-create --kind):
  - Data only (data): seed and demo data files, no screens, services or code.
  - Native (native): Cargo.toml and src/lib.rs with submit_component! and a type="rust" service; ./moq build links it.
  - Theme (theme): a ScreenTheme of the /qapps shell on the Default theme, its style sheet and a color scheme.
  - Skill pack (skills): a skill, a suggested prompt and retrieval cases, no code.
  - Dev tool (dev-tool): mock services and a test data screen, purpose="dev", always in runtime/components/dev.
- Each may start its git repository with a remote as origin (Git remote): https with no user name or token, ssh://,
  git:// or git@host:path.
A new component loads after a restart; a native one after a rebuild (./moq build, the banner's Rebuild) first.
