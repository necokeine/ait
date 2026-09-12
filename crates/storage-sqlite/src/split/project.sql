PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
CREATE TABLE project_identity (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  project_id TEXT NOT NULL UNIQUE, coordinator_id TEXT NOT NULL,
  last_operation TEXT
) STRICT;
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES project_identity(project_id),
  parent_message_id TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX messages_project_parent ON messages(project_id, parent_message_id);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project_identity(project_id),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX sessions_project ON sessions(project_id);
CREATE TABLE runs (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project_identity(project_id),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX runs_project ON runs(project_id);
CREATE INDEX runs_session ON runs(json_extract(body_json, '$.session_id'));
CREATE INDEX runs_cron ON runs(json_extract(body_json, '$.cron_id'));
CREATE TABLE run_credentials (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project_identity(project_id),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX run_credentials_project ON run_credentials(project_id);
CREATE TABLE workspace_run_journals (
  id TEXT PRIMARY KEY REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED,
  project_id TEXT NOT NULL REFERENCES project_identity(project_id),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX workspace_run_journals_project ON workspace_run_journals(project_id);
CREATE TRIGGER messages_immutable_update BEFORE UPDATE ON messages BEGIN
  SELECT RAISE(ABORT, 'messages are immutable');
END;
CREATE TRIGGER messages_immutable_delete BEFORE DELETE ON messages BEGIN
  SELECT RAISE(ABORT, 'messages are immutable');
END;
CREATE TABLE durable_events (
  cursor INTEGER PRIMARY KEY, kind TEXT NOT NULL, entity_id TEXT,
  body_json TEXT NOT NULL CHECK (json_valid(body_json)), created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE run_progress (
  run_id TEXT PRIMARY KEY, body_json TEXT NOT NULL CHECK (json_valid(body_json)),
  updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE prepared_commit (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1), operation_id TEXT NOT NULL,
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
