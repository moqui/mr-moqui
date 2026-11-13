---
name: xml-actions
title: XML Actions
description: Use when you write actions: service actions, screen actions, transition actions or ECA rule actions.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [actions, entity-find, service-call, set, iterate, if, script]
sections:
  find: Finding records
  write: Making and writing records
  flow: Calls, conditions and loops
---
## Find
`<entity-find-one entity-name="Thing" value-field="thing"/>` by the context's key fields;
`<entity-find entity-name="Thing" list="thingList"><econdition field-name="statusId" value="X"/><order-by field-name="-createdDate"/></entity-find>`.

## Write
`<entity-make-value entity-name="Thing" value-field="thing" map="context"/>`, `<entity-create value-field="thing"/>`,
`<entity-update value-field="thing"/>`; or a `service-call` of `create#Thing` (entity-auto).

## Flow
`<service-call name="a.B.run#Thing" in-map="context" out-map="result"/>`; `<set field="total" from="a + b"/>` (`from`
is JavaScript); `<if condition="!thing"><return error="true" message="Not found"/></if>`;
`<iterate list="thingList" entry="thing">...</iterate>`; `<script>` holds JavaScript with `ec`.
