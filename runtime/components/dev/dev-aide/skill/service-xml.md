---
name: service-xml
title: Service definitions
description: Use when you write or change a service definition: its parameters, type and implementation.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [service, in-parameters, out-parameters, entity-auto, script service, implements]
sections:
  services: A service and its parameters
  implementations: Inline actions, scripts, entity-auto and rust
---
## Services
In `service/<path>.xml`: `<service verb="update" noun="Thing">`, named `<path>.update#Thing`. `in-parameters` and
`out-parameters` hold `parameter name="" type="" required="true"` (types String, Integer, BigDecimal, Timestamp, Date,
Boolean, List, Map); `auto-parameters entity-name="" include="pk|nonpk|all"` adds an entity's fields.

## Implementations
- inline: `<actions>` of XML Actions (skill `xml-actions`).
- `type="script" location="component://.../Thing.js"`: JavaScript with `ec` and the in-parameters, setting the out ones.
- `type="entity-auto"` with verb create, update, store or delete on the noun's entity; or call `create#my.site.Thing`
  without a definition.
- `type="interface"` for `implements service=""`.
