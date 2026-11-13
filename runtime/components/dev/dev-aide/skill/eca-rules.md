---
name: eca-rules
title: ECA rules
description: Use when you add a rule that runs on a service call (SECA) or an entity write (EECA).
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [seca, eeca, rule, trigger, post-service]
sections:
  seca: Rules on services
  eeca: Rules on entities
---
## SECA
In `service/<name>.secas.xml`: `<seca id="MySite_ThingDone" service="a.B.update#Thing" when="post-service">` with a
`condition` and `actions`; `when` is pre-validate, pre-service, post-service, tx-commit and the like. An `id` replaces
an earlier rule of that id.

## EECA
In `entity/<name>.eecas.xml`: `<eeca id="" entity="my.site.Thing" on-create="true" on-update="true">` with `actions`.
