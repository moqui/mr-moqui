---
name: qvue-screens
title: Vue screens (.qvue)
description: Use when you read or change a .qvue screen: an XML screen whose page is a Vue single-file component.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [qvue, vue screen, sfc]
sections:
  files: The two files of a .qvue screen
  canvas: Previewing a .qvue screen with the qvue-screen canvas
  bridge: Reading and writing from a .qvue screen with $bridge
---
## Files
A `.qvue` screen is its XML (parameters, transitions, actions, `render-modes="qvue"` and a `render-mode text` naming the
`.qvue`) and the `.qvue` file (`<template>`, `<script>`, `<style>`). Change both through `propose_edit`; a site extends
an upstream screen with `screen-extend`, which XML screens take, not `.qvue` ones.

## Canvas
To show a new or changed `.qvue` screen before it is saved, call `write_ui` with the `qvue-screen` dialect. The program
is one text: the header `<!-- qvue-screen at="component://<component>/<path of the screen's .xml>" path="/qapps/<the
page that shows it>" -->`, then the XML screen as it would be saved, then the SFC blocks of its `.qvue` (`<template>`,
`<script>` with `module.exports = { ... }`, and `<style>`, `<actions>` or `<reads>` when it has them, as a Vue SFC
canvas has). The XML renders the `.qvue` beside it: `render-modes="qvue"` and `<widgets><render-mode><text type="qvue"
location="component://<component>/<path>.qvue" template="false"/></render-mode></widgets>`. The SFC runs in the Vue SFC
sandbox, so the rules of the `vue-sfc` skill hold for it. Nothing is saved by the canvas: when the user wants it, propose
both files with `propose_edit`.

## Bridge
A `.qvue` component reads and writes through `$bridge`, the same in the canvas and in the saved screen.
`this.$bridge.get(path, query)` reads a transition that answers JSON: a name alone (`items`) is a transition of the
screen itself, a path starting with `/` any screen's (`/apps/<screen>/<transition>`). In the canvas it is answered by the
program's own transitions under the read fence, and a read of another screen must be in its `<reads>` block.
`this.$bridge.submit(name, values)` posts to the screen's transition `name`; in the canvas it is the `<actions>` entry of
that name, which the user confirms. Name each action as its transition, so the same code works in both places.

