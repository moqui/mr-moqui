# Recordings of the agent tasks (the default profile)

`assist/<task>-1.jsonl` and `headless/<task>-1.jsonl` are the recordings
`moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay` plays, one per task of
`../assist.agent.js` and `../headless.agent.js`; the baseline is `../baseline-replay.json`. The JSON tool mode
recordings of a small model are in `../replay-small/`.

**The model replies are hand-authored, not recorded from a model.** Each `model` line is a plausible, correct reply of
a small model (the header's `model` is the settings of the `local-qwen` example: the small prompt variant, a
4,000-character result cap, 10 rows a list, a 100x30 screen). The `tool` lines are what the tools answered when the
replies were played on a `moqui test` database with every data type of the repository's runtime; a replay compares them
with what the tools answer now and notes a difference as drift. The checks in the task files decide whether a task
passes, and each task there says what it exercises.

To replace a recording with one from a model, run the task live and copy its transcript here as `<task>-1.jsonl`
(`runtime/log/agent-eval/<run>/assist/<task>-<n>.jsonl`, the same form); to keep several runs, name them
`<task>-<n>.jsonl`, which `--runs` plays in turn. Agent evaluation is described in `docs/Testing.md` ("Agent
evaluation").
