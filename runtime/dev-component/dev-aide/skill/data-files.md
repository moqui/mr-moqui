---
name: data-files
title: Data files
description: Use when you add seed or demo records, such as enumerations and status items, in a data file.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [seed data, enumeration, status item, entity-facade-xml]
sections:
  files: Data files and their types
---
## Files
`data/<Name>.xml` holds `<entity-facade-xml type="seed">` with one element per record, named by the entity:
`<moqui.basic.EnumerationType enumTypeId="ThingKind" description="Thing Kind"/>`,
`<moqui.basic.Enumeration enumId="TkBig" enumTypeId="ThingKind" description="Big"/>`. Seed loads with every load,
`demo` only with demo data. A data file is loaded, not reloaded on save.
