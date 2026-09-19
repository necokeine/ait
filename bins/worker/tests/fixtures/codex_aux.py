"""Offline native auxiliary operations, each owned by a supervised worker."""
import json
import os
import sys

def emit(value):
    print(json.dumps(value), flush=True)

def thread():
    return {"id": "aux-thread", "sessionId": "aux-session", "cwd": os.getcwd(), "source": "appServer", "preview": "", "historyMode": "paginated", "status": {"type": "idle"}, "createdAt": 1, "updatedAt": 1, "turns": []}

for line in sys.stdin:
    request = json.loads(line)
    with open(LOG, "a") as output:
        output.write(json.dumps({"pid": os.getpid(), "request": request}) + "\n")
    method = request.get("method")
    identity = request.get("id")
    if method == "initialize":
        result = {}
    elif method == "thread/list":
        result = {"data": [] if request["params"].get("archived") else [thread()], "nextCursor": None}
    elif method == "thread/read":
        assert request["params"]["includeTurns"] is False
        result = {"thread": thread()}
    elif method == "thread/turns/list":
        assert request["params"]["itemsView"] == "full"
        result = {"data": [], "nextCursor": None}
    elif method == "model/list":
        result = {"data": [{"model": "fixture-model", "displayName": "Fixture", "supportedReasoningEfforts": [{"reasoningEffort": "high"}]}], "nextCursor": None}
    elif method == "thread/start":
        assert request["params"]["ephemeral"] is True
        assert request["params"]["sandbox"] == "read-only"
        assert "developerInstructions" not in request["params"]
        result = {"thread": thread()}
    elif method == "turn/start":
        assert request["params"]["approvalPolicy"] == "never"
        emit({"id": identity, "result": {"turn": {"id": "title-turn"}}})
        emit({"method": "item/completed", "params": {"threadId": "aux-thread", "turnId": "title-turn", "item": {"id": "title", "type": "agentMessage", "text": json.dumps({"title": "Review native execution", "description": "Verify the unified worker"})}}})
        emit({"method": "turn/completed", "params": {"threadId": "aux-thread", "turn": {"id": "title-turn", "status": "completed"}}})
        continue
    else:
        continue
    emit({"id": identity, "result": result})
