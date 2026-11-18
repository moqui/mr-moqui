---
name: testing
description: Where a test goes (the regression or the acceptance suite), what a regression test checks, the budget, walks and sweeps, and what to run while working. Use before writing or running tests in mr-moqui or a component.
---

# Testing

- Two suites. The regression suite (`test/`, `test/client/`) is for development time: small, run while working and before every commit. The acceptance suite (`test/acceptance/`, `test/acceptance/client/`) is for pre-deploy and system validation: complete scenarios, run on a schedule, before a release, or when asked. Say which suite a new test goes in, and why. Details: [docs/Build-Test.md](../../../docs/Build-Test.md#the-two-suites).
- Regression tests check rules, not pages. Write one for a business rule you add or change (its result and its refusals) and for a bug you fix (the test that would have caught it). Access: one holder and one non-holder on one verb of the rule, and another organization's record refused; not every route for every group. Render a screen only when it has logic of its own, once per mode, with all of its checks on that render, and in a second mode only where it has its own qvt or tui template. No `where:` doubling of authorization on and off, no users × modes matrices. Mantle rules are tested in Mantle, not again through an app. A rule test is not cut to meet a count: time decides, through the budget.
- Walks. The regression suite has one golden-path client walk per app, kept current. Every other walk is an acceptance test: a plan's "usable when" scenario (server side or in a browser), a phone-width check, an access or isolation walk.
- Sweeps. The acceptance suite may hold one screen sweep per app per mode; the regression suite holds none. No screenshots for review.
- Order. A spec that needs another spec's data names it in `dependsOn`; a run then puts it after that spec, in any component, and `--spec` brings it in. Do not rename a spec to sort after its fixture. Details: [docs/dev/JS-Testkit.md](../../../docs/dev/JS-Testkit.md#spec).
- Budget. A regression spec runs in well under a second as a rule; one over 15 s fails the run. A component over 1.5 times its budget (`test/budget`) fails the run; cut duplicates or move scenarios to acceptance rather than raise the budget. A budget is raised only with the user's agreement.
- What to run. While working: the regression specs of what you touched (`./moq test <component> --spec ...`, `./moq test-client <component> --serve --spec ...`), on an already built binary (`--nobuild`) unless `.rs` files changed. Before a commit: the [pre-commit skill](../pre-commit/SKILL.md), by what changed; before a release, the [pre-production skill](../pre-production/SKILL.md).
- Acceptance tests run only when the user asks (a component's, one spec, or all: `./moq accept [component...]`, `./moq accept <component> --spec Name`, `./moq accept-all`), or when you add or change one: then that test, and its component's acceptance suite once the test is written. Never because code it uses changed, never to check your own work, never as part of "done" for a phase.
- Plans and Done. A phase is done when its regression tests for the rules it added pass, the component's regression suite passes within budget, and `moqui lint --screens` is clean. A plan's "usable when" scenarios become acceptance tests to write (named in the plan), not regression tests, and running them is not part of Done. A brief that wants scenario checks names the acceptance tests to write and the suite of each test it asks for.
- A disposable runtime (`MOQUI_RUNTIME`) goes under the repository's ignored `target/` (`target/<name>`, linking
  `runtime/components/base` and `dev` and copying `runtime/conf`), removed afterwards and run on ports no other server
  holds. Not under a temp or scratch directory: the embedded Postgres socket, `<runtime>/db/embedded/.s.PGSQL.5432`,
  must be under 108 bytes.
- A Rust test that runs the `git` CLI (clone, fetch, push) or `ssh` isolates it from the desktop, or a credential
  prompt opens and the test waits on it: `-c credential.helper=`, `GIT_ASKPASS=false`, `SSH_ASKPASS=false`,
  `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, no `XDG_CONFIG_HOME`, a scratch `HOME`,
  `GIT_TERMINAL_PROMPT=0` (the `git_in` helper of the git server tests is the pattern).
