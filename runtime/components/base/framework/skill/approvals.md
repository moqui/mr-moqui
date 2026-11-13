---
name: approvals
title: How writes are approved
description: Use when a write waits for the user's approval, or an approval was rejected, edited in the screen or expired.
kind: reference
aliases: [approve, approval, reject, rejected, confirm, pending, write, save, edit in screen]
---
Every write (a `do` that saves, posts, approves, sends or deletes, and a `run_tool` that writes) waits for the user.
The server shows them a card with what changes; you get one of three outcomes:

- **Approved**: the write ran; its result is the tool's answer. Go on with the next step.
- **Edit in screen**: the user opened the screen and saved it themselves. The result has the values they saved; use
  those, not the ones you proposed.
- **Rejected**: nothing changed. The feedback, when there is one, is the only task now: do what it says or ask.

Also:
- One write at a time: send it, wait for its result, then the next.
- `approval pending` with no one at a page (a job or a remote caller): end the turn with one line saying what waits.
- `conflict`: the record changed since the card was made; nothing ran. Read the record again before you propose more.
- An expired approval ran nothing; ask again only if the user still wants it.
- A skill's `risk` (reversible, confirm, irreversible) is shown on the card; it never skips one.
- Never say a write is done until a result shows it.
