---
name: screen-xml
title: XML screens and the screen-xml canvas
description: Use when you write an XML screen or screen-extend, or show one on the canvas with write_ui kind screen-xml.
kind: reference
unlocks: [write_ui]
agents: [dev]
surfaces: [drawer]
aliases: [screen, form-single, form-list, transition, widgets, screen-xml canvas, archetype]
sections:
  screens: A screen's parts
  forms: Forms and lists
  canvas: Showing a screen on the canvas
  archetypes: The style guide's screen archetypes
---
## Screens
`<screen>` with `parameter`, `transition` (a `service-call` and a `default-response url="."`), `actions` and `widgets`.
Subscreens are the screen files in the directory of its name, or `subscreens-item` elements.

## Forms
`<form-single name="EditThing" transition="updateThing" map="thing">` with `field` elements, each with a
`default-field title=""` and a widget (`text-line`, `drop-down`, `date-time`, `display`, `hidden`, `submit`);
`field-layout` with `field-ref` places them. `<form-list name="ThingList" list="thingList">` with columns from its
fields; `search-form-inputs` lists filter from a find form.

## Canvas
`write_ui` with `kind: "screen-xml"` shows the page of a screen with your program standing at its location; nothing is
saved. The program starts with one line naming both:
`<!-- screen-xml at="component://my-site/screen-extend/example/screen/Gallery/ExampleDetail.xml" path="/qapps/example/Gallery/ExampleDetail?exampleId=100" -->`
then the screen or screen-extend as it would be saved. It is checked like a file; to save it, `propose_edit` it.

## Archetypes
Landing, List, Detail, 360, Task, Board, Calendar, Split, Settings, Wizard, Dialog, Report, Editor, Panel and Tree list,
as the example component's Gallery shows them (`component://example/screen/Gallery/`).
