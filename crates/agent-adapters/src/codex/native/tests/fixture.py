#!/usr/bin/env python3
"""Offline app-server fixture using the 0.153.4 response field names."""
import json
import os
import sys

scenario, log = sys.argv[-2:]
with open(log + ".pid", "w") as output:
    output.write(str(os.getpid()))
turns = []
pending = None


def send(value):
    print(json.dumps(value), flush=True)


def finish():
    global turns
    status = "interrupted" if scenario in ("cancel", "wait_cancel") else "completed"
    turns = [{"id": "native-turn", "status": status, "error": None,
              "itemsView": "full", "startedAt": 1, "completedAt": None if status == "interrupted" else 2,
              "items": [{"id": "input", "type": "userMessage", "clientId": pending["clientUserMessageId"], "content": pending["input"]},
                        {"id": "command", "type": "commandExecution", "command": "pwd", "status": "completed", "aggregatedOutput": os.getcwd()},
                        {"id": "answer", "type": "agentMessage", "text": "authoritative answer"}]}]
    send({"method": "turn/completed", "params": {"threadId": "thread", "turn": {"id": "native-turn", "status": status, "items": [], "itemsView": "notLoaded"}}})


for line in sys.stdin:
    request = json.loads(line)
    with open(log, "a") as output:
        output.write(json.dumps(request) + "\n")
    method = request.get("method")
    identity = request.get("id")
    if method == "initialize":
        send({"id": identity, "result": {}})
    elif method == "thread/resume":
        if scenario == "busy":
            send({"id": identity, "error": {"code": -32600, "message": "thread thread already has an active writer"}})
            continue
        params = request["params"]
        assert params["approvalsReviewer"] == "user"
        assert "cwd" not in params
        assert "developerInstructions" not in params
        send({"id": identity, "result": {
            "thread": {"id": "thread"}, "model": params["model"], "modelProvider": "openai", "reasoningEffort": "medium",
            "cwd": "/wrong" if scenario == "cwd" else os.getcwd(), "sandbox": {"type": "readOnly", "networkAccess": False},
            "approvalPolicy": "on-request", "approvalsReviewer": "auto_review" if scenario == "reviewer" else "user"}})
    elif method == "thread/read":
        send({"id": identity, "result": {"thread": {"id": "thread", "sessionId": "session", "cwd": os.getcwd(), "source": "appServer",
            "preview": "", "historyMode": "paginated", "status": {"type": "active" if any(t["status"] == "inProgress" for t in turns) else "idle"}, "createdAt": 1, "updatedAt": 2, "turns": []}}})
    elif method == "thread/turns/list":
        assert request["params"]["itemsView"] == "full"
        send({"id": identity, "result": {"data": turns, "nextCursor": None}})
    elif method == "turn/start":
        pending = request["params"]
        assert pending["approvalsReviewer"] == "user"
        assert pending["effort"] == "high"
        assert pending["sandboxPolicy"] == {"type": "readOnly", "networkAccess": False}
        if scenario == "reject":
            send({"id": identity, "error": {"code": -32602, "message": "invalid input"}})
        elif scenario == "disconnect":
            sys.exit(0)
        else:
            send({"id": identity, "result": {"turn": {"id": "native-turn"}}})
            if scenario == "wait_cancel":
                turns = [{"id": "native-turn", "status": "inProgress", "error": None, "itemsView": "full", "startedAt": 1, "completedAt": None,
                          "items": [{"id": "input", "type": "userMessage", "clientId": pending["clientUserMessageId"], "content": pending["input"]}]}]
            else:
                finish()
    elif method == "turn/interrupt":
        send({"id": identity, "result": {}})
        finish()
