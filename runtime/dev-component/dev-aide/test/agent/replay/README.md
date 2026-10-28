# Recordings of the dev agent's tasks

`dev/<task>-1.jsonl` are the recordings `moqui test dev-aide --agents --replay runtime/dev-component/dev-aide/test/agent/replay`
plays, one per task of `../dev.agent.js`.

**The model replies in them are hand-authored, not recorded from a model**, as the tools component's first recordings were:
each `model` line is a plausible, correct reply of a small model (the header's `model` is the `local-qwen` example's
settings). They carry no `tool` lines, so a replay notes no drift; the checks of `../dev.agent.js` decide whether a task
passes. The replies, task by task:

- `add-a-field-through-the-site-and-show-it`: `skill moqui-dev` and `skill site-customizing`; `artifact find` of the
  UserAccount entity and the UserAccountForm form; `source` of the form; then `propose_edit` of an `extend-entity` with
  the nickname field, inserted at the end of the site's entity file, and a new `screen-extend` of UserAccountDetail
  placing the field after Full Name. The task approves it; the files are written and reloaded.
- `add-a-list-column`: `skill moqui-dev` and `artifact find` of the list; `propose_edit` of a `screen-extend` of
  UserAccountList with the Currency column after Time Zone, approved.
- `find-where-a-service-is-used`: `artifact refs` of `create#UserAccount`, and an answer naming the UserAccountList
  screen's transition.
- `write-a-spec-from-a-try`: `skill moqui-dev` and `try_service` of `create#moqui.example.Example` in the sim (the rows it
  would write, nothing kept); `propose_edit` of a new spec file in the example component's `test` directory, approved.

To replace a recording with one from a model, run the task live and copy its transcript here as `dev/<task>-1.jsonl`
(`runtime/log/agent-eval/<run>/dev/<task>-<n>.jsonl`).
