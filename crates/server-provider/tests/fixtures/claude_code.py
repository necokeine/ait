#!/usr/bin/env python3
"""Offline Claude Code stream-json peer; no model, credentials, or external tools."""
import json
import os
import sys
import uuid
from pathlib import Path

args = sys.argv[1:]
cwd = Path.cwd()
config = Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude")))
session = next((arg.split("=", 1)[1] for arg in args if arg.startswith(("--session-id=", "--resume="))), str(uuid.uuid4()))
project = "".join(c if c.isascii() and c.isalnum() else "-" for c in str(cwd.resolve()))
history = config / "projects" / project / (session + ".jsonl")
model = next((arg.split("=", 1)[1] for arg in args if arg.startswith("--model=")), "sonnet")
pending = None
timestamp = "2026-09-26T00:00:00Z"

def emit(value):
    print(json.dumps(value), flush=True)

def save(value):
    if "--no-session-persistence" in args:
        return
    history.parent.mkdir(parents=True, exist_ok=True)
    record = dict(value, cwd=str(cwd), sessionId=session, timestamp=timestamp)
    with history.open("a") as output:
        output.write(json.dumps(record) + "\n")

def assistant(content, mid=None):
    record = {"type":"assistant", "uuid":str(uuid.uuid4()), "session_id":session,
        "message":{"id":mid or str(uuid.uuid4()), "model":model, "content":content, "stop_reason":"end_turn"}}
    save(record)
    emit(record)

def finish(text, with_usage=False):
    mid = str(uuid.uuid4())
    for event in [
        {"type":"message_start", "message":{"id":mid, **({"usage":{"input_tokens":10,"cache_read_input_tokens":20}} if with_usage else {})}},
        {"type":"content_block_start", "index":0, "content_block":{"type":"text", "text":""}},
        {"type":"content_block_delta", "index":0, "delta":{"type":"text_delta", "text":text}},
        {"type":"content_block_stop", "index":0},
    ]:
        emit({"type":"stream_event", "session_id":session, "event":event})
    assistant([{"type":"text", "text":text}], mid)
    emit({"type":"result", "subtype":"success", "is_error":False, "session_id":session, "result":text,
        **({"usage":{"input_tokens":10,"cache_read_input_tokens":20,"output_tokens":5},"total_cost_usd":0.02,"modelUsage":{"fixture":{"contextWindow":200000}}} if with_usage else {})})

for line in sys.stdin:
    message = json.loads(line)
    if message["type"] == "control_request":
        kind = message["request"]["subtype"]
        if kind == "initialize":
            (cwd / "claude-args.json").write_text(json.dumps(args))
            (cwd / "claude-pid").write_text(str(os.getpid()))
            if (cwd / "fail-init").exists():
                sys.exit(1)
            if (cwd / "timeout-init").exists():
                continue
            result = {"models":[
                {"value":"sonnet", "displayName":"Sonnet", "supportedEffortLevels":["low", "high"]},
                {"value":"default", "displayName":"Default"}],
                "commands":[{"name":"review", "description":"Review changes", "argumentHint":"file"}]}
        elif kind == "rewind_files":
            (cwd / "tracked.txt").write_text("checkpoint restored")
            result = {"canRewind":True}
        else:
            result = {}
        emit({"type":"control_response", "response":{"subtype":"success", "request_id":message["request_id"], "response":result}})
    elif message["type"] == "user":
        if message.get("priority") == "next":
            emit({"type":"result", "subtype":"success", "is_error":False, "session_id":session, "result":"Earlier turn"})
        emit(message)
        save(message)
        prompt = message["message"]["content"]
        if isinstance(prompt, list):
            prompt = " ".join(block.get("text", "[Image attachment]") for block in prompt)
        emit({"type":"system", "subtype":"init", "session_id":session, "model":model})
        if prompt == "rotate-session":
            session = str(uuid.uuid4())
            history = config / "projects" / project / (session + ".jsonl")
            emit({"type":"system", "subtype":"init", "session_id":session, "model":model})
            finish("Claude: rotated")
            continue
        if prompt == "hold":
            continue
        if prompt == "exit":
            sys.exit(1)
        if prompt == "malformed":
            print('{broken', flush=True)
            continue
        if prompt == "oversized":
            print('x' * (2 * 1024 * 1024), flush=True)
            continue
        if prompt == "wrong-session":
            emit({"type":"result", "session_id":"wrong", "subtype":"success"})
            continue
        if prompt == "error-result":
            emit({"type":"result", "session_id":session, "subtype":"error_during_execution", "is_error":True})
            continue
        if prompt == "structured":
            assistant([{"type":"text","text":"Preparing structured output"}])
            emit({"type":"result", "uuid":str(uuid.uuid4()), "subtype":"success", "is_error":False,
                "session_id":session,"structured_output":{"answer":42}})
            continue
        if prompt == "autonomous":
            finish("Foreground finished")
            finish("Autonomous follow-up")
            continue
        if prompt == "live-subagent":
            call = "child-call"
            task = "child-task"
            assistant([{"type":"tool_use","id":call,"name":"Agent","input":{"name":"Reviewer","prompt":"Review child task"}}])
            emit({"type":"system","subtype":"task_started","session_id":session,"task_id":task,"tool_use_id":call,
                "task_type":"local_agent","subagent_type":"Explore","prompt":"Review child task"})
            emit({"type":"system","subtype":"task_updated","session_id":session,"task_id":task,"patch":{"is_backgrounded":True}})
            result = {"type":"user","uuid":str(uuid.uuid4()),"session_id":session,
                "message":{"content":[{"type":"tool_result","tool_use_id":call,"content":"Task running in background"}]}}
            save(result)
            emit(result)
            finish("Parent finished")
            emit({"type":"assistant","uuid":"child-message","session_id":session,"parent_tool_use_id":call,
                "message":{"id":"child-answer","model":model,"content":[{"type":"text","text":"Background child answer"}]}})
            emit({"type":"system","subtype":"task_notification","session_id":session,"task_id":task,"tool_use_id":call,"status":"completed"})
            continue
        if prompt == "unknown-control":
            emit({"type":"control_request", "request_id":"unknown", "request":{"subtype":"unsupported"}})
            continue
        if prompt in ("permission", "question"):
            tool = "AskUserQuestion" if prompt == "question" else "Bash"
            tool_input = {"questions":[{"question":"Which color?", "header":"Color", "options":[{"label":"Blue", "description":"Blue"}], "multiSelect":False}]} if prompt == "question" else {"command":"echo fixture"}
            call = str(uuid.uuid4())
            pending = (call, prompt)
            assistant([{"type":"tool_use", "id":call, "name":tool, "input":tool_input}])
            emit({"type":"control_request", "request_id":"permission", "request":{"subtype":"can_use_tool", "tool_name":tool, "input":tool_input, "tool_use_id":call}})
            continue
        finish("Claude: " + prompt, prompt == "usage")
    elif message["type"] == "control_response" and pending:
        call, prompt = pending
        pending = None
        response = message["response"]["response"]
        allowed = response["behavior"] == "allow"
        if allowed and prompt == "question":
            assert response["updatedInput"]["answers"] == {"Which color?":"Blue"}
            assert "allowOther" not in response["updatedInput"]["questions"][0]
        record = {"type":"user", "uuid":str(uuid.uuid4()), "session_id":session,
            "message":{"role":"user", "content":[{"type":"tool_result", "tool_use_id":call, "content":"ok" if allowed else "denied", "is_error":not allowed}]}}
        save(record)
        emit(record)
        finish("Allowed" if allowed else "Denied")
