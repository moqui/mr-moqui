---
name: add-status-flow
title: Add a status flow
description: Use when the user wants records to move through statuses: a status field, its items and the allowed moves.
kind: procedure
screens: [/apps, /apps/dev]
requires: [site-customizing]
agents: [dev]
surfaces: [drawer]
aliases: [status, workflow, state machine]
risk: confirm
---
## Steps
1. Ask the statuses and which moves are allowed.
2. `propose_edit`: a `statusId` field with a `one` relationship to `moqui.basic.StatusItem` (an `extend-entity` for an
   upstream entity), and in the site's seed data the `StatusType`, a `StatusItem` per status and a
   `StatusFlowTransition` per move.
