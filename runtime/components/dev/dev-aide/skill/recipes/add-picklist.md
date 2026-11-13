---
name: add-picklist
title: Add a picklist
description: Use when the user wants a field chosen from a list of values: an enumeration field with its seed records.
kind: procedure
screens: [/apps, /apps/dev]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [picklist, drop-down, enumeration, choice list]
risk: confirm
---
## Steps
1. Ask the values when the user did not give them.
2. `propose_edit`: in the site's entity file an `extend-entity` with an `...EnumId` field and a `one` relationship to
   `moqui.basic.Enumeration` titled by the type; in the site's seed data file the `EnumerationType` and an `Enumeration`
   per value; on the form a `drop-down` of the type's enumerations (a `screen-extend`).
3. Say the field's column is added on the save, and the seed data loads with Load this file on the seed data file in the
   Workspace (the entity view's own Add field loads it on Save).
