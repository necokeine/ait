.bail on

-- Preserved from the accepted design work in NEC-146.
-- Run from a disposable directory. It creates two files mirroring the POC
-- topology, then ATTACHes them only for cross-boundary audit assertions.
.open split-smoke-global.sqlite3
.read schema-global-poc-v3.sql

INSERT INTO projects(
  id, name, root_path, git_initialized_by_manager, instruction_revision,
  instruction_digest, created_at_ms, updated_at_ms
) VALUES (
  'p1', 'Split POC', '/tmp/metafab-poc-project', 1, 1,
  lower(hex(zeroblob(32))), 1, 1
);
INSERT INTO agents(id, name, created_at_ms, updated_at_ms)
VALUES ('a1', 'mock-agent', 1, 1);
INSERT INTO agent_revisions(
  agent_id, revision, driver_type, connection_name, model, capabilities_json,
  default_parameters_json, tool_policy_json, config_digest, created_at_ms
) VALUES (
  'a1', 1, 'mock', 'local_mock', 'mock-v1',
  '{"streaming":true,"tools":true}', '{}', '{}', lower(hex(zeroblob(32))), 1
);
INSERT INTO project_agent_defaults(project_id, agent_id) VALUES ('p1', 'a1');

.open split-smoke-project.sqlite3
.read schema-project-poc-v3.sql

INSERT INTO project_identity(singleton, project_id, created_at_ms, format_version)
VALUES (1, 'p1', 1, 1);
INSERT INTO project_instruction_revisions(
  revision, source_manifest_json, rendered_prompt, content_digest, created_at_ms
) VALUES (1, '[]', 'You are a local agent.', lower(hex(zeroblob(32))), 1);
INSERT INTO attachments(
  id, filename, media_type, byte_length, sha256, data, created_at_ms
) VALUES ('att1', 'note.txt', 'text/plain', 4, lower(hex(randomblob(32))), x'6e6f7465', 2);

INSERT INTO messages(
  id, parent_message_id, role, message_kind, origin,
  content_json, content_digest, created_at_ms
) VALUES (
  'm0', NULL, 'system', 'standard', 'project',
  '[{"type":"text","text":"You are a local agent."}]', lower(hex(randomblob(32))), 2
);
INSERT INTO sessions(id, name, current_message_id, version, created_at_ms, updated_at_ms)
VALUES ('s1', 'main', 'm0', 1, 3, 3);
INSERT INTO messages(
  id, parent_message_id, role, message_kind, origin, content_json,
  content_digest, created_by_session_id, created_at_ms
) VALUES (
  'm1', 'm0', 'user', 'standard', 'human',
  '[{"type":"text","text":"read this"},{"type":"file_ref","attachment_id":"att1"}]',
  lower(hex(randomblob(32))), 's1', 4
);
INSERT INTO message_attachments(message_id, part_index, attachment_id)
VALUES ('m1', 1, 'att1');
UPDATE sessions SET current_message_id = 'm1', version = version + 1, updated_at_ms = 4
 WHERE id = 's1' AND version = 1 AND current_message_id = 'm0';

INSERT INTO runs(
  id, base_message_id, follow_session_id, agent_id, agent_revision,
  agent_snapshot_json, agent_snapshot_digest, trigger_kind, status, phase,
  max_steps, retry_policy_json, started_at_ms, created_at_ms, updated_at_ms
) VALUES (
  'r1', 'm1', 's1', 'a1', 1,
  '{"agent_id":"a1","revision":1,"driver_type":"mock","connection_name":"local_mock","model":"mock-v1"}',
  lower(hex(zeroblob(32))), 'manual', 'running', 'calling_agent',
  10, '{}', 5, 5, 5
);
INSERT INTO run_attempts(id, run_id, attempt_no, reason, status, started_at_ms)
VALUES ('ra1', 'r1', 1, 'initial', 'running', 5);
INSERT INTO messages(
  id, parent_message_id, role, message_kind, origin, content_json,
  content_digest, created_by_session_id, run_id, run_seq, created_at_ms
) VALUES (
  'm2', 'm1', 'assistant', 'standard', 'agent',
  '[{"type":"text","text":"checking"},{"type":"tool_use","call_id":"call-1","tool_name":"read_file","arguments":{"path":"note.txt"}}]',
  lower(hex(randomblob(32))), 's1', 'r1', 1, 6
);
UPDATE sessions SET current_message_id = 'm2', version = version + 1, updated_at_ms = 6
 WHERE id = 's1' AND active_run_id = 'r1' AND current_message_id = 'm1';
UPDATE runs SET last_message_id = 'm2', step_count = 1, updated_at_ms = 6
 WHERE id = 'r1' AND last_message_id IS NULL;
INSERT INTO tool_executions(
  id, run_id, call_id, assistant_message_id, tool_use_index, tool_name,
  arguments_json, attempt, approval_status, status, result_json,
  started_at_ms, ended_at_ms, created_at_ms
) VALUES (
  'te1', 'r1', 'call-1', 'm2', 1, 'read_file', '{"path":"note.txt"}',
  1, 'not_required', 'succeeded', '{"text":"note"}', 7, 7, 7
);
INSERT INTO messages(
  id, parent_message_id, role, message_kind, origin, content_json,
  content_digest, created_by_session_id, run_id, run_seq,
  tool_result_call_id, tool_result_status, created_at_ms
) VALUES (
  'm3', 'm2', 'user', 'tool_result', 'tool',
  '[{"type":"tool_result","call_id":"call-1","status":"succeeded","result":{"text":"note"}}]',
  lower(hex(randomblob(32))), 's1', 'r1', 2, 'call-1', 'succeeded', 8
);
UPDATE sessions SET current_message_id = 'm3', version = version + 1, updated_at_ms = 8
 WHERE id = 's1' AND active_run_id = 'r1' AND current_message_id = 'm2';
UPDATE runs SET last_message_id = 'm3', step_count = 2,
  usage_json = '{"input_tokens":10,"output_tokens":3}', updated_at_ms = 8
 WHERE id = 'r1' AND last_message_id = 'm2';
INSERT INTO run_events(run_id, seq, event_type, payload_json, created_at_ms)
VALUES ('r1', 1, 'message_committed', '{"message_id":"m3"}', 8);
UPDATE run_attempts SET status = 'completed', ended_at_ms = 9 WHERE id = 'ra1';
UPDATE runs SET status = 'completed', phase = 'settled', stop_reason = 'end_turn',
  ended_at_ms = 9, updated_at_ms = 9 WHERE id = 'r1';

-- Simulate the cross-DB Cron saga: claim globally, idempotently create the
-- local Run, then acknowledge its local ID globally.
.open split-smoke-global.sqlite3
ATTACH DATABASE 'split-smoke-project.sqlite3' AS project_runtime;
INSERT INTO crons(
  id, name, project_id, base_message_id, agent_id, schedule, timezone,
  enabled, concurrency_policy, misfire_policy, next_run_at_ms, created_at_ms, updated_at_ms
) VALUES (
  'c1', 'daily', 'p1', 'm3', 'a1', '0 9 * * *', 'Asia/Shanghai',
  1, 'forbid', 'run_once', 100, 10, 10
);
INSERT INTO cron_fires(
  cron_id, scheduled_at_ms, project_id, state, claimed_at_ms, updated_at_ms
) VALUES ('c1', 100, 'p1', 'claimed', 100, 100);
INSERT INTO project_runtime.runs(
  id, base_message_id, agent_id, agent_revision, agent_snapshot_json,
  agent_snapshot_digest, trigger_kind, cron_id, scheduled_at_ms, status, phase,
  max_steps, retry_policy_json, dedupe_key, created_at_ms, updated_at_ms
) VALUES (
  'r-cron-1', 'm3', 'a1', 1,
  '{"agent_id":"a1","revision":1,"driver_type":"mock","connection_name":"local_mock","model":"mock-v1"}',
  lower(hex(zeroblob(32))), 'cron', 'c1', 100, 'queued', 'queued',
  10, '{}', 'cron:c1:100', 100, 100
);
UPDATE cron_fires SET state = 'started', local_run_id = 'r-cron-1', updated_at_ms = 101
 WHERE cron_id = 'c1' AND scheduled_at_ms = 100 AND state = 'claimed';

CREATE TEMP TABLE smoke_assertion(ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO smoke_assertion(ok)
SELECT (SELECT project_id FROM project_runtime.project_identity WHERE singleton = 1) = 'p1'
   AND (SELECT root_path FROM projects WHERE id = 'p1') = '/tmp/metafab-poc-project'
   AND EXISTS (SELECT 1 FROM project_runtime.messages WHERE id = 'm3')
   AND (SELECT current_message_id FROM project_runtime.sessions WHERE id = 's1') = 'm3'
   AND (SELECT data FROM project_runtime.attachments WHERE id = 'att1') = x'6e6f7465'
   AND EXISTS (
     SELECT 1 FROM crons c JOIN project_runtime.messages m ON m.id = c.base_message_id
      WHERE c.id = 'c1' AND c.project_id = 'p1'
   )
   AND EXISTS (
     SELECT 1 FROM cron_fires f JOIN project_runtime.runs r ON r.id = f.local_run_id
      WHERE f.cron_id = 'c1' AND f.scheduled_at_ms = 100
        AND f.state = 'started' AND r.dedupe_key = 'cron:c1:100'
   )
   AND NOT EXISTS (
     SELECT 1 FROM sqlite_schema WHERE type = 'table' AND lower(name) LIKE '%secret%'
   )
   AND NOT EXISTS (
     SELECT 1 FROM project_runtime.sqlite_schema WHERE type = 'table' AND lower(name) LIKE '%secret%'
   );
SELECT 'split_sqlite_poc_smoke_ok';
PRAGMA main.foreign_key_check;
PRAGMA project_runtime.foreign_key_check;
PRAGMA main.integrity_check;
PRAGMA project_runtime.integrity_check;
DETACH DATABASE project_runtime;
