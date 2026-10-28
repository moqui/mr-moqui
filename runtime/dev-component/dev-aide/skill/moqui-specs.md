---
name: moqui-specs
title: Component specs
description: Use when you write or run a spec of a component's services or screens with moqui test.
kind: reference
agents: [dev]
surfaces: [drawer, chat, tui]
aliases: [spec, test, checkData, moqui test]
sections:
  specs: A spec file
---
## Specs
`test/<Name>.spec.js` of the component: `spec({ setupSpec() { login("john.doe") } })` then `feature("what it does",
{ expect() { const out = ec.service.sync().name("a.B.run#Thing").parameters({...}).call(); out.result == "x" } })`;
each expression line is checked. `checkData(text)` compares records written. Run it with `./moq test <component>
--spec <Name>`.
