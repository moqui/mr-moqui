---
name: services
title: Running services as tools
description: Use when no screen does the job and you look for a service with find_tools, describe it and run it with run_tool.
kind: guide
aliases: [service, run_tool, find_tools, describe_tools, api, tool]
---
Services are the second way, after screens. `find_tools` with a few words lists up to 8 names, each with one line and
read or write; `describe_tools` with every name you need in one call gives their parameters and results; `run_tool`
runs one as the user with `name` and `parameters`.

- A read runs now. A write (create, update, delete, send) waits for the user's approval; one write at a time.
- Pass parameters as `describe_tools` lists them; a wrong one is an error that names the right ones. Never pass
  `authUsername`, `authPassword` or any password or key: the call already runs as the user.
- Read the result, not only `ok`: the ids it returns are the ones to use next, and its messages say what happened.
- A `create#Entity` style service writes the row alone, without the checks, status and related records a screen's
  button makes. Prefer the screen, or a named service a skill gives, for anything the user does on a screen.
- A service missing from `find_tools` is not offered to you: say so, do not guess its name.
