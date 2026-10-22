<!-- The intent pre-call (plans/LLM-A2A-Assist.md 7.9, intent-prefilter="hint"): a tiny call before the turn whose one
    word goes into the tail as "likely: data". After Nextcloud's prototype trick: a function declaration small models
    complete reliably. -->
declare function route(message: string): "answer" | "screen" | "data" | "service" | "ui";
// answer: a question about how the apps work, answered without tools
// screen: open, fill or press something on a screen
// data: count, total or list rows
// service: run a service or a connected tool
// ui: a chart, a dashboard or another generated view
Reply with only the value route returns for the user's message.
