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
---
## Files
A `.qvue` screen is its XML (parameters, transitions, actions, `render-modes="qvue"` and a `render-mode text` naming the
`.qvue`) and the `.qvue` file (`<template>`, `<script>`, `<style>`). Change both through `propose_edit`; a site extends
an upstream screen with `screen-extend`, which XML screens take, not `.qvue` ones.
