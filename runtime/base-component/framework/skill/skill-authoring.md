---
name: skill-authoring
title: Writing a skill
description: Use when the user asks you to write, save or improve a skill, a playbook or a procedure for next time.
kind: guide
aliases: [playbook, procedure, how to, save as skill, front matter, instructions]
sections:
  front-matter: The keys a skill starts with
  body: How to write the steps
---
A skill is Markdown that starts with front matter. The catalog shows its description; `skill` loads its text. Write
one from what worked: the screens, fields, buttons and services of a run that succeeded, not from guesses.

## Front matter
```
---
name: create-user-account          # kebab-case: a-z, 0-9 and -
title: Creating a user account
description: Use when the user asks to add a login for a person.   # starts "Use when", at most 160 characters
kind: procedure                    # procedure, reference or guide
aliases: [login, new user]         # words users say for it
screens: [/apps/system/Security/UserAccount/UserAccountList]
risk: confirm                      # reversible, confirm or irreversible
---
```
`requires: [screen-use]` loads another skill with it; `sections:` gives each `##` section a purpose for the contents.

## Body
- Start with one or two sentences: what it is for and where it is done.
- Then `## Steps`, numbered: the tool call of each step with the exact path, element labels and field names.
- Say what to check before a create (search first) and where to stop for the user (a password, a choice).
- Keep it under 6,000 characters; a longer skill goes in `##` sections that load one at a time.
- No ids, names or values of one user's records; no passwords or keys.
