#!/usr/bin/env python3
"""A tiny mock stdio MCP server for hermetic integration tests.

Speaks newline-delimited JSON-RPC 2.0 over stdin/stdout. It understands the
three methods the client exercises (`initialize`, `tools/list`, `tools/call`)
plus the `notifications/initialized` notification (which carries no id and gets
no reply). Anything else returns a JSON-RPC method-not-found error.

Special tool `boom` makes the process exit non-zero mid-session so the client's
broken-pipe / child-exited error path can be tested.
"""

import json
import sys


def write(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def handle(req):
    method = req.get("method")
    req_id = req.get("id")

    # Notifications (no id) get no response.
    if req_id is None:
        return None

    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": "2025-06-18",
                "serverInfo": {"name": "mock-stdio", "version": "9.9.9"},
            },
        }

    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo back the message",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"msg": {"type": "string"}},
                        },
                    }
                ]
            },
        }

    if method == "tools/call":
        params = req.get("params") or {}
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "boom":
            # Crash mid-session: caller's next read sees a closed pipe.
            sys.exit(3)
        if name == "echo":
            return {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {"structuredContent": {"echoed": args.get("msg")}},
            }
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "isError": True,
                "content": [{"type": "text", "text": "unknown tool"}],
            },
        }

    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "error": {"code": -32601, "message": "method not found"},
    }


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        resp = handle(req)
        if resp is not None:
            write(resp)


if __name__ == "__main__":
    main()
