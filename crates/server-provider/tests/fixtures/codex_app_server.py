#!/usr/bin/env python3
"""Offline stdio protocol peer. Never invokes Codex, a network API, or a model."""
import json
import os
from pathlib import Path
import sys
import time
import subprocess
import threading
import time
import uuid

root = Path.cwd()
mode = (root / "behavior").read_text() if (root / "behavior").exists() else "normal"
output_lock = threading.Lock()
pending = None
thread_id = None
turn_text = ""
history_lock = threading.Lock()
stream_items = []
steered_inputs = []

def emit(value):
    with output_lock:
        sys.stdout.write(json.dumps(value) + "\n")
        sys.stdout.flush()

def history_path():
    return root / ("native-history-" + thread_id + ".json")

def load_history():
    path = history_path()
    return json.loads(path.read_text()) if path.exists() else []

def thread_metadata(identifier):
    path = root / ("native-session-" + identifier + ".json")
    defaults = {"id": identifier, "cwd": str(root), "createdAt": 1700000000,
                "updatedAt": 1700000000, "status": {"type": "idle"},
                "model": "offline-model", "reasoningEffort": "high", "preview": "Offline prompt"}
    if path.exists():
        defaults.update(json.loads(path.read_text()))
    return defaults

def save_turn(turn_id, text, status):
    with history_lock:
        turns = [turn for turn in load_history() if turn["id"] != turn_id]
        items = [{"type": "userMessage", "id": turn_id + "-user", "content": [{"type": "text", "text": turn_text}]}]
        items.extend(stream_items)
        items.extend(steered_inputs)
        if text is not None:
            items.append({"type": "agentMessage", "id": turn_id + "-assistant", "text": "Echo: " + text})
        turns.append({"id": turn_id, "status": status, "startedAt": 1700000000, "items": items})
        history_path().write_text(json.dumps(turns))

def complete(turn_id, text):
    global pending
    if pending != turn_id:
        return
    save_turn(turn_id, text, "failed" if text == "fail" else "completed")
    emit({"method": "item/completed", "params": {
        "threadId": thread_id, "turnId": turn_id,
        "item": {"type": "agentMessage", "id": turn_id + "-assistant", "text": "Echo: " + text}}})
    emit({"method": "turn/completed", "params": {
        "threadId": thread_id, "turn": {"id": turn_id, "status": "failed" if text == "fail" else "completed"}}})
    pending = None

for line in sys.stdin:
    request = json.loads(line)
    with (root / "native-requests.jsonl").open("a") as log:
        log.write(json.dumps({"pid": os.getpid(), "argv":sys.argv[1:], **request}) + "\n")
    method = request.get("method")
    if method is None and "result" in request:
        if pending:
            complete(pending, "approved" if request["result"].get("decision") == "accept" or request["result"].get("answers") else "denied")
        continue
    if method == "initialized" or method is None:
        continue
    params = request.get("params", {})
    if mode == "interrupt-completed" and method == "turn/interrupt":
        complete(params["turnId"], "raced")
        emit({"id": request["id"], "error": {"message": "turn already completed"}})
        continue
    if mode == "timeout":
        time.sleep(60)
    if mode == "malformed":
        print("not json", flush=True)
        continue
    if mode == "oversize":
        print("x" * (2 * 1024 * 1024), flush=True)
        continue
    if mode == "wrong-id":
        emit({"id": 999999, "result": {}})
        continue
    if mode == "error":
        emit({"id": request["id"], "error": {"message": "sensitive native error"}})
        continue
    result = {}
    if method == "initialize":
        result = {"userAgent":"codex_cli_rs/0.153.4"} if mode == "workflows" else {}
    elif method == "collaborationMode/list":
        result = {"data":[{"name":"Plan","mode":"plan","model":"offline-model","reasoning_effort":"high"},
            {"name":"Default","mode":"default","model":"offline-model"}]} if mode == "workflows" else {"data":[]}
    elif method in ("thread/start", "thread/resume", "thread/read"):
        thread_id = params.get("threadId", str(uuid.uuid4()))
        if mode == "wrong-thread":
            thread_id = "wrong-thread"
        if thread_id == "missing-session":
            emit({"id": request["id"], "error": {"message": "sensitive missing session path"}})
            continue
        metadata = thread_metadata(thread_id)
        if method == "thread/start":
            (root / ("native-session-" + thread_id + ".json")).write_text(json.dumps(metadata))
        result = {"thread": {**metadata, "turns": load_history() if params.get("includeTurns") else []}, "model": "offline-model"}
        if mode == "missing-thread":
            result = {}
    elif method == "thread/fork":
        source = params["threadId"]
        turns_path = root / ("native-history-" + source + ".json")
        turns = json.loads(turns_path.read_text()) if turns_path.exists() else []
        if "lastTurnId" in params:
            turns = turns[:next(index + 1 for index, turn in enumerate(turns) if turn["id"] == params["lastTurnId"])]
        thread_id = source if mode == "fork-same-id" else str(uuid.uuid4())
        metadata = {**thread_metadata(source), "id": thread_id}
        (root / ("native-session-" + thread_id + ".json")).write_text(json.dumps(metadata))
        history_path().write_text(json.dumps(turns))
        result = {"thread": {**metadata, "turns": turns}}
    elif method == "thread/rollback":
        thread_id = params["threadId"]
        turns = load_history()[:-params["numTurns"]]
        history_path().write_text(json.dumps(turns))
        result = {"thread": {**thread_metadata(thread_id), "turns": turns}}
    elif method == "account/read":
        result = {"account": {"type": "chatgpt", "email": "private@example.test", "planType": "plus"}, "requiresOpenaiAuth": True}
    elif method == "account/rateLimits/read":
        result = {"rateLimits": {"planType": "plus", "primary": {"usedPercent": 25, "resetsAt": 1900000000}, "secondary": {"usedPercent": 81}}}
    elif method == "skills/list":
        result = {"data": [{"cwd": params["cwds"][0], "errors": [], "skills": [
            {"name": "review", "description": "Offline review", "enabled": True, "path": str(root / "review" / "SKILL.md")},
            {"name": "disabled", "description": "Disabled", "enabled": False, "path": str(root / "disabled" / "SKILL.md")}
        ]}]}
    elif method == "thread/list":
        pages = root / "session-pages.json"
        if pages.exists():
            result = json.loads(pages.read_text())[params.get("cursor") or "first"]
        else:
            sessions = [thread_metadata(path.name[len("native-session-"):-5])
                        for path in root.glob("native-session-*.json")]
            if params.get("sourceKinds") == ["subAgentThreadSpawn"]:
                sessions = [entry for entry in sessions if entry.get("parentThreadId") or isinstance(entry.get("source"), dict)]
            sessions.sort(key=lambda entry: entry["updatedAt"], reverse=True)
            result = {"data": sessions, "nextCursor": None}
    elif method == "model/list":
        result = {"data": [{"id": "offline-model", "model": "offline-model", "displayName": "Offline model", "isDefault": True, "hidden": False, "description": "Offline fixture", "supportedReasoningEfforts": [{"reasoningEffort": "high", "description": "High effort"}], "defaultReasoningEffort": "high", "serviceTiers": [] if mode == "no-fast" else [{"id": "fast"}]}], "nextCursor": None}
    elif method == "turn/start":
        if mode == "delayed-voice-admission":
            while not (root / "release-voice-admission").exists():
                time.sleep(0.01)
        pending = str(uuid.uuid4())
        stream_items = []
        steered_inputs = []
        result = {"turn": {"id": pending}}
    elif method == "turn/steer":
        if mode == "steer-completed":
            complete(pending, "raced")
        if mode in ("steer-reject", "steer-completed") or (root / "reject-steer").exists() or params["expectedTurnId"] != pending:
            emit({"id": request["id"], "error": {"code": -32600, "message": "no active turn to steer"}})
            continue
        if mode == "steer-ambiguous":
            emit({"id": request["id"], "error": {"code": -32603, "message": "uncertain internal failure"}})
            continue
        if mode == "steer-exit":
            sys.exit(0)
        result = {"turnId": "wrong-turn" if mode == "steer-wrong-id" else pending}
    emit({"id": request["id"], "result": result})
    if method == "thread/compact/start":
        compact_turn = pending or str(uuid.uuid4())
        if pending is None:
            emit({"method":"turn/started","params":{"threadId":thread_id,"turn":{"id":compact_turn}}})
        item = {"type":"contextCompaction","id":"compact-" + compact_turn}
        stream_items.append(item)
        emit({"method":"item/completed","params":{"threadId":thread_id,"turnId":compact_turn,"item":item}})
        if pending is None:
            emit({"method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":compact_turn,"status":"completed"}}})
    if method == "thread/goal/set" and params.get("objective") == "autonomous fixture":
        time.sleep(0.1)
        pending = str(uuid.uuid4())
        turn_text = "native goal continuation"
        emit({"method":"turn/started","params":{"threadId":thread_id,"turn":{"id":pending}}})
        complete(pending, "Goal completed")
    if method == "turn/start":
        text = next((item["text"] for item in params["input"] if item["type"] == "text"), "skill-only")
        turn_text = text
        if text == "usage":
            emit({"method":"thread/tokenUsage/updated", "params":{"threadId":thread_id,
                "tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":30,"outputTokens":7,"totalTokens":107},"modelContextWindow":200000}}})
        save_turn(pending, None, "inProgress")
        emit({"method": "item/completed", "params": {"threadId": thread_id, "turnId": pending,
            "item": {"type": "userMessage", "id": pending + "-user", "content": [{"type": "text", "text": text}]}}})
        if text == "exit":
            sys.exit(0)
        if text == "propose-plan":
            plan = {"type":"plan","id":pending+"-plan","text":"1. Implement the fix\n2. Verify the result"}
            stream_items.append(plan)
            emit({"method":"item/completed","params":{"threadId":thread_id,"turnId":pending,"item":plan}})
            complete(pending, "Plan ready")
            continue
        if text == "plan-progress":
            emit({"method":"turn/plan/updated","params":{"threadId":thread_id,"turnId":pending,
                "plan":[{"step":"First step","status":"completed"},{"step":"Second step","status":"inProgress"}]}})
            complete(pending, "Progress reported")
            continue
        if text in ("async-question", "async-question-running"):
            question = {"type": "agentMessage", "id": pending + "-question", "delivery": "async",
                "questions": [{"title": "Which runtime?", "options": ["Rust", "Python"]}]}
            stream_items.append(question)
            emit({"method": "item/completed", "params": {"threadId": thread_id, "turnId": pending, "item": question}})
            if text == "async-question":
                complete(pending, "Work continued")
            continue
        if text == "live-subagent":
            child = "child-" + thread_id
            spawn = {"type":"collabAgentToolCall", "id":"spawn-child", "senderThreadId":thread_id,
                "receiverThreadIds":[child], "tool":"spawnAgent", "status":"completed", "prompt":"inspect",
                "agentsStates":{child:{"status":"running"}}}
            stream_items.append(spawn)
            emit({"method":"item/completed", "params":{"threadId":thread_id,"turnId":pending,"item":spawn}})
            (root / ("native-session-" + child + ".json")).write_text(json.dumps({"parentThreadId":thread_id}))
            emit({"method":"item/agentMessage/delta","params":{"threadId":child,"turnId":"child-turn","itemId":"child-answer","delta":"Independent child"}})
            child_item = {"type":"agentMessage", "id":"child-answer", "text":"Independent child answer"}
            emit({"method":"item/completed", "params":{"threadId":child,"turnId":"child-turn","item":child_item}})
            emit({"method":"turn/completed", "params":{"threadId":child,"turn":{"id":"child-turn","status":"completed"}}})
            (root / ("native-history-" + child + ".json")).write_text(json.dumps([{"id":"child-turn","status":"completed","startedAt":1700000000,"items":[child_item]}]))
            complete(pending, "Parent only")
            continue
        if text == "approval":
            emit({"id": "approval-1", "method": "item/commandExecution/requestApproval", "params": {}})
        elif text in ("permit-command", "permit-file", "permit-question"):
            approval_method = {"permit-command": "item/commandExecution/requestApproval", "permit-file": "item/fileChange/requestApproval", "permit-question": "item/tool/requestUserInput"}[text]
            approval = {"threadId": thread_id, "turnId": pending, "itemId": "approval-item", "command": "offline command", "cwd": str(root)}
            if text == "permit-question":
                approval["questions"] = [{"id": "choice", "header": "Choice", "question": "Select one", "options": [{"label": "first", "description": "First choice"}]}]
            emit({"id": "approval-1", "method": approval_method, "params": approval})
        elif text == "child":
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            (root / "child.pid").write_text(str(child.pid))
        elif text == "stream":
            for method_name, item_id, delta, extra in [
                ("item/reasoning/summaryTextDelta", "thought", "Think", {"summaryIndex": 0}),
                ("item/reasoning/summaryTextDelta", "thought", "ing", {"summaryIndex": 0}),
            ]:
                emit({"method": method_name, "params": {"threadId": thread_id, "turnId": pending,
                    "itemId": item_id, "delta": delta, **extra}})
            reasoning = {"type": "reasoning", "id": "thought", "summary": ["Thinking"]}
            stream_items.append(reasoning)
            emit({"method": "item/completed", "params": {"threadId": thread_id, "turnId": pending, "item": reasoning}})
            tool = {"type": "commandExecution", "id": "command", "command": "offline", "status": "inProgress"}
            emit({"method": "item/started", "params": {"threadId": thread_id, "turnId": pending, "item": tool}})
            emit({"method": "item/commandExecution/outputDelta", "params": {"threadId": thread_id,
                "turnId": pending, "itemId": "command", "delta": "offline output"}})
            tool = {**tool, "status": "completed", "aggregatedOutput": "offline output"}
            stream_items.append(tool)
            emit({"method": "item/completed", "params": {"threadId": thread_id, "turnId": pending, "item": tool}})
            for source_thread, source_turn, delta in [("foreign", pending, "FOREIGN"),
                    (thread_id, "stale", "STALE"), (thread_id, pending, "Echo: ")]:
                emit({"method": "item/agentMessage/delta", "params": {"threadId": source_thread,
                    "turnId": source_turn, "itemId": pending + "-assistant", "delta": delta}})
        elif text != "hang":
            timer = threading.Timer(0.05, complete, args=(pending, text))
            timer.daemon = True
            timer.start()
    elif method == "turn/steer":
        text = params["input"][0]["text"]
        user = {"type": "userMessage", "id": str(uuid.uuid4()), "content": [{"type": "text", "text": text}]}
        steered_inputs.append(user)
        emit({"method": "item/completed", "params": {"threadId": thread_id, "turnId": pending, "item": user}})
        if mode != "steer-wrong-id":
            complete(pending, "stream + " + text if turn_text == "stream" else text)
    elif method == "turn/interrupt":
        save_turn(params["turnId"], None, "interrupted")
        pending = None
        emit({"method": "turn/completed", "params": {
            "threadId": thread_id, "turn": {"id": params["turnId"], "status": "interrupted"}}})
