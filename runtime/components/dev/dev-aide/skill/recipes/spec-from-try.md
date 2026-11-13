---
name: spec-from-try
title: Write a spec from my last Try
description: Use when the user wants a spec of a service from what it did when run in the sim.
kind: procedure
screens: [/apps/dev]
requires: [moqui-specs]
agents: [dev]
surfaces: [drawer]
aliases: [spec, test, try, checkData]
risk: reversible
---
## Steps
1. `try_service` the service with the parameters (the user's last Try's, or ask them).
2. Write a feature that calls it with those parameters, checks the out-parameters, and `checkData` of the rows the sim
   wrote (trimmed to the fields that matter).
3. `propose_edit` it into `component://<component>/test/<Service>.spec.js` (append a feature when the file is there).
4. Tell the user the service view's Try panel does the same by hand: Save as spec after a Try in sim, then Run spec runs
   the file in the sim, so it writes nothing.
