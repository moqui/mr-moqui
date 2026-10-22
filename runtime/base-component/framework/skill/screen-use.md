---
name: screen-use
title: Working on screens
description: Use when you fill a form, pick from a drop-down, open a dialog, filter or page a list, or a screen answer is an error.
kind: guide
aliases: [form, field, dialog, drop-down, dropdown, button, menu, filter, page, element]
sections:
  forms: Setting fields, drop-downs by label or value, dependent fields
  lists: Finding rows, filtering a list, paging, opening a row
  dialogs: Dialogs, confirms and do Yes
  errors: What an error answer means and what to do next
---
`screen` works the apps as the user does. Steps run in order: `go`, `set`, `find`, `do`; with none it looks again.
The answer is the screen as text: `path`, `title`, `focus`, `overlay`, `message`, the screen with the focused element
in `[[ ]]`, then `--- ok` or `--- error` and why. Use the paths, labels and ids an answer shows.

## Forms
- An element is its label, its id or its number from `elements` (`#3`): the same label first, then one that holds the
  text, ignoring case. `elements=true` adds the list with each field's kind, value and required, changed or error marks.
- `set` takes several fields at once: `set={"Username": "jdoe", "Email": "jdoe@example.com"}`.
- A drop-down takes a choice's label or its value and chooses it as a person does, so the fields that depend on it
  follow. Set the field it depends on first, then the dependent one, in the same call or the next.
- A date field takes `YYYY-MM-DD`, `YYYY-MM-DD HH:mm` or a phrase (see the `dates` skill).
- Leave a field you were not given empty; never guess a value for a required field: ask.
- Elements marked `[needs approval]` write; pressing one asks the user (see the `approvals` skill).

## Lists
- `find` moves to text on the screen, a list row's included; `do` on a row's link opens the record.
- To filter: `do Find` opens the list's find fields, `set` them, then `do Find` again. Each find field has its own
  operator; an empty field adds nothing.
- A list shows a page of rows with `(showing 25 of 312)`; page on before you say something is not there, or count and
  total with `data` instead (see `data-reads`).
- `rows` (at most 100) shows more of a long screen.

## Dialogs
- A dialog's fields exist only after `do` on its button: `do Create User Account`, then `set` its fields, then `do` its
  submit button.
- `overlay` names what is open: a dialog, a confirm with its choices, a drop-down. With a confirm open, `do Yes`
  answers it, or `do No`.
- `key Esc` closes a dialog without saving. Use `key` only when no element fits.

## Errors
- `--- error no element named "Svae"; did you mean: Save (button)?`: use the name it suggests.
- `message error ...` after a save: the form's validation; fix the field it names and press again.
- A permission error is the answer about permission: tell the user, do not look for another way in.
- The same call failing the same way twice: stop and tell the user what failed.
