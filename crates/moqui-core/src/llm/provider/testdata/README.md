# Provider stream fixtures

Hand-written, not recorded. No live model endpoint was available when these were made (phase 13.1), so each `.sse`
file is written from the vendor's documented wire format and from how the Java prototype read it
(`OpenAiCompatProtocol.java`, `OpenAiResponsesProtocol.java`, `LlmRetryClassifier.java` on the `llm-client` branch).
Ids, signatures, encrypted content and token counts are made up. Replace a file with a recorded stream when one is
captured, keeping its name, and rewrite the goldens with `UPDATE_GOLDEN=1 cargo test -p moqui-core llm::provider`.

Each `<name>.sse` is played through an installed `HttpSender` and decoded by its protocol; `<name>.events.json` is the
golden: the normalized events and the assembled reply.

| File | Protocol | What it covers |
| --- | --- | --- |
| `lmstudio-qwen-tool.sse` | chat-completions | LM Studio with Qwen: `reasoning_content` (an empty first delta, kept for the echo), one tool call whose arguments come in pieces, the usage chunk after the finish |
| `llamacpp-reasoning.sse` | chat-completions | llama.cpp: `reasoning_content`, text, usage and `timings` in the finish chunk (`cache_n`) |
| `ollama-parallel-index0.sse` | chat-completions | Ollama: two whole tool calls both at index 0 told apart by id, and `finish_reason` stop with tool calls |
| `vllm-reasoning-tools.sse` | chat-completions | vLLM: the `reasoning` field, two tool calls by index, `prompt_tokens_details.cached_tokens` |
| `deepseek-reasoning-content.sse` | chat-completions | DeepSeek: `reasoning_content` to replay, `prompt_cache_hit_tokens`, `reasoning_tokens` |
| `gemini-thought-signature.sse` | chat-completions | Gemini's compatible endpoint: a tool call's `extra_content.google.thought_signature` to replay; reasoning tokens outside `completion_tokens` |
| `chat-truncated.sse` | chat-completions | a stream that ends with text and no finish reason: a retryable failure |
| `openai-responses-reasoning-tool.sse` | responses | OpenAI: `event:` lines, a reasoning item with a summary and `encrypted_content` to replay, a function call with argument deltas, cached and reasoning tokens |
| `xai-responses-text.sse` | responses | xAI: the type only in the data (no `event:` lines), an encrypted reasoning item with no summary, text, `[DONE]` |
| `responses-incomplete.sse` | responses | `response.incomplete` at `max_output_tokens`: a cut reply |
| `anthropic-thinking-tool.sse` | anthropic-messages | a `thinking` block with its `signature_delta`, a `redacted_thinking` block, text, a `tool_use` with `input_json_delta`, `ping`, cache reads and writes in the usage |
| `anthropic-overloaded.sse` | anthropic-messages | an `overloaded_error` event in the middle of a reply |
