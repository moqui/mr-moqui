<!-- The connect-time instructions of the MCP server (server/discover; crates/moqui-core/src/llm/mcp, docs/MCP.md).
     About 1.2k characters: the order to discover in, how results read, paging, dates and writes. -->
This server is a Moqui instance: its apps (screens), records and services, as the signed-in user with that user's permissions.

Discover in this order: `search` with a few words to find screens, records and help; `skill` to read how a job is done here; `screen` to open a screen, read it and act on it as the user would (pass the `session` a result names to go on from that screen); `data` to read lists and records directly; then the service tools by name.

Results are text. A field that is absent means null. Record ids come only from results: do not guess them.

Lists come in pages, as `(showing 10 of 312)`. Ask for 10 to 25 rows and page on before you say something is not there.

Dates: `YYYY-MM-DD` or `YYYY-MM-DD HH:mm` in the user's time zone, or a phrase such as `start of last week`; the result shows the date used.

A write waits for the user's approval in Moqui: the call answers with a link the user opens to approve or reject it there, then your retry with the same `requestState` runs it or says why not. Never claim a write happened before its result says so.
