---
name: add-list-column
title: Add a column to this list
description: Use when the user wants another column in a list (form-list) of an app screen, from the site component.
kind: procedure
screens: [/apps]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [add column, list column, show in list]
risk: confirm
---
Adds a column to a form-list through a `screen-extend` of the site component.

## Steps
1. `artifact` find the screen of the page and its forms (`describe`: parts of kind form); pick the form-list.
2. `source` the form-list to see its fields and the entity it lists; check the field exists on that entity (or is a
   field the site added).
3. `propose_edit` a `screen-extend` at `component://<site>/screen-extend/<component>/screen/<path>` with
   `<form-list name="<the list>"><field name="<field>" after="<a column>"><default-field title="..."><display/></default-field></field></form-list>`.
