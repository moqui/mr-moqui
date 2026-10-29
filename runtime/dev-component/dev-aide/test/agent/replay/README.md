# Recordings of the dev agent's tasks

`dev/<task>-1.jsonl` are the recordings
`moqui test dev-aide --agents --replay runtime/dev-component/dev-aide/test/agent/replay` plays, one per task of
`../dev.agent.js`.

**The model replies are hand-authored, not recorded from a model**: each `model` line is a plausible, correct reply of
a small model (the header's `model` is the `local-qwen` example's settings). They carry no `tool` lines, so a replay
notes no drift; the checks of `../dev.agent.js` decide whether a task passes.

To replace a recording with one from a model, run the task live and copy its transcript here as `dev/<task>-1.jsonl`
(`runtime/log/agent-eval/<run>/dev/<task>-<n>.jsonl`).
