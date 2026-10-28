---
name: add-field-and-show-it
title: Add a field to this form
description: Use when the user wants a new field on a record and on its screens: extend the entity and the form from the site.
kind: procedure
screens: [/apps]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [add field, new field, custom field, show field]
risk: confirm
---
Adds a field to an entity and shows it on a form, both as extensions in the site component.

## Steps
1. `artifact` find the entity of the page's record (or the one the user names) and `describe` it: its fields, file and
   side. Ask the name, the kind (text, number, date, yes/no, picklist) and the label when the user did not say.
2. `artifact` refs of the entity, kinds `form`: the forms that show it. On an app page, the page's screen's form first.
   `source` the form's element to find the field to place it after.
3. `propose_edit` one change set in the site component: an `extend-entity` with the `field` in its entity file
   (`component://<site>/entity/<Site>Entities.xml`), and a `screen-extend` at
   `component://<site>/screen-extend/<component>/screen/<path of the screen>` with the form by its name and the
   `field` with `after` and a `default-field title`.
4. Tell the user the entity change takes a restart (the banner's Restart) before the field holds data.
