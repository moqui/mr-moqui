---
name: list-and-detail
title: Make a list and a detail for an entity
description: Use when the user wants new screens to find, list, view and edit the records of an entity.
kind: procedure
screens: [/apps/dev]
requires: [screen-xml]
agents: [dev]
surfaces: [drawer]
aliases: [new screen, crud, list screen, detail screen]
risk: confirm
---
## Steps
1. `artifact` describe the entity: its fields and key.
2. Write a List screen (a find form with `search-form-inputs`, a form-list linking to the detail) and a Detail screen
   (a form-single with an update transition), as the Gallery's archetypes do; show the list with `write_ui` kind
   `screen-xml`.
3. `propose_edit` both files in the site component, under its app's screen directory.
