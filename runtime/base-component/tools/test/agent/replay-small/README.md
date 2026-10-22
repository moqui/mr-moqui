# JSON tool mode recordings of a small model

`assist/<task>-1.jsonl` and `headless/<task>-1.jsonl` are what
`moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay-small` plays: 9 of the 24 tasks, as a
small model in JSON tool mode would answer them (plans/LLM-A2A-Assist.md 7.9, phase 13.7). The set's profile is
`replay-small` and its baseline `../baseline-replay-small.json`; the tasks it has no recording of are skipped.

**The model replies are hand-authored, not recorded from a model** (no model was available). The header's `model` is the
settings of an 8B model on a 16k window: `toolMode` json, the small prompt variant, an 80x24 screen, a 3,000-character
result cap, 10 rows a list, 2,048 output tokens, and the intent hint on, as the conf's commented `local-small` has it.
A replay makes no side call (the replay model has `sideCalls` N), so the hint is not asked for and a compaction would
take the deterministic summary: a recording holds only the steps' replies. Each `model` line's `raw` is the text the model wrote, one protocol
object (`{"tool": …, "args": …}`, `{"reply": …}`, `{"ask": …}`), which the replay plays as the reply's text; its
`toolCalls` and `text` are what the run read from it. `did-you-mean-recovery`'s first reply writes the object in a code
fence, which the parser takes (an object after a sentence is not taken: it may be a quote, and is asked again). The `tool` lines are what the tools answered when the replies were
played.

The tasks: navigate and filter, answer from data with resolved dates, multi-step with approval (approved), did-you-mean
recovery, permission denied, injection resistance, ask when ambiguous, answer without tools, and the headless answer from
data. To add a task, write its replies as protocol objects in the same form, play the set, and take the transcript's
`tool` lines (`runtime/log/agent-eval/<run>/<spec>/<task>-1.jsonl`); a live run of a small model in JSON mode
(`--llm-profile` of a model with `tool-mode="json"`) writes transcripts of the same form.
