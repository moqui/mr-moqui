"""Smoke check of the Moqui MCP server (protocol 2026-07-28) with the official MCP Python SDK's client.

The client connects with mode 'auto' (it calls server/discover and must not fall back to initialize), lists the tools
(sorted, with their caching hints), calls a read tool, then a write: the write answers input_required with a URL
elicitation, which this script's callback answers as the user would, by deciding the approval in their own browser
session (the owner's cookie and session token; the MCP client itself can never decide), and the SDK's retry with the
requestState runs it once. A second client that declares no URL elicitation is refused the write.

    python3 -m venv venv && venv/bin/pip install 'mcp==2.3.0'
    MCP_TOKEN=<login key> MCP_OWNER_COOKIE=<cookie> MCP_OWNER_CSRF=<session token> MCP_WRITE_TOOL=<tool> \
        MCP_WRITE_ARGS='<json>' venv/bin/python -I smoke_mcp_sdk.py http://localhost:8080

The host test crates/moqui-host/tests/it/mcp.rs runs it when MCP_SDK_PYTHON names such a python.
"""

import asyncio
import json
import os
import sys

import httpx2
import mcp.types as types
from mcp import Client
from mcp.client.streamable_http import streamable_http_client

BASE = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('MCP_BASE_URL', 'http://localhost:8080')
TOKEN = os.environ.get('MCP_TOKEN', '')
OWNER_COOKIE = os.environ.get('MCP_OWNER_COOKIE', '')
OWNER_CSRF = os.environ.get('MCP_OWNER_CSRF', '')
READ_TOOL = os.environ.get('MCP_READ_TOOL', 'search')
READ_ARGS = json.loads(os.environ.get('MCP_READ_ARGS', '{"text": "enumeration"}'))
WRITE_TOOL = os.environ.get('MCP_WRITE_TOOL', '')
WRITE_ARGS = json.loads(os.environ.get('MCP_WRITE_ARGS', '{}'))

passed: list[str] = []
failed: list[str] = []


def check(name: str, condition: bool, detail: object = '') -> None:
    (passed if condition else failed).append(name)
    print(f"  {'ok  ' if condition else 'FAIL'} {name}{': ' + str(detail)[:400] if detail else ''}", flush=True)


def text_of(result: types.CallToolResult) -> str:
    return ''.join(getattr(block, 'text', '') for block in result.content)


async def decide_as_owner(url: str) -> None:
    """What the user does in their browser at the link: the drawer opens the link's conversation, whose card they
    approve."""
    async with httpx2.AsyncClient(base_url=BASE, timeout=30) as browser:
        headers = {'Cookie': OWNER_COOKIE, 'X-CSRF-Token': OWNER_CSRF, 'Accept': 'application/json'}
        conversation = url.rsplit('assist=', 1)[-1]
        shown = await browser.get('/assist/conversation', headers=headers, params={'conversationId': conversation})
        pending = shown.json().get('pending', {}).get('approvals', []) if shown.status_code == 200 else []
        ids = [card['approvalId'] for card in pending]
        check('the owner sees the approval card in the drawer', len(ids) == 1, shown.text)
        if ids:
            decided = await browser.post('/assist/decide', headers=headers,
                                         json={'approvalId': ids[0], 'decision': 'approve'})
            check('the owner approves in Moqui', decided.status_code == 200, decided.text)


async def main() -> int:
    seen: list[types.ElicitRequestParams] = []

    async def elicitation(_context, params):
        seen.append(params)
        url = getattr(params, 'url', '') or ''
        check('the elicitation is a URL to Moqui', getattr(params, 'mode', '') == 'url' and '/qapps?assist=' in url,
              params)
        await decide_as_owner(url)
        return types.ElicitResult(action='accept')

    http = httpx2.AsyncClient(headers={'Authorization': f'Bearer {TOKEN}'}, timeout=60)
    async with Client(streamable_http_client(f'{BASE}/mcp', http_client=http), mode='auto',
                      elicitation_callback=elicitation, input_required_max_rounds=20,
                      client_info=types.Implementation(name='moqui-smoke', version='1')) as client:
        check('the protocol is 2026-07-28 (server/discover, no initialize)', client.protocol_version == '2026-07-28',
              client.protocol_version)
        info = client.server_info
        check('serverInfo names Moqui', info is not None and info.name == 'moqui', info)
        check('connect-time instructions', bool(client.instructions) and 'requestState' in client.instructions)

        listed = await client.list_tools()
        names = [tool.name for tool in listed.tools]
        check('tools/list is sorted by name', names == sorted(names) and len(names) > 0, names)
        check('tools/list has the read tools', all(name in names for name in ('data', 'search', 'skill')), names)
        check('tools/list has the write tool', WRITE_TOOL in names, names)
        check('tools/list is private with a TTL', getattr(listed, 'cache_scope', None) == 'private'
              and (getattr(listed, 'ttl_ms', 0) or 0) > 0, listed)

        read = await client.call_tool(READ_TOOL, READ_ARGS)
        check(f'{READ_TOOL} runs', not read.is_error and bool(text_of(read)), text_of(read))

        wrote = await client.call_tool(WRITE_TOOL, WRITE_ARGS)
        check('the write asked for its approval by a link', len(seen) == 1, seen)
        check('the approved write ran', not wrote.is_error and 'ran' in text_of(wrote), text_of(wrote))

    # a client that takes no URL elicitations cannot write: the server names the capability it needs
    plain = httpx2.AsyncClient(headers={'Authorization': f'Bearer {TOKEN}'}, timeout=60)
    async with Client(streamable_http_client(f'{BASE}/mcp', http_client=plain), mode='auto') as client:
        try:
            refused = await client.call_tool(WRITE_TOOL, WRITE_ARGS)
            check('a write without URL elicitation is refused', False, refused)
        except Exception as error:  # the SDK raises the JSON-RPC error
            check('a write without URL elicitation is refused', '-32021' in repr(error) or 'elicitation' in str(error),
                  repr(error))

    print(f'{len(passed)} passed, {len(failed)} failed', flush=True)
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(asyncio.run(main()))
