---
name: vue-sfc
title: Vue SFC canvases
description: Use when a generated view needs a layout or behavior OpenUI Lang cannot express and this agent may write a Vue single-file component.
kind: guide
unlocks: [write_ui]
aliases: [vue, vue-sfc, sfc, single-file component, vue component]
surfaces: [drawer]
locked: true
version: 1
sections:
  shape: The blocks of a program, the script and the template
  sandbox: What the frame does not have
  bridge: Reads, writes with actions, links
  widgets: The Quasar components and the widgets
  example: A whole program on a framework screen
---
A Vue SFC canvas is the escape hatch of the canvases: use it only when OpenUI Lang (skill openui-lang) cannot express
the view, and prefer a real screen first. Call write_ui with kind vue-sfc, a short title and program, the whole
component. It runs in a sandboxed frame beside the drawer that has no page, session, cookies, storage, network or
navigation of its own; everything it needs comes through its values, its events and this.$bridge. The server checks
the program before anything is shown; an error names the section of this skill and quotes its rule: fix it and send it
again. To change a canvas, send patch with only the blocks that change (a whole <script>, say).

The rules broken most often:

1. Vue 3, Options API, module.exports = { ... }: no export default, no <script setup>, no import.
2. No window, document, location, fetch or timers that load: read with this.$bridge.get.
3. A write is an <actions> entry, run by this.$bridge.submit(name, values) from a click; the user confirms it.
4. Links are /apps paths; widgets are q-* and the six m- widgets of section widgets.

## Shape

A program is one Vue 3 single-file component: a <template>, a <script> that sets module.exports = { ... }, an optional <style> and an optional <actions> block, and nothing else.

- The script is plain JavaScript that sets module.exports to the Options API object: no import, no export default, no <script setup> and no TypeScript.
- The template is Vue 3 template syntax: every tag closes and every attribute value is quoted.
- Props: values (the canvas's values as the page keeps them). Emit `this.$emit('input', {name, value})` or an object of
  values to change them; the page keeps them for a reload and for a submit.
- Vue 3 names: v-model on a component is modelValue and update:modelValue; q-input and q-select take v-model.

## Sandbox

The component has no page, no network and no navigation: never use window, document, location, fetch, XMLHttpRequest, WebSocket, eval, Function or import; read with this.$bridge.get and write with this.$bridge.submit.

- The template has no <script>, <style>, <iframe>, <object>, <embed>, <form>, <base>, <meta> or <link>, no v-html and no javascript: URL.
- A <style> styles the component only: no @import, and url() only of a data: image.
- Also never use self, top, parent, RTCPeerConnection or WebRTC. These checks are defence in depth; the real boundary
  is the sandboxed frame, so do not rely on them and do not try to work around them.
- Images are data: URLs or this site's static files (/images/…); another site's only when the user allows external
  content, and while that is on the canvas cannot read screen data (so an image cannot carry a read out). A frame that
  navigates itself is closed and the attempt recorded; a frame that hangs, or floods the page with messages, is closed too.

## Bridge

Read with this.$bridge.get(path, query, list): path is an /apps screen with a list, or a transition that answers JSON, and the Promise gives its JSON.

- Every path this.$bridge.get reads is declared in the <reads> block, a JSON list of /apps screen paths; a path not listed is refused.
- A screen list answers `{rows, totalCount, list, columns: [{key, title}]}`, each row keyed by the field names data
  shows as row keys, with `_id` and `_link`. query is a flat object of strings; list names one when the screen has
  several. A read that would write is refused. Reads are limited to a few at once and about a minute's budget.
- A write is an action of the <actions> block: this.$bridge.submit(name, values) from the user's click asks the user to confirm it, and the server runs it.
- <actions> is a JSON list of {name, label, path, fields, parameters}: path is a transition of an /apps screen, fields the values a submit sends, parameters fixed values, and no secret.
- A submit that is not from a click is ignored; one every two seconds at most. The Promise resolves with
  `{status: "saved"}`, `"canceled"`, `"failed"` (with `error`) or `"ignored"`.
- A link is <a href="/apps/..."> to a screen of the apps, which opens beside the canvas; another site opens only when the user allows external content.

## Widgets

Use Quasar components (q-*) and the widgets listed here: m-text-line, m-drop-down, m-date-time, m-display, m-date-period and m-container-box; no other m- component and no router-link.

- Quasar: q-btn, q-input, q-select, q-table, q-card, q-card-section, q-list, q-item, q-item-section, q-checkbox,
  q-toggle, q-radio, q-banner, q-tabs, q-tab, q-tab-panels, q-tab-panel, q-chip, q-icon, q-separator, q-space,
  q-tooltip, q-date, q-time, q-popup-proxy, q-markup-table. Convention: dense outlined stack-label.
- m-text-line: v-model, label. m-display: value, label.
- m-drop-down: v-model, label, :options="[{value, label}]", or options-url (an /apps transition that answers options)
  with value-field, label-field and :server-search="true".
- m-date-time: v-model, type date, time or date-time (YYYY-MM-DD, HH:mm, YYYY-MM-DD HH:mm), label.
- m-date-period: :fields="values" and name; it sets name_period, name_poffset, name_pdate, name_from and name_thru.
- m-container-box: title, a card section with a slot.

## Example

A list of user groups with a description to save, on the framework's own screens:

```
<template>
  <m-container-box title="User groups">
    <q-table :rows="rows" :columns="columns" row-key="_id" dense flat></q-table>
    <m-text-line label="Description of ADMIN" v-model="description"></m-text-line>
    <q-btn color="primary" label="Save" @click="save"></q-btn>
    <a href="/apps/system/Security/UserGroup/UserGroupList">All groups</a>
  </m-container-box>
</template>

<script>
module.exports = {
  props: { values: Object },
  data: function () {
    return { rows: [], description: (this.values && this.values.description) || '',
      columns: [{ name: 'g', label: 'Group', field: 'userGroupId' }, { name: 'd', label: 'Description', field: 'description' }] };
  },
  mounted: function () {
    var vm = this;
    this.$bridge.get('/apps/system/Security/UserGroup/UserGroupList', { pageSize: '20' })
      .then(function (r) { vm.rows = r.rows || []; });
  },
  methods: {
    save: function () {
      this.$bridge.submit('save', { description: this.description });
    }
  }
};
</script>

<actions>
[{"name": "save", "label": "Save the description", "path": "/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup",
  "fields": ["description"], "parameters": {"userGroupId": "ADMIN"}}]
</actions>

<reads>
["/apps/system/Security/UserGroup/UserGroupList"]
</reads>
```
