---
name: entity-xml
title: Entity definitions
description: Use when you write or change an entity, view entity or extend-entity in an entity XML file.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [entity, field, relationship, view-entity, key-map, field type]
sections:
  entity: An entity and its fields
  relationships: Relationships and key maps
  view-entities: View entities
---
## Entity
`<entity entity-name="Thing" package="my.site">` with `<field name="thingId" type="id" is-pk="true"/>` and other
fields. Types: `id`, `id-long`, `text-indicator` (Y/N), `text-short`, `text-medium`, `text-long`, `text-very-long`,
`date`, `time`, `date-time`, `number-integer`, `number-decimal`, `number-float`, `currency-amount`, `currency-precise`.
A field has `not-null`, `default`, `encrypt`, `enable-audit-log` and a `description` child.

## Relationships
`<relationship type="one" related="moqui.basic.StatusItem"/>`: the key fields named as the related entity's primary
key; `title` distinguishes two to one entity; a `key-map field-name="x" related="y"` maps other names. `one-nofk` adds
no foreign key; `many` points back.

## View entities
`<view-entity entity-name="ThingAndStatus" package="my.site">` with `member-entity entity-alias="T" entity-name=".."`
and `member-entity ... join-from-alias="T"` with `key-map`, `alias-all entity-alias="T"`, `alias name="" field=""`.
