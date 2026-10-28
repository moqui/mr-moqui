---
name: moqui-coding-agent
title: Changing Moqui components from a coding agent
description: Use when you edit a Moqui component's files from a terminal or editor and need them live, checked and extended the right way.
kind: guide
aliases: [moqui check, moqui refs, dev reload, live reload, extend-entity, screen-extend, site component, coding agent]
sections:
  reloading: What a saved file reloads on a dev instance, and when a restart or rebuild is needed
  checks: The schema checks moqui check runs on a file
  references: The reference checks of names in a file
  expressions: The expression checks
  extending: Extend an upstream component from the site component instead of editing it
  specs: Running a component's specs
---
A Moqui component is XML (entities, services, screens, ECA rules, data), JavaScript (scripts and expressions), and
sometimes Rust. On a dev instance (`instance_purpose` dev, the default `./moq run`), the server watches every component
directory: save a file and what it defines is live a moment later. Check what you changed with `moqui check` before
you say it is done.

## Reloading
- A service file: its services are reloaded, added and removed; the next call runs the new definition.
- A SECA, EmECA or gitECA file (`service/*.secas.xml` and the like) and an EECA file (`entity/*.eecas.xml`): the rules
  of that kind are read again.
- A screen file, a `screen-extend` file, a `.qvue`, a template or a script: dropped from the caches at once and read on
  next use.
- A REST API file (`service/*.rest.xml`): the REST roots are read again. A skill file: synced into the skill index.
- An entity file: on a dev instance the entity model is linked again and the table check adds the new tables and columns
  (the reload report lists the DDL). A file that does not parse keeps the old model and says restart to apply.
- `component.xml`, `MoquiConf.xml` or the runtime conf: restart to apply. Rust source or a `Cargo.toml`: rebuild with
  `./moq build`, then restart.
- A file that does not parse keeps the definitions loaded before; the server log names the file and the error.
- The server log has one `Dev reload:` line per change with what was reloaded.

## Checks
Run `moqui check` (or `./moq run check` from the repository) with the files you changed, or with no files to check
what git shows as changed. Each problem is `path:line:col: error: message [rule, skill#section]`; `--json` gives one
JSON object per line. The exit status is 1 when there is an error and 0 otherwise, so it works as a hook.

- A file that does not parse as XML keeps the definitions loaded before.
- An element the schema does not allow here is a typo or misplaced.
- An attribute the schema does not define for the element is ignored.
- A required attribute is missing.
- The value is not one the attribute allows.
- The service does not register as written.

The schemas are the framework's XSDs in `framework/xsd`; each element's documentation there says what it does. Schema
problems are warnings: the XSDs trail a few attributes and values the framework reads, so check the framework's code
before changing a file for one. References, expressions and XML that does not parse are errors.

## References
Names in a file are checked against what the server loaded. `moqui refs <name>` lists what uses an artifact (`--in`)
or what it uses (`--out`): the name is an artifact id (`entity:moqui.example.Example`, `service:a.B.run#Thing`,
`screen:component://...`, `form:component://...#Name`), a full or short name (`OrderHeader`, `place#Order`), or a file.
On a dev server the same index answers the MCP tools `artifact` (find, describe, refs, at), `source` and `check`.

- No entity or view entity has this name.
- No service has this name, and it is not an entity-auto name of an entity.
- The entity of the find around it has no field of this name.
- The service called has no in-parameter of this name.
- Nothing is at this location.

A name with `${}` in it is made at run time and is not checked.

## Expressions
- Expressions and scripts are JavaScript, and this one does not parse.

`condition`, `from`, `in-map` and other expression attributes, `${}` in text, and `<script>` bodies are JavaScript.
Groovy idioms (`?.` on a map is fine, but `as`, `?:` with Groovy truth, closures with `->`) need the JavaScript form.

## Extending
An upstream component (the framework, or any app or library of entities and services the site does not own) is not
edited in place: an edit there is a fork that every upgrade fights. Put the change in the site's own component:

- A field, relationship or index on an upstream entity: `extend-entity` in the site component's entity file.
- A field on an upstream form, a list column, a widget, an action, a subscreen or a transition: a `screen-extend` file
  in the site component's `screen-extend` directory, at the path of the base screen.
- A rule on an upstream service or entity: a new SECA or EECA file in the site component.
- Seed data such as enumerations and status items: a seed data file in the site component.

- A screen-extend file names no base screen at its path.
- The base screen has no element of this name to extend.
- No entity has this name to extend.
- No service or entity has this name for the rule to run on.

`moqui check --extensions` checks every site extension against what it extends, which is worth running after pulling
an upstream component. A `screen-extend` needs a named form, container, section or transition in the base screen to anchor to. When the base
screen has none where you need it, the anchor is added to the base screen as its own change, never generated.

## Specs
`./moq test <component>` runs a component's JavaScript specs (`test/*.spec.js`); `--spec Name` runs one. A spec calls
services and checks the rows they wrote with `checkData`.
