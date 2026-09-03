-- NEC-146 POC v3: per-Project runtime database.
-- Preserved from the accepted design work in NEC-146.
-- Location: <project-root>/.metafab/project.sqlite3

PRAGMA application_id = 0x4d465031; -- "MFP1"
PRAGMA user_version = 3;
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE schema_migrations (
  version       INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  sha256        TEXT NOT NULL CHECK (length(sha256) = 64),
  app_version   TEXT NOT NULL,
  applied_at_ms INTEGER NOT NULL
) STRICT;

-- Exactly one row. This lets a moved/copied DB identify its Project without
-- depending on the global registry path.
CREATE TABLE project_identity (
  singleton       INTEGER PRIMARY KEY CHECK (singleton = 1),
  project_id      TEXT NOT NULL UNIQUE,
  created_at_ms   INTEGER NOT NULL,
  format_version  INTEGER NOT NULL CHECK (format_version >= 1)
) STRICT;

CREATE TABLE project_instruction_revisions (
  revision             INTEGER PRIMARY KEY CHECK (revision >= 1),
  source_manifest_json TEXT NOT NULL CHECK (json_valid(source_manifest_json)),
  component_json       TEXT NOT NULL CHECK (json_valid(component_json)),
  content_digest       TEXT NOT NULL CHECK (length(content_digest) = 64),
  created_at_ms        INTEGER NOT NULL
) STRICT;

CREATE TABLE attachments (
  id            TEXT PRIMARY KEY,
  filename      TEXT,
  media_type    TEXT NOT NULL,
  byte_length   INTEGER NOT NULL CHECK (byte_length >= 0),
  sha256        TEXT NOT NULL CHECK (length(sha256) = 64),
  data          BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE messages (
  -- Canonical hyphenated UUID; enforced by the domain type before persistence.
  id                    TEXT PRIMARY KEY,
  parent_message_id     TEXT,
  role                  TEXT NOT NULL CHECK (role IN ('user', 'system', 'assistant')),
  message_kind          TEXT NOT NULL DEFAULT 'standard'
    CHECK (message_kind IN ('standard', 'tool_result')),
  origin                TEXT NOT NULL
    CHECK (origin IN ('project', 'human', 'agent', 'tool', 'scheduler', 'system')),
  content_json          TEXT NOT NULL CHECK (json_valid(content_json) AND json_type(content_json) = 'array'),
  content_digest        TEXT NOT NULL CHECK (length(content_digest) = 64),
  created_by_session_id TEXT,
  run_id                TEXT,
  run_seq               INTEGER,
  tool_result_call_id   TEXT,
  tool_result_status    TEXT
    CHECK (tool_result_status IN ('succeeded', 'failed', 'denied', 'cancelled')),
  metadata_json         TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  created_at_ms         INTEGER NOT NULL,
  CHECK (parent_message_id IS NOT NULL OR role = 'system'),
  CHECK ((run_id IS NULL AND run_seq IS NULL) OR (run_id IS NOT NULL AND run_seq >= 1)),
  CHECK (
    (message_kind = 'standard' AND tool_result_call_id IS NULL AND tool_result_status IS NULL) OR
    (message_kind = 'tool_result' AND role = 'user' AND origin = 'tool'
      AND run_id IS NOT NULL AND tool_result_call_id IS NOT NULL AND tool_result_status IS NOT NULL)
  ),
  UNIQUE (run_id, run_seq),
  UNIQUE (run_id, tool_result_call_id),
  FOREIGN KEY (parent_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (created_by_session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE message_attachments (
  message_id    TEXT NOT NULL,
  part_index    INTEGER NOT NULL CHECK (part_index >= 0),
  attachment_id TEXT NOT NULL,
  PRIMARY KEY (message_id, part_index),
  FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (attachment_id) REFERENCES attachments(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE sessions (
  id                 TEXT PRIMARY KEY,
  name               TEXT NOT NULL,
  title              TEXT,
  current_message_id TEXT NOT NULL,
  active_run_id      TEXT,
  agent_id           TEXT NOT NULL,
  status             TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
  version            INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL,
  FOREIGN KEY (current_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (active_run_id) REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- agent_id/revision and cron_id are soft references to the global DB. The full
-- non-secret Agent revision is snapshotted here so copied Project history is
-- readable and reproducible without the global catalog.
CREATE TABLE runs (
  id                    TEXT PRIMARY KEY,
  base_message_id       TEXT NOT NULL,
  last_message_id       TEXT,
  follow_session_id     TEXT,
  agent_id              TEXT NOT NULL,
  agent_revision        INTEGER NOT NULL CHECK (agent_revision >= 1),
  agent_snapshot_json   TEXT NOT NULL CHECK (json_valid(agent_snapshot_json)),
  agent_snapshot_digest TEXT NOT NULL CHECK (length(agent_snapshot_digest) = 64),
  trigger_kind          TEXT NOT NULL CHECK (trigger_kind IN ('manual', 'cron')),
  cron_id               TEXT,
  scheduled_at_ms       INTEGER,
  status                TEXT NOT NULL
    CHECK (status IN ('queued', 'running', 'waiting_approval', 'retry_wait', 'settling', 'completed', 'failed', 'cancelled', 'limit_exceeded')),
  phase                 TEXT NOT NULL,
  stop_reason           TEXT,
  error_json            TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
  step_count            INTEGER NOT NULL DEFAULT 0 CHECK (step_count >= 0),
  max_steps             INTEGER NOT NULL CHECK (max_steps > 0),
  token_budget          INTEGER CHECK (token_budget IS NULL OR token_budget >= 0),
  cost_budget_micros    INTEGER CHECK (cost_budget_micros IS NULL OR cost_budget_micros >= 0),
  usage_json            TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(usage_json)),
  attempt_count         INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  compaction_count      INTEGER NOT NULL DEFAULT 0 CHECK (compaction_count >= 0),
  retry_policy_json     TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(retry_policy_json)),
  next_retry_at_ms      INTEGER,
  queue_version         INTEGER NOT NULL DEFAULT 0 CHECK (queue_version >= 0),
  queue_cursor          INTEGER NOT NULL DEFAULT 0 CHECK (queue_cursor >= 0),
  dedupe_key            TEXT UNIQUE,
  started_at_ms         INTEGER,
  ended_at_ms           INTEGER,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  CHECK ((trigger_kind = 'manual' AND cron_id IS NULL AND scheduled_at_ms IS NULL) OR
         (trigger_kind = 'cron' AND cron_id IS NOT NULL AND scheduled_at_ms IS NOT NULL)),
  CHECK ((status IN ('completed', 'failed', 'cancelled', 'limit_exceeded') AND ended_at_ms IS NOT NULL) OR
         (status NOT IN ('completed', 'failed', 'cancelled', 'limit_exceeded') AND ended_at_ms IS NULL)),
  FOREIGN KEY (base_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (last_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (follow_session_id) REFERENCES sessions(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE run_attempts (
  id            TEXT PRIMARY KEY,
  run_id        TEXT NOT NULL,
  attempt_no    INTEGER NOT NULL CHECK (attempt_no >= 1),
  reason        TEXT NOT NULL CHECK (reason IN ('initial', 'retry', 'recovery')),
  status        TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
  error_json    TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
  started_at_ms INTEGER NOT NULL,
  ended_at_ms   INTEGER,
  UNIQUE (run_id, attempt_no),
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE run_queue_items (
  id             TEXT PRIMARY KEY,
  run_id         TEXT NOT NULL,
  seq            INTEGER NOT NULL CHECK (seq >= 1),
  kind           TEXT NOT NULL,
  payload_json   TEXT NOT NULL CHECK (json_valid(payload_json)),
  dedupe_key     TEXT,
  status         TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'consumed', 'rejected')),
  created_at_ms  INTEGER NOT NULL,
  consumed_at_ms INTEGER,
  UNIQUE (run_id, seq),
  UNIQUE (run_id, dedupe_key),
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE run_checkpoints (
  id               TEXT PRIMARY KEY,
  run_id           TEXT NOT NULL,
  attempt_no       INTEGER NOT NULL,
  after_message_id TEXT NOT NULL,
  format_version   INTEGER NOT NULL CHECK (format_version >= 1),
  state_blob       BLOB NOT NULL,
  sha256           TEXT NOT NULL CHECK (length(sha256) = 64),
  created_at_ms    INTEGER NOT NULL,
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT,
  FOREIGN KEY (after_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  UNIQUE (run_id, id)
) STRICT;

CREATE TABLE run_branch_conflicts (
  id                          TEXT PRIMARY KEY,
  run_id                      TEXT NOT NULL,
  message_id                  TEXT NOT NULL UNIQUE,
  expected_session_message_id TEXT NOT NULL,
  observed_session_message_id TEXT NOT NULL,
  observed_session_version    INTEGER NOT NULL CHECK (observed_session_version >= 1),
  status                      TEXT NOT NULL CHECK (status IN ('pending', 'adopted', 'abandoned')),
  created_at_ms               INTEGER NOT NULL,
  resolved_at_ms              INTEGER,
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT,
  FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (expected_session_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (observed_session_message_id) REFERENCES messages(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE tool_executions (
  id                     TEXT PRIMARY KEY,
  run_id                 TEXT NOT NULL,
  call_id                TEXT NOT NULL,
  assistant_message_id   TEXT NOT NULL,
  tool_use_index         INTEGER NOT NULL CHECK (tool_use_index >= 0),
  tool_result_message_id TEXT,
  tool_name              TEXT NOT NULL,
  arguments_json         TEXT NOT NULL CHECK (json_valid(arguments_json)),
  attempt                INTEGER NOT NULL CHECK (attempt >= 1),
  approval_status        TEXT NOT NULL CHECK (approval_status IN ('not_required', 'pending', 'approved', 'denied')),
  status                 TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'denied', 'cancelled')),
  result_json            TEXT CHECK (result_json IS NULL OR json_valid(result_json)),
  error_json             TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
  started_at_ms          INTEGER,
  ended_at_ms            INTEGER,
  created_at_ms          INTEGER NOT NULL,
  UNIQUE (run_id, call_id, attempt),
  UNIQUE (tool_result_message_id),
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT,
  FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE RESTRICT,
  FOREIGN KEY (tool_result_message_id) REFERENCES messages(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE run_events (
  run_id        TEXT NOT NULL,
  seq           INTEGER NOT NULL CHECK (seq >= 1),
  event_type    TEXT NOT NULL,
  payload_json  TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (run_id, seq),
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX messages_parent_idx ON messages(parent_message_id, created_at_ms, id);
CREATE INDEX messages_created_idx ON messages(created_at_ms, id);
CREATE INDEX messages_run_idx ON messages(run_id, run_seq);
CREATE INDEX sessions_status_idx ON sessions(status, updated_at_ms DESC);
CREATE UNIQUE INDEX runs_one_active_per_session_idx ON runs(follow_session_id)
  WHERE follow_session_id IS NOT NULL AND status NOT IN ('completed', 'failed', 'cancelled', 'limit_exceeded');
CREATE INDEX runs_status_idx ON runs(status, created_at_ms);
CREATE INDEX runs_retry_due_idx ON runs(next_retry_at_ms) WHERE status = 'retry_wait';
CREATE INDEX run_queue_pending_idx ON run_queue_items(run_id, status, seq);
CREATE INDEX run_conflicts_pending_idx ON run_branch_conflicts(run_id, created_at_ms) WHERE status = 'pending';
CREATE INDEX tool_executions_pending_idx ON tool_executions(run_id, status, created_at_ms);
CREATE INDEX run_events_created_idx ON run_events(run_id, created_at_ms);

CREATE TRIGGER messages_no_update BEFORE UPDATE ON messages BEGIN
  SELECT RAISE(ABORT, 'MESSAGE_IMMUTABLE');
END;
CREATE TRIGGER messages_no_delete BEFORE DELETE ON messages BEGIN
  SELECT RAISE(ABORT, 'MESSAGE_IMMUTABLE');
END;
CREATE TRIGGER message_attachments_no_update BEFORE UPDATE ON message_attachments BEGIN
  SELECT RAISE(ABORT, 'MESSAGE_IMMUTABLE');
END;
CREATE TRIGGER message_attachments_no_delete BEFORE DELETE ON message_attachments BEGIN
  SELECT RAISE(ABORT, 'MESSAGE_IMMUTABLE');
END;
CREATE TRIGGER instruction_revisions_no_update BEFORE UPDATE ON project_instruction_revisions BEGIN
  SELECT RAISE(ABORT, 'INSTRUCTION_REVISION_IMMUTABLE');
END;

CREATE TRIGGER messages_parent_exists_before_insert BEFORE INSERT ON messages
WHEN new.parent_message_id IS NOT NULL BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM messages p WHERE p.id = new.parent_message_id
  ) THEN RAISE(ABORT, 'MESSAGE_PARENT_MUST_PREEXIST') END;
END;

CREATE TRIGGER messages_run_path_guard BEFORE INSERT ON messages
WHEN new.run_id IS NOT NULL BEGIN
  SELECT CASE WHEN new.run_seq <> COALESCE((
    SELECT max(m.run_seq) + 1 FROM messages m WHERE m.run_id = new.run_id
  ), 1) THEN RAISE(ABORT, 'RUN_SEQUENCE_CONFLICT') END;
  SELECT CASE WHEN new.parent_message_id IS NOT (
    SELECT coalesce(r.last_message_id, r.base_message_id) FROM runs r WHERE r.id = new.run_id
  ) THEN RAISE(ABORT, 'RUN_PATH_CONFLICT') END;
END;

CREATE TRIGGER message_attachment_guard BEFORE INSERT ON message_attachments BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM messages m JOIN attachments a ON a.id = new.attachment_id
     WHERE m.id = new.message_id
       AND json_extract(m.content_json, '$[' || new.part_index || '].type') = 'file_ref'
       AND json_extract(m.content_json, '$[' || new.part_index || '].attachment_id') = new.attachment_id
  ) THEN RAISE(ABORT, 'MESSAGE_ATTACHMENT_MISMATCH') END;
END;

CREATE TRIGGER sessions_pointer_guard BEFORE UPDATE OF current_message_id ON sessions
WHEN new.current_message_id IS NOT old.current_message_id BEGIN
  SELECT CASE WHEN new.version <> old.version + 1 OR NOT EXISTS (
    SELECT 1 FROM messages m WHERE m.id = new.current_message_id
      AND m.parent_message_id = old.current_message_id
  ) THEN RAISE(ABORT, 'SESSION_POINTER_CONFLICT') END;
END;

CREATE TRIGGER runs_follow_session_guard BEFORE INSERT ON runs
WHEN new.follow_session_id IS NOT NULL BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM sessions s WHERE s.id = new.follow_session_id
      AND s.current_message_id = new.base_message_id AND s.active_run_id IS NULL
      AND s.status = 'active' AND s.agent_id = new.agent_id
  ) THEN RAISE(ABORT, 'SESSION_BUSY_OR_POINTER_CONFLICT') END;
END;
CREATE TRIGGER runs_claim_session AFTER INSERT ON runs
WHEN new.follow_session_id IS NOT NULL BEGIN
  UPDATE sessions SET active_run_id = new.id, version = version + 1, updated_at_ms = new.created_at_ms
   WHERE id = new.follow_session_id AND active_run_id IS NULL AND current_message_id = new.base_message_id;
END;
CREATE TRIGGER runs_release_session AFTER UPDATE OF status ON runs
WHEN new.status IN ('completed', 'failed', 'cancelled', 'limit_exceeded')
 AND old.status NOT IN ('completed', 'failed', 'cancelled', 'limit_exceeded') BEGIN
  UPDATE sessions SET active_run_id = NULL, version = version + 1, updated_at_ms = new.updated_at_ms
   WHERE id = new.follow_session_id AND active_run_id = new.id;
END;

CREATE TRIGGER run_queue_items_bump_version AFTER INSERT ON run_queue_items BEGIN
  UPDATE runs SET queue_version = queue_version + 1, updated_at_ms = new.created_at_ms
   WHERE id = new.run_id;
END;

CREATE TRIGGER tool_execution_content_guard BEFORE INSERT ON tool_executions BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM messages m WHERE m.id = new.assistant_message_id
      AND m.run_id = new.run_id AND m.role = 'assistant'
      AND json_extract(m.content_json, '$[' || new.tool_use_index || '].type') = 'tool_use'
      AND json_extract(m.content_json, '$[' || new.tool_use_index || '].call_id') = new.call_id
      AND json_extract(m.content_json, '$[' || new.tool_use_index || '].tool_name') = new.tool_name
  ) THEN RAISE(ABORT, 'TOOL_USE_NOT_FOUND') END;
END;

CREATE TRIGGER tool_result_message_guard BEFORE INSERT ON messages
WHEN new.message_kind = 'tool_result' BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM tool_executions te WHERE te.run_id = new.run_id
      AND te.call_id = new.tool_result_call_id AND te.tool_result_message_id IS NULL
  ) THEN RAISE(ABORT, 'TOOL_USE_NOT_FOUND_OR_RESULT_DUPLICATE') END;
END;
CREATE TRIGGER tool_result_message_link AFTER INSERT ON messages
WHEN new.message_kind = 'tool_result' BEGIN
  UPDATE tool_executions SET tool_result_message_id = new.id
   WHERE id = (
     SELECT id FROM tool_executions WHERE run_id = new.run_id
       AND call_id = new.tool_result_call_id ORDER BY attempt DESC LIMIT 1
   );
END;

INSERT INTO schema_migrations(version, name, sha256, app_version, applied_at_ms)
VALUES (3, '0003_project_runtime_poc', lower(hex(zeroblob(32))), 'design-reference', 0);
