"""Interoperability check of the Moqui A2A server with the official a2a-sdk client, over both bindings.

Ported from the Java prototype's framework/test/a2a/interop_a2a_sdk.py (JSON-RPC only there) to A2A 1.0 as served at
/a2a: discovery from /.well-known/agent-card.json, then for JSON-RPC and HTTP+JSON each: SendMessage, GetTask,
ListTasks, CancelTask of an ended and of an unknown task, SendStreamingMessage, SubscribeToTask, CancelTask of a
working task, a push notification to a loopback receiver (the server's conf must name 127.0.0.1 in push allow-hosts),
and GetExtendedAgentCard.

    python3 -m venv venv && venv/bin/pip install a2a-sdk httpx
    A2A_TOKEN=<login key> venv/bin/python interop_a2a_sdk.py http://localhost:8080

The host test crates/moqui-host/tests/it/a2a.rs runs it when A2A_SDK_PYTHON names such a python, against a server
whose model is the scripted fake provider (each reply takes a moment, so a task can be subscribed to and canceled).
"""

import asyncio
import json
import os
import sys
import uuid

import a2a.types as t
import httpx
from a2a.client import ClientConfig, create_client

BASE = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('A2A_BASE_URL', 'http://localhost:8080')
TOKEN = os.environ.get('A2A_TOKEN', '')

passed: list[str] = []
failed: list[str] = []


def check(name: str, condition: bool, detail: object = '') -> None:
    (passed if condition else failed).append(name)
    print(f"  {'ok  ' if condition else 'FAIL'} {name}{': ' + str(detail) if detail else ''}", flush=True)


def message(text: str) -> t.Message:
    return t.Message(message_id=str(uuid.uuid4()), role=t.Role.ROLE_USER, parts=[t.Part(text=text)])


def payload_of(event: object) -> tuple[str, object]:
    value = getattr(event, 'root', event)
    which = value.WhichOneof('payload') if hasattr(value, 'WhichOneof') else None
    return which or type(value).__name__, value


def state(task: t.Task) -> str:
    return t.TaskState.Name(task.status.state)


class Receiver:
    """A loopback HTTP receiver of push notifications: each POST's headers and JSON body."""

    def __init__(self) -> None:
        self.posts: list[tuple[dict[str, str], dict]] = []
        self.server: asyncio.base_events.Server | None = None
        self.port = 0

    async def start(self) -> None:
        self.server = await asyncio.start_server(self.handle, '127.0.0.1', 0)
        self.port = self.server.sockets[0].getsockname()[1]

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            head = await reader.readuntil(b'\r\n\r\n')
            lines = head.decode('latin-1').split('\r\n')
            headers = {}
            for line in lines[1:]:
                if ':' in line:
                    name, value = line.split(':', 1)
                    headers[name.strip().lower()] = value.strip()
            body = await reader.readexactly(int(headers.get('content-length', '0')))
            self.posts.append((headers, json.loads(body or b'{}')))
            writer.write(b'HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
            await writer.drain()
        finally:
            writer.close()

    async def wait_for(self, task_id: str, wanted: str, seconds: float = 60) -> tuple[dict[str, str], dict] | None:
        for _ in range(int(seconds * 10)):
            for headers, body in self.posts:
                update = body.get('statusUpdate') or {}
                if update.get('taskId') == task_id and update.get('status', {}).get('state') == wanted:
                    return headers, body
            await asyncio.sleep(0.1)
        return None


async def binding(http_client: httpx.AsyncClient, name: str, receiver: Receiver) -> None:
    print(f'== {name}', flush=True)
    client = await create_client(BASE, ClientConfig(httpx_client=http_client, streaming=False,
                                                    supported_protocol_bindings=[name]))
    task = None
    async for event in client.send_message(t.SendMessageRequest(message=message('Hello, who are you?'))):
        which, value = payload_of(event)
        if which == 'task':
            task = value.task
    check(f'{name} SendMessage returns a Task', task is not None)
    if task is None:
        return
    check(f'{name} task completed', state(task) == 'TASK_STATE_COMPLETED', state(task))
    check(f'{name} response artifact',
          bool(task.artifacts) and task.artifacts[0].artifact_id == 'response' and bool(task.artifacts[0].parts[0].text),
          task.artifacts[0].parts[0].text if task.artifacts else 'none')
    roles = [t.Role.Name(m.role) for m in task.history]
    check(f'{name} history has the user and agent messages', roles == ['ROLE_USER', 'ROLE_AGENT'], roles)

    fetched = await client.get_task(t.GetTaskRequest(id=task.id, history_length=10))
    check(f'{name} GetTask', fetched.id == task.id and fetched.context_id == task.context_id, state(fetched))
    listed = await client.list_tasks(t.ListTasksRequest(page_size=100))
    check(f'{name} ListTasks', any(other.id == task.id for other in listed.tasks),
          f'{len(listed.tasks)} of {listed.total_size}')
    try:
        await client.cancel_task(t.CancelTaskRequest(id=task.id))
        check(f'{name} CancelTask of an ended task is refused', False, 'accepted')
    except Exception as error:  # noqa: BLE001 - the SDK raises its own typed errors
        check(f'{name} CancelTask of an ended task is refused', type(error).__name__ == 'TaskNotCancelableError',
              f'{type(error).__name__}: {str(error)[:90]}')
    try:
        await client.get_task(t.GetTaskRequest(id='does-not-exist'))
        check(f'{name} GetTask of an unknown task is refused', False, 'accepted')
    except Exception as error:  # noqa: BLE001
        check(f'{name} GetTask of an unknown task is refused', type(error).__name__ == 'TaskNotFoundError',
              f'{type(error).__name__}: {str(error)[:90]}')

    streaming = await create_client(BASE, ClientConfig(httpx_client=http_client, streaming=True,
                                                       supported_protocol_bindings=[name]))
    seen: list[str] = []
    final = None
    async for event in streaming.send_message(t.SendMessageRequest(message=message('Answer as a stream'))):
        which, value = payload_of(event)
        seen.append(which)
        if which == 'status_update':
            final = t.TaskState.Name(value.status_update.status.state)
    check(f'{name} stream carries the Task and its updates',
          seen[:1] == ['task'] and 'artifact_update' in seen and final == 'TASK_STATE_COMPLETED', seen)

    polling = await create_client(BASE, ClientConfig(httpx_client=http_client, streaming=False, polling=True,
                                                     supported_protocol_bindings=[name]))
    started = None
    async for event in polling.send_message(t.SendMessageRequest(message=message('Take your time'))):
        which, value = payload_of(event)
        if which == 'task':
            started = value.task
    check(f'{name} returnImmediately gives the task at once', started is not None and
          state(started) in ('TASK_STATE_SUBMITTED', 'TASK_STATE_WORKING'), state(started) if started else None)
    if started is not None:
        updates: list[str] = []
        async for event in streaming.subscribe(t.SubscribeToTaskRequest(id=started.id)):
            which, value = payload_of(event)
            updates.append(which if which != 'status_update' else t.TaskState.Name(value.status_update.status.state))
        check(f'{name} SubscribeToTask streams the task to its end',
              updates[:1] == ['task'] and updates[-1] == 'TASK_STATE_COMPLETED', updates)

    working = None
    async for event in polling.send_message(t.SendMessageRequest(message=message('This one is canceled'))):
        which, value = payload_of(event)
        if which == 'task':
            working = value.task
    if working is not None:
        canceled = await client.cancel_task(t.CancelTaskRequest(id=working.id))
        check(f'{name} CancelTask of a working task', state(canceled) == 'TASK_STATE_CANCELED', state(canceled))

    token = str(uuid.uuid4())
    config = t.TaskPushNotificationConfig(url=f'http://127.0.0.1:{receiver.port}/hook', token=token,
                                          authentication=t.AuthenticationInfo(scheme='Bearer', credentials='push-secret'))
    pushed = None
    async for event in polling.send_message(t.SendMessageRequest(
            message=message('Tell me when done'),
            configuration=t.SendMessageConfiguration(task_push_notification_config=config))):
        which, value = payload_of(event)
        if which == 'task':
            pushed = value.task
    if pushed is not None:
        got = await receiver.wait_for(pushed.id, 'TASK_STATE_COMPLETED')
        check(f'{name} push to the loopback receiver', got is not None and
              got[0].get('x-a2a-notification-token') == token and
              got[0].get('authorization') == 'Bearer push-secret',
              got[0] if got else f'{len(receiver.posts)} posts')
        configs = await client.list_task_push_notification_configs(
            t.ListTaskPushNotificationConfigsRequest(task_id=pushed.id))
        check(f'{name} push configs are listed without their secrets',
              len(configs.configs) == 1 and not configs.configs[0].token and
              not configs.configs[0].authentication.credentials, configs.configs)

    extended = await client.get_extended_agent_card(t.GetExtendedAgentCardRequest())
    check(f'{name} extended card', bool(extended.skills), f'{extended.name}, {len(extended.skills)} skills')


async def main() -> None:
    receiver = Receiver()
    await receiver.start()
    headers = {'Authorization': f'Bearer {TOKEN}'} if TOKEN else {}
    async with httpx.AsyncClient(headers=headers, timeout=180) as http_client:
        print('== discovery from /.well-known/agent-card.json', flush=True)
        client = await create_client(BASE, ClientConfig(httpx_client=http_client, streaming=False))
        card = getattr(client, '_card', None)
        if card is not None:
            bindings = sorted(f'{i.protocol_binding} {i.protocol_version}' for i in card.supported_interfaces)
            check('agent card resolved', bool(card.name), f'{card.name} {card.version}')
            check('JSON-RPC and HTTP+JSON 1.0 interfaces', bindings == ['HTTP+JSON 1.0', 'JSONRPC 1.0'], bindings)
            check('capabilities match the implementation',
                  card.capabilities.streaming and card.capabilities.push_notifications
                  and card.capabilities.extended_agent_card, card.capabilities)
        for name in ('JSONRPC', 'HTTP+JSON'):
            await binding(http_client, name, receiver)
    print(f'\n{len(passed)} passed, {len(failed)} failed' + (f' -> {failed}' if failed else ''), flush=True)
    sys.exit(1 if failed else 0)


asyncio.run(main())
