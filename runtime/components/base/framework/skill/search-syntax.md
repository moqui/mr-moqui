---
name: search-syntax
title: Search words, ids and groups
description: Use when a search finds nothing or too much, or you look for a record by its id, a person, a screen or an action.
kind: reference
aliases: [search, palette, id, lookup, groups]
---
`search` finds what the search box finds, as the user: records, people, messages, pages, events, saved finds and,
from the menus, Screens and Actions. It answers one line a hit, at most 5 a group, and a `more:` line.

- Give a few plain words or one id: `search text="john doe"`, `search text="EX_JOHN_DOE"`. Not a sentence, no
  operators, no quotes inside.
- An id is matched exactly by the groups whose ids look like it; a word matches titles and names, and its plural or
  `-ing` form finds the same.
- `groups=["Screens"]` (or Actions, or a group a result named) narrows to those groups; `limit` up to 10 shows more.
  The search box's typed prefixes are groups too: use `groups` instead.
- Budget: one search, then open the best hit with `screen go=` and its path. No hit: try one other word (a synonym, a
  shorter name) once, then say it is not found or ask.
- A screen hit is a path to `go` to; a record hit's path opens that record.
