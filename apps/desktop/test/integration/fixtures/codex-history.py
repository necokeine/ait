#!/usr/bin/env python3
"""Offline history fixture behind the real Ait daemon and worker."""
import json
import pathlib
import sys

root = pathlib.Path(__file__).resolve().parent.parent

def thread(project):
    return {
        "id": "native-" + project, "sessionId": "session-" + project,
        "projectId": "unrelated-codex-project", "cwd": str(root / project),
        "source": "cli", "preview": "New native conversation", "name": None,
        "historyMode": "paginated", "status": {"type": "notLoaded"},
        "createdAt": 1, "updatedAt": 2, "turns": [],
    }

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    with (root / "methods.jsonl").open("a") as output:
        output.write(json.dumps(method) + "\n")
    if method == "initialized":
        continue
    if method == "initialize":
        result = {}
    elif method == "thread/list":
        result = {"data": [] if request["params"]["archived"] else [thread("project-one"), thread("project-two")], "nextCursor": None}
    elif method == "thread/read":
        result = {"thread": thread(request["params"]["threadId"].removeprefix("native-"))}
    elif method == "thread/turns/list":
        result = {"data": [], "nextCursor": None}
    else:
        raise RuntimeError("Unexpected operation: " + method)
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
