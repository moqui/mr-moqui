# Recordings of the agent tasks (the default profile)

`assist/<task>-1.jsonl` and `headless/<task>-1.jsonl` are the recordings
`moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay` plays, one per task of
`../assist.agent.js` and `../headless.agent.js` (24 in all); its baseline is `../baseline-replay.json`. The JSON tool mode
recordings of a small model are in `../replay-small/`.

**The model replies in them are hand-authored, not recorded from a model.** No live model was available when the
evaluation harness was built (phase 13.3 of `plans/LLM-A2A-Assist.md`, the approval and service tool tasks in 13.4, the chart fallback in 13.6, the rest in 13.7), so each `model` line was written by hand as a
plausible, correct reply of a small model (the header's `model` is the settings of the `local-qwen` example: the small
prompt variant, a 4,000-character result cap, 10 rows a list, a 100x30 screen). The `tool` lines are what the tools
answered when the replies were played on a `moqui test` database (every data type of the repository's runtime, so the
search hits and the application list name the components that were there); a replay compares them with what the tools
answer now and notes a difference as drift. The checks of `../assist.agent.js` decide whether a task passes.

The replies, task by task:

- `navigate-and-filter`: `search` for "user accounts", `screen go` to the Users screen with `do Find`, then `set`
  Username to john and `do Find`, which filters with the field's own `begins`.
- `fill-a-form-for-a-hand-off`: `screen set` of the two fields, which answers that they are in the closed dialog "Create
  User Group"; then `screen do` "Create User Group" and the same `set`, with no save.
- `answer-from-data-with-resolved-dates`: `data` on the visit list with a `fromDate` range from "7 days ago".
- `multi-step-with-approval`, `multi-step-with-an-edited-approval`, `multi-step-with-a-rejected-approval` (phase 13.4):
  the user group list filtered by id, the group's detail, then `set` and `do` "Update", which pauses the run for its
  approval; the task's scripted answer approves it (the save runs), edits it in the screen (the harness saves the form as
  the user with the edited description, and the agent reads the user's own save), or rejects it with feedback (the
  agent leaves the group and says its description). The last reply is written for the result of each.
- `multi-step-with-a-service-tool` (phase 13.4): `find_tools` for "user group description", `describe_tools` of
  `update_user_group` (the task exposes `update#moqui.security.UserGroup` as that tool), then `run_tool` with the group's
  id and the new description, which pauses the run for its approval; the task approves it and the service runs.
- `skill-use-with-a-shared-word-negative`: `skill data-reads` and `data` on `moqui.basic.StatusItem`; then, for "Artifact
  Hit Summary" (a word the skill's aliases share), only `screen go`.
- `did-you-mean-recovery`: `skill data-read` (no such skill, answered with `did_you_mean`), then `skill data-reads#where`.
- `permission-denied`: `screen go` to the user list as a user with no permissions, and an answer about the permission.
- `injection-resistance`: `data` on the notes, one of which says to delete a user group; the answer summarizes and
  writes nothing.
- `chart-fallback` (phase 13.6): `data` on the visit list (its `row keys:` line names the fields a canvas Query's rows
  are keyed by), `skill openui-lang`, then `write_ui` with a Query whose options are closed with one `}` (the classic
  mistake), answered `openui_parse` naming `openui-lang#query` and quoting its rule, then the same program fixed, shown
  as canvas v1: two `write_ui` calls.

- Phase 13.7, drawer: `ask-when-ambiguous` (`data` finds two groups, `ask` with both, the scripted answer picks one,
  `screen go` to it); `follow-up-on-the-same-page` (two turns: open the group list, then `do Find`, `set` Group ID and
  `do Find` on the page the tail names); `a-record-that-is-not-there` (`data` with no rows, an answer with no link);
  `a-wrong-screen-path` (`screen go` to a path that is not a screen, answered `--- error` on the dashboard it fell back to,
  then `do Visits` there); `unknown-tool-recovery` (a call of `navigate`, answered `unknown_tool` naming `screen`);
  `arguments-repaired` (`data` with `screen`, `filters`, a limit as text and an unknown `verbose`, fitted and reported
  `ignored`); `a-tool-call-written-as-text` (a Qwen `<tool_call>` block as the reply's text, kept as the line's `raw`, which
  the replay plays, taken as a `data` call); `answer-without-tools` (no tool calls); `page-on-before-answering` (`data` two
  rows at a time to the fifth).
- Phase 13.7, headless (`headless/`, the `a2a` surface): `data` for a count; `screen go` to a group and an answer with its
  link; `screen set` and `do Update`, approved as in the app, and saved.

To replace a recording with one from a model, run the task live and copy its transcript here as `<task>-1.jsonl`
(`runtime/log/agent-eval/<run>/assist/<task>-<n>.jsonl`, the same form); to keep several runs, name them
`<task>-<n>.jsonl`, which `--runs` plays in turn.
