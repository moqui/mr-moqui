---
name: add-related-section
title: Add a related list to this page
description: Use when the user wants the related records of this record shown on its detail page, from the site component.
kind: procedure
screens: [/apps]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [related list, section, tab, related records]
risk: confirm
---
## Steps
1. `artifact` describe the page's entity and `refs` in, kinds entity: the entities that point to it.
2. Ask which related records to show when there are several.
3. `propose_edit` a `screen-extend` with a `widgets-extend name="<a named container>" where="append"` holding a
   `section` with an `entity-find` in its actions and a `form-list`, or a new subscreen in the site component.
