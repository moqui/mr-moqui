---
name: add-filter
title: Add a filter to this list
description: Use when the user wants to filter a list by another field: add it to the list's find form from the site.
kind: procedure
screens: [/apps]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [filter, search field, find by]
risk: confirm
---
## Steps
1. `artifact` and `source`: the list and its find form; a list filters by a field when its find uses
   `search-form-inputs` (say why not otherwise).
2. `propose_edit` a `screen-extend` adding the field to the find form (`form-single` by its name), with `after`.
