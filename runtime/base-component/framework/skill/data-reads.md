---
name: data-reads
title: Reading rows with data
description: Use when you count, total or list rows of a screen list or a basic table, or read on in a large result.
kind: guide
aliases: [rows, list, table, count, total, artifact]
---
`data` reads rows without opening the screen, as the user may see them. Use a path a tool returned.

## Lists on screens
`data path=/apps/... list=Name` reads one list of a screen; with no list it reads the first and names the others.
The answer starts `Name on /apps/... (showing 25 of 312)`, then a table, then `rows link to ...` when every row opens
the same screen with its id, then `more: data offset=25`. Page on with the same arguments and the offset it gives.

## Where
`where` is an object of field values, by the list's find field names:
- `{"statusId": "Open"}` is equal; `{"statusId": ["Open", "Held"]}` is any of them.
- `{"name": "%acme%"}` contains; `{"name": "acme%"}` begins with.
- `{"orderDate": ">=2026-01-01"}`, `"<=..."`, `">..."`, `"<..."` compare; `{"ownerId": "!=X"}` is not.
- `{"orderDate": {"from": "start of last month", "thru": "end of last month"}}` is a range.
- `{"note": null}` is empty.
A date may be a phrase (`today`, `3 days ago`, `start of last week`, `end of this month`); the answer's
`resolved:` line shows the date used. Not valid: SQL (`status = 'Open'`), `AND`/`OR` text, field names you guessed.

## Basic tables
`data entity=moqui.basic.StatusItem where={"statusTypeId": "..."}` reads the enumerations, status items, units and
geos directly. Other entities are read through a list on a screen; `entity_not_readable` says so.

## Fields and order
`fields` names the columns to show (a column's title or field name); unknown ones are listed as ignored. `order` is a
field, `-field` for descending.

## Artifacts and grep
A result past the size limit ends `artifact A12 (312 rows, 48,210 chars): data artifact=A12 offset=40, or grep=text`.
`data artifact=A12 offset=40` reads on from that line; `data artifact=A12 grep=Acme` gives the lines with Acme,
numbered. Read only what the answer needs.
