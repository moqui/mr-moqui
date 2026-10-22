# The LLM gateway and the LLM embedder

Moqui passes model calls of internal tools and scripts through to its configured models at `/llm/v1`, an
OpenAI-compatible API: the callers use their own keys, Moqui adds the central key, checks the quotas and logs the usage
as it does for its agents. The same configured models make embeddings for semantic search and the skill retrieval (the
LLM embedder). The design is [plans/LLM-A2A-Assist.md](../plans/LLM-A2A-Assist.md) (4.17, 4.2, D10, 4.19). As of Oct
22, 2026 (13.13 built).

The code: `crates/moqui-core/src/llm/gateway.rs` (routes, auth, the allowlists, aliases, admission, the pass-through),
`crates/moqui-core/src/llm/embed.rs` (embeddings and `LlmEmbedder`), the host adapter
`crates/moqui-host/src/llm/gateway.rs`, and the embedder choice in `crates/moqui-host/src/search.rs`. Tests: the unit
tests in both core files, and `crates/moqui-host/tests/it/gateway.rs` (over HTTP on embedded Postgres against a fake
model server on loopback, and the official OpenAI Python client, below).

## Turning it on

The gateway is off by default; while it is off everything under `/llm/v1` answers 404.

```xml
<llm-facade>
    <gateway enabled="${llm_gateway_enabled}" require-client="true" timeout="300">
        <alias name="fast" model="local-qwen"/>
        <alias name="default" feature="assist"/>
    </gateway>
</llm-facade>
```

- `llm_gateway_enabled=true` turns it on. `timeout` is the seconds one call may take in all.
- **Aliases** are the model names callers use: `model` names a model of the registry (System › LLM › Models), `feature`
  a feature whose binding (with its fallback and its reasoning level) the alias follows. `LlmModelAlias` rows (`alias`,
  `modelId` or `featureId`) replace a conf alias of the same name, and `disabled="Y"` turns one off. A conf that lists
  aliases replaces the earlier conf's. A request whose `model` is not an alias the caller may use is 404
  `model_not_found`; nothing is sent.
- A caller needs the `LlmGatewayUse` permission and, for its credential, an `LlmApiClient` row (System › LLM › Gateway,
  `store#LlmApiClient`) that names it (`loginKey`, kept as the hash a `UserLoginKey` is stored under, or
  `appPasswordId`), of the same user, not disabled, with `gateway` in `scopes`. An app password (made for DAV, SFTP or
  git) always needs one; a login key needs one with `require-client` (the default). The row of the credential with
  the gateway scope is the one used (rows for A2A or MCP are not); `store#LlmApiClient` refuses a second gateway row
  for a credential, and two found are refused as ambiguous. The client's `allowedAliases` (empty for all) narrow the
  aliases, and its `dailyTokens`, `dailyCost` and `requestsPerMinute` apply on top of the user's own limits (a client
  cannot raise what its user may use). `lastUsedDate` is noted at most once a minute.

## Calling it

Any OpenAI client works with `base_url` `https://<host>/llm/v1` and the key as its API key:

```python
client = openai.OpenAI(base_url='https://erp.example.com/llm/v1', api_key='<login key or app password>')
client.chat.completions.create(model='fast', messages=[{'role': 'user', 'content': 'hi'}], stream=True)
```

| Path | |
| --- | --- |
| `GET /models`, `GET /models/{alias}` | The aliases the caller may use whose model can take a call |
| `POST /chat/completions` | Whole or streamed, to a `chat-completions` endpoint |
| `POST /responses` | Whole or streamed (the event names kept), to a `responses` endpoint, with `store: false` |
| `POST /embeddings` | To the `/v1/embeddings` of a `chat-completions` or `responses` endpoint |

- **Auth**: `Authorization: Bearer <login key>` or `Bearer <app password>` (what OpenAI clients send), or HTTP Basic
  with an app password (or a login key as the password, never the account password); only over TLS, a believed proxy's
  https, or loopback. Missing or bad credentials are 401 `invalid_api_key` with `WWW-Authenticate`; a refused login
  counts for web protection.
- **What passes**: only the request fields of the API's allowlist (Chat Completions: `messages`, `stream`,
  `stream_options.include_usage`, `temperature`, `top_p`, `n`, `stop`, `max_tokens`, `max_completion_tokens`,
  `presence_penalty`, `frequency_penalty`, `logit_bias`, `logprobs`, `top_logprobs`, `seed`, `response_format`, `tools`,
  `tool_choice`, `parallel_tool_calls`, `reasoning_effort`, `user`; Responses: `input`, `instructions`, `stream`,
  `temperature`, `top_p`, `max_output_tokens`, `tools`, `tool_choice`, `parallel_tool_calls`, `text`, `reasoning`,
  `truncation`, `include` (only `reasoning.encrypted_content`), `store` (only false), `user`; embeddings: `input`,
  `encoding_format`, `dimensions`, `user`). Any other field is 400 `unsupported_parameter` naming it, so a caller
  cannot change the base URL, reach another API, or use stored responses (`previous_response_id`). `n` must be 1 (more
  choices are more calls), and `max_tokens`, `max_completion_tokens` and `max_output_tokens` are clamped to the model's
  `max-output`. The caller's headers are not sent on.
- **The upstream body** is those fields with the model's `api-config` merged over them and, for a reasoning level the
  request asks for (`reasoning_effort`, `reasoning.effort`; `minimal` is low, `xhigh` max) or the alias's feature binds,
  the model's patch for that level (`provider::apply_model_patches`, the agents' own, which cannot change the messages,
  input or tools; the stream settings are the gateway's own); `model` becomes the provider's name and the endpoint's key and headers are added.
  The central key never reaches the caller.
- **No tools run here.** Function tool definitions pass through to the model and its tool calls come back to the caller,
  who runs them; any other tool type (hosted search, file search, code interpreter, MCP) is 400 `unsupported_tool`.
- **Streams**: a Chat Completions stream is sent upstream with `stream_options.include_usage`, so its usage is known;
  the usage chunk reaches the caller only when it asked for it. A stream is read through the provider pump and written
  on as it comes, each write taken within the endpoint's `idle-timeout` and the call's deadline: a caller that goes
  away or stops reading is gone, the upstream request is dropped (and with it the endpoint slot it held), and the call
  is logged as canceled. A stream the model server cuts, or that fails or times out, ends with an error frame
  (`data: {"error": ...}`), not `[DONE]`.
- **Whole calls** a caller leaves are canceled too: the gateway stops waiting and logs the call as canceled. The
  blocking upstream request itself ends when the model server answers or the call's `timeout` passes, and holds its
  endpoint slot until then.
- **Errors** are in the OpenAI shape `{"error": {"message", "type", "param", "code"}}`, with generic messages: the model
  server's detail (which may name its internal URL) is logged and kept in `LlmCall.errorText`. A model server's refusal
  of a request is its status (400, 409, 413, 422), its rate limit 429 with its `Retry-After`, a refusal of the central
  key, a failure or a bad response 502, a timeout 504. `/llm/v1` is exempt from the webapp's `request-timeout`: the
  gateway's own `timeout` bounds a call and answers 504 in this shape.

## Limits and usage

Before a call: the user's quota for the `gateway` scope (`LlmQuota`, else the conf's daily limits) and the client's
daily tokens and cost (counted from its `LlmCall` rows), each with the most the call may use: its prompt estimated from
its text by the model's calibration and its output limit (the request's, clamped, else the model's `max-output`), at
the model's prices; over either is 429 `insufficient_quota` with `Retry-After` until the day or month ends. Then the
host's room for one more open call (below; 503 or 429 `too_many_open_calls`), and only then the requests per minute of
the user's `LlmQuota` (the smallest of its rows) and of the client (429 `rate_limit_exceeded` with the seconds until a
request of the last minute ages out) and the AT_LLM tarpit (`gateway:<alias>`, 120 a minute per user in the seed data),
so a call the host has no room for counts against neither. Requests per minute are counted per node in memory, so a
cluster of n nodes lets up to n times the limit through; the quotas are counted from the database.

A call runs on a thread of the host's own, not a request worker, counted with the A2A server's waits and streams (256 on
a node, 16 of one user; one more is 503 or 429). The endpoint's `max-concurrent` and the model's `max-concurrency` limit
it as they limit an agent's calls. Each upstream request is one `LlmCall` row of source `gateway` with the user, the
client (`clientId`), the alias's feature, the tokens and the cost, and the content only with `content-log`; the daily
rollup and the usage screens count it as any call. When the model's usage did not come (a stream cut or left before
its usage chunk), the tokens are estimated (the prompt from its text, the output from what was passed on) and the row
says `usageEstimated="Y"`; the quotas count them as any usage.

## Admin screens

System › LLM › Gateway (AgentAdmin): **Clients** lists the `LlmApiClient` rows with their user, their credential's kind
(an app password's id, or "login key"; a key is never shown), scopes, aliases, limits and last use, and makes a client
from one of a user's app passwords or login keys (`store#LlmApiClient`, which checks that the credential is the user's
and refuses a second gateway client for it); **Client** edits its scopes, aliases and limits, sets a new login key, and
shows its recent calls; **Aliases** lists, adds, edits and deletes the `LlmModelAlias` rows (the conf's aliases are in
the conf).

## The LLM embedder

`LlmEmbedder` (`crate::llm::embed`) implements the search facade's `Embedder` over a configured model's
`/v1/embeddings` (LM Studio, Ollama, llama.cpp, vLLM, OpenAI), at most 64 texts a request, each request an `LlmCall` of
source `embed`. It keeps the vectors of the batches that succeeded by text, so a batch that failed is sent again without
them. When the server does not answer (a timeout, a network or server error), its breaker skips it for 60 seconds:
calls are refused at once, the skill retrieval ranks by words, and the search worker does not try a failed batch again
a text at a time. A query (`embed_quick`) waits at most 8 seconds. Its name is fixed when it is made: a call whose
binding now goes to another model (a changed binding, its fallback) is refused, so vectors of two models never mix
under one name. Nothing is sent unless it is configured:

- `<search-facade><semantic enabled="true" embedder="llm" dimensions="768"/></search-facade>` makes it the search
  facade's embedder, over the model bound to the `embed` feature (`embedder="llm:<modelId>"` names a model instead).
  The vectors must have the `dimensions` the store keeps; a model that shortens its vectors takes
  `<api-config>{"dimensions": 768}</api-config>`. Its model name, stored with each vector, is `llm:<modelId>/<provider
  model>`, taken at startup.
- The skill retrieval's semantic branch uses the search facade's embedder, else the `embed` feature's model when the
  `llm-facade` (or an `LlmFeatureBinding` row) binds it, so skills are found by meaning without pgvector. A skill found
  only by meaning is never a strong match.

## Checking it with the OpenAI Python client

```sh
python3 -m venv /tmp/openai-venv && /tmp/openai-venv/bin/pip install openai
OPENAI_PYTHON=/tmp/openai-venv/bin/python LANG=en_US.UTF-8 cargo test -p moqui-host --test it gateway
```

Without `OPENAI_PYTHON` the host test prints a `SKIPPED:` line for the client check and runs the rest. The script,
`crates/moqui-host/tests/fixtures/gateway/openai_client.py`, lists and retrieves models, makes whole and streamed chat
completions (the usage chunk, a tool call), whole and streamed Responses calls and embeddings, and checks that the
client raises `NotFoundError`, `AuthenticationError`, `BadRequestError` and `RateLimitError` where it should. It was
run with openai 3.36.1.

## Not built

- Chat Completions to a model on a Responses or Anthropic Messages endpoint (and Responses to a Chat Completions one):
  the gateway passes each API through to its own protocol only, so such an alias is 400 `unsupported_model`.
- The legacy `/completions`, files, batches, images, audio and the stored-response APIs.
- Making a login key for a client in the admin screen and showing it once: the screen takes a key the user made, or
  one of the user's app passwords.
- Interrupting a whole call's blocking upstream request when its caller leaves (see Whole calls).
