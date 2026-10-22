"""The official OpenAI Python client against the Moqui LLM gateway (/llm/v1).

Lists the models, makes a whole and a streamed chat completion (with the usage chunk asked for), a streamed one with
a function tool whose call comes back, a whole and a streamed Responses call, and embeddings; then the errors the client
maps to its exceptions: an unknown model (NotFoundError), a bad key (AuthenticationError), a key whose quota is used up
(RateLimitError, when GATEWAY_KEY_OVER_QUOTA is given) and a field the gateway does not pass (BadRequestError).

    python3 -m venv venv && venv/bin/pip install openai
    GATEWAY_KEY=<login key> venv/bin/python openai_client.py http://localhost:8080/llm/v1

The host test crates/moqui-host/tests/it/gateway.rs runs it when OPENAI_PYTHON names such a python, against a fake
upstream model server on loopback (the aliases fast, resp and embed of its conf).
"""

import os
import sys

import openai

BASE = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('GATEWAY_BASE_URL', 'http://localhost:8080/llm/v1')
KEY = os.environ.get('GATEWAY_KEY', '')
OVER_QUOTA = os.environ.get('GATEWAY_KEY_OVER_QUOTA', '')

passed: list[str] = []
failed: list[str] = []


def check(name: str, condition: bool, detail: object = '') -> None:
    (passed if condition else failed).append(name)
    print(f"  {'ok  ' if condition else 'FAIL'} {name}{': ' + str(detail) if detail else ''}", flush=True)


def raises(name: str, error: type, call) -> None:
    try:
        call()
        check(name, False, 'no error')
    except error as caught:
        check(name, True, f'{type(caught).__name__} {getattr(caught, "status_code", "")}')
    except Exception as caught:  # noqa: BLE001 - the wrong exception is the failure being reported
        check(name, False, f'{type(caught).__name__}: {caught}')


client = openai.OpenAI(base_url=BASE, api_key=KEY, max_retries=0, timeout=60)
print(f'openai {openai.__version__} against {BASE}')

models = sorted(model.id for model in client.models.list())
check('models.list', 'fast' in models and 'embed' in models and 'other' not in models, models)
check('models.retrieve', client.models.retrieve('fast').id == 'fast')

whole = client.chat.completions.create(model='fast', messages=[{'role': 'user', 'content': 'hi'}])
check('chat whole', whole.choices[0].message.content == 'Hello whole', whole.choices[0].message.content)
check('chat whole usage', whole.usage is not None and whole.usage.total_tokens == 12, whole.usage)

text, usage = '', None
for chunk in client.chat.completions.create(model='fast', messages=[{'role': 'user', 'content': 'hi'}], stream=True,
                                            stream_options={'include_usage': True}):
    if chunk.choices and chunk.choices[0].delta.content:
        text += chunk.choices[0].delta.content
    if chunk.usage is not None:
        usage = chunk.usage
check('chat stream', text == 'Hello there', text)
check('chat stream usage', usage is not None and usage.prompt_tokens == 11 and usage.completion_tokens == 4, usage)

tools = [{'type': 'function', 'function': {'name': 'lookup', 'parameters': {'type': 'object', 'properties': {}}}}]
arguments, finish = '', None
for chunk in client.chat.completions.create(model='fast', messages=[{'role': 'user', 'content': 'use-tool'}],
                                            stream=True, tools=tools):
    if chunk.choices:
        for call in chunk.choices[0].delta.tool_calls or []:
            arguments += call.function.arguments or ''
        finish = chunk.choices[0].finish_reason or finish
check('chat stream tool call', arguments == '{"q":1}' and finish == 'tool_calls', (arguments, finish))

response = client.responses.create(model='resp', input='hi')
check('responses whole', response.output_text == 'Hi there', response.output_text)
deltas, kinds = '', []
for event in client.responses.create(model='resp', input='hi', stream=True):
    kinds.append(event.type)
    if event.type == 'response.output_text.delta':
        deltas += event.delta
check('responses stream', deltas == 'Hi there' and kinds[-1] == 'response.completed', (deltas, kinds))

embedded = client.embeddings.create(model='embed', input=['forgot password', 'ship order'])
check('embeddings', len(embedded.data) == 2 and len(embedded.data[0].embedding) == 4, len(embedded.data))
first = next(item for item in embedded.data if item.index == 0).embedding
check('embeddings by index', first[0] > first[1], first)

raises('unknown model is NotFoundError', openai.NotFoundError,
       lambda: client.chat.completions.create(model='nope', messages=[{'role': 'user', 'content': 'hi'}]))
raises('a bad key is AuthenticationError', openai.AuthenticationError,
       lambda: openai.OpenAI(base_url=BASE, api_key='0' * 40, max_retries=0).models.list())
raises('a field not passed through is BadRequestError', openai.BadRequestError,
       lambda: client.chat.completions.create(model='fast', messages=[{'role': 'user', 'content': 'hi'}],
                                              extra_body={'web_search_options': {}}))
if OVER_QUOTA:
    raises('a used-up quota is RateLimitError', openai.RateLimitError,
           lambda: openai.OpenAI(base_url=BASE, api_key=OVER_QUOTA, max_retries=0).chat.completions.create(
               model='fast', messages=[{'role': 'user', 'content': 'hi'}]))

print(f'{len(passed)} passed, {len(failed)} failed')
sys.exit(1 if failed else 0)
