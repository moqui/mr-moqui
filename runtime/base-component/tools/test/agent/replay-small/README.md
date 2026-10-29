# JSON tool mode recordings of a small model

`assist/<task>-1.jsonl` and `headless/<task>-1.jsonl` are what
`moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay-small` plays: 9 of the tasks, as a
small model in JSON tool mode would answer them (navigate and filter, dates, approval, did-you-mean recovery,
permission denied, injection resistance, ask when ambiguous, answer without tools, and a headless answer). The set's
profile is `replay-small` and its baseline `../baseline-replay-small.json`; tasks with no recording are skipped.

**The model replies are hand-authored, not recorded from a model.** The header's `model` is the settings of an 8B model
on a 16k window, as the conf's commented `local-small` has it (`toolMode` json, the small prompt variant, an 80x24
screen, a 3,000-character result cap). A replay makes no side call, so a recording holds only the steps' replies. Each
`model` line's `raw` is the text the model wrote, one protocol object (`{"tool": …, "args": …}`, `{"reply": …}`,
`{"ask": …}`), which the replay plays; its `toolCalls` and `text` are what the run read from it. The `tool` lines are
what the tools answered when the replies were played.

To add a task, write its replies as protocol objects in the same form, play the set, and take the transcript's `tool`
lines (`runtime/log/agent-eval/<run>/<spec>/<task>-1.jsonl`). A live run of a small model in JSON mode
(`--llm-profile` of a model with `tool-mode="json"`) writes transcripts of the same form.
