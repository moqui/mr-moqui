---
name: site-customizing
title: Customizing a site from its site component
description: Use when you add a field, column, rule or seed data to an upstream app: extend it from the site component.
kind: guide
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [extend, extend-entity, screen-extend, customize, site component, upstream, DbForm, fork]
sections:
  extend-do-not-fork: Why upstream components are extended, not edited
  entities: A field, relationship or index on an upstream entity
  screens: A field, column or widget on an upstream screen
  rules-and-data: Rules on upstream services and entities, and seed data
  limits: What each mechanism can and cannot do
---
A site extends the apps it uses; it does not edit them.

## Extend, do not fork
An edit to an upstream component is a fork that every upgrade fights. Put the change in the site component as an
extension; `check` reports an in-place edit of an upstream file while the lock is on, and `propose_edit` refuses it.

## Entities
Add a field with `extend-entity` in the site component's entity file (`entity/<Site>Entities.xml`):
```
<extend-entity entity-name="Example" package="moqui.example">
    <field name="dueDate" type="date"/>
</extend-entity>
```
The column is added when the server restarts. A picklist field is an `...EnumId` field with a `one` relationship to
`moqui.basic.Enumeration` (title the enumeration type) and its `EnumerationType` and `Enumeration` rows in the site's
seed data file.

## Screens
A `screen-extend` file at `screen-extend/<component>/screen/<path of the base screen>` merges into that screen:
- a form by its name: `<form-single name="ExampleFacts"><field name="dueDate" after="exampleEndDate">...</field></form-single>`;
  `after` places the field in the form's `field-layout`, or a list's column after that field.
- `<widgets-extend name="Box" where="after">` (before, after, prepend, append) puts widgets at a named element.
- a transition by its name replaces it; `actions` add to the screen's actions.
A screen-extend anchors only to a named form, container, section or transition of the base screen.

## Rules and data
A rule on an upstream service is a SECA in the site component (`service/<Site>.secas.xml`); on an entity an EECA in
`entity/<Site>.eecas.xml`. Seed data (enumerations, status items) goes in the site's `data/<Site>SeedData.xml`.

## Limits
- A form merge adds fields, changes a field's widgets, and places them with `after`; it does not remove a field (hide
  it with a `field` whose widget is `ignored`).
- When the base screen has no named element where the change goes, the anchor is added to the base screen as its own
  change (upstream, by a framework developer), never generated.
- DbForm (System, Form Configuration) changes forms in production per user group; it is configuration, not code.
- An entity change needs a restart; a screen, service or rule change is live at once.
