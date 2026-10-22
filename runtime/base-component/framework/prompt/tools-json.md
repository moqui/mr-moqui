<!-- The protocol of JSON tool mode (plans/LLM-A2A-Assist.md 7.9), for a model whose tool-mode is json: the tools are
    not sent as tools, so they are listed after this text as signatures, every one every time (a fixed prompt keeps
    the cache). -->
## Tools
You have tools but no tool calls. Each reply is exactly one JSON object and nothing else:
- a tool: {"tool": "screen", "args": {"go": "/apps/system/Security/UserGroup/UserGroupList"}}
- the answer: {"reply": "The answer, in plain sentences."}
- a question: {"ask": {"question": "Which one?", "options": ["First", "Second"]}}
A tool's result comes back as <result tool="name">…</result>; then send the next object.
The tools, as name(argument: type) and what each does; ? marks an argument you may leave out:
