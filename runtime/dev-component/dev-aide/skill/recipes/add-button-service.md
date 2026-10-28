---
name: add-button-service
title: Add a button here
description: Use when the user wants a button on a page that runs a service on the page's record, from the site component.
kind: procedure
screens: [/apps]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [button, action, run service]
risk: confirm
---
## Steps
1. `artifact` find the service (or write one in the site component's service file first).
2. `try_service` it in the sim with the page's record to show what it does.
3. `propose_edit` a `screen-extend` with a `transition` calling the service and a `widgets-extend` adding a `link` to
   it at a named container of the page.
