#!/usr/bin/env python3
"""Offline stdio protocol peer. Never invokes Codex, a network API, or a model."""
import json
import os
from pathlib import Path
import sys
import subprocess
import threading
import time
import uuid

root = Path.cwd()
mode = (root / "behavior").read_text() if (root / "behavior").exists() else "normal"
output_lock = threading.Lock()
pending = None
thread_id = None

def emit(value):
    with output_lock:
        sys.stdout.write(json.dumps(value) + "\n")
        sys.stdout.flush()

def complete(turn_id, text):
    global pending
    if pending != turn_id:
        return
    emit({"method": "item/completed", "params": {
        "threadId": thread_id, "turnId": turn_id,
        "item": {"type": "agentMessage", "text": "Echo: " + text}}})
    emit({"method": "turn/completed", "params": {
        "threadId": thread_id, "turn": {"id": turn_id, "status": "failed" if text == "fail" else "completed"}}})
    pending = None

for line in sys.stdin:
    request = json.loads(line)
    with (root / "native-requests.jsonl").open("a") as log:
        log.write(json.dumps({"pid": os.getpid(), **request}) + "\n")
    method = request.get("method")
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
    if method in ("thread/start", "thread/resume", "thread/read"):
        thread_id = params.get("threadId", str(uuid.uuid4()))
        if mode == "wrong-thread":
            thread_id = "wrong-thread"
        result = {"thread": {"id": thread_id}, "model": "offline-model"}
        if mode == "missing-thread":
            result = {}
    elif method == "turn/start":
        pending = str(uuid.uuid4())
        result = {"turn": {"id": pending}}
    emit({"id": request["id"], "result": result})
    if method == "turn/start":
        text = params["input"][0]["text"]
        if text == "exit":
            sys.exit(0)
        if text == "approval":
            emit({"id": "approval-1", "method": "item/commandExecution/requestApproval", "params": {}})
        elif text == "child":
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            (root / "child.pid").write_text(str(child.pid))
        elif text != "hang":
            timer = threading.Timer(0.05, complete, args=(pending, text))
            timer.daemon = True
            timer.start()
    elif method == "turn/interrupt":
        pending = None
        emit({"method": "turn/completed", "params": {
            "threadId": thread_id, "turn": {"id": params["turnId"], "status": "interrupted"}}})
