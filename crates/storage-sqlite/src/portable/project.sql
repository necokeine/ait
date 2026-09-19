CREATE TABLE control_metadata (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), revision INTEGER NOT NULL
) STRICT;
INSERT INTO control_metadata VALUES(1, 0);
CREATE TABLE project_identity (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), project_id TEXT NOT NULL UNIQUE,
  stream_id TEXT NOT NULL, owner_epoch INTEGER NOT NULL, owner_instance TEXT NOT NULL,
  origin_catalog TEXT NOT NULL
) STRICT;
CREATE TABLE projects (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL CHECK(project_id=id),
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE TABLE messages (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  parent_message_id TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE INDEX messages_parent ON messages(project_id, parent_message_id);
CREATE TRIGGER messages_immutable_update BEFORE UPDATE ON messages BEGIN
  SELECT RAISE(ABORT, 'messages are immutable'); END;
CREATE TRIGGER messages_immutable_delete BEFORE DELETE ON messages BEGIN
  SELECT RAISE(ABORT, 'messages are immutable'); END;
CREATE TABLE sessions (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE INDEX sessions_project ON sessions(project_id);
CREATE TABLE runs (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE INDEX runs_project ON runs(project_id);
CREATE INDEX runs_session ON runs(json_extract(body_json, '$.session_id'));
CREATE INDEX runs_cron ON runs(json_extract(body_json, '$.cron_id'));
CREATE TABLE run_credentials (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE TABLE workspace_run_journals (
  id TEXT PRIMARY KEY REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED,
  project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE TABLE agents (
  id TEXT PRIMARY KEY, project_id TEXT, body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE INDEX agents_provider ON agents(json_extract(body_json, '$.config.provider_id'));
CREATE TABLE agent_providers (
  id TEXT PRIMARY KEY, project_id TEXT, body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE TABLE configuration_sources (
  kind TEXT NOT NULL, id TEXT NOT NULL, catalog_id TEXT NOT NULL,
  PRIMARY KEY(kind, id)
) STRICT;
CREATE TABLE configuration_bindings (
  source_agent_id TEXT PRIMARY KEY, catalog_id TEXT NOT NULL, agent_id TEXT NOT NULL,
  agent_json TEXT NOT NULL CHECK(json_valid(agent_json)),
  provider_json TEXT NOT NULL CHECK(json_valid(provider_json))
) STRICT;
CREATE TABLE crons (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK(json_valid(body_json))
) STRICT;
CREATE INDEX crons_project ON crons(project_id);
CREATE TABLE durable_events (
  cursor INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, entity_id TEXT,
  body_json TEXT NOT NULL CHECK(json_valid(body_json)), created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE run_progress (
  run_id TEXT PRIMARY KEY, body_json TEXT NOT NULL CHECK(json_valid(body_json)), updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE commit_receipts (
  operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL, revision INTEGER NOT NULL
) STRICT;
CREATE TABLE projection_changes (
  revision INTEGER NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL, deleted INTEGER NOT NULL,
  PRIMARY KEY(revision, kind, id)
) STRICT;
CREATE TABLE worker_processes (
  pid INTEGER PRIMARY KEY, runtime_instance_id TEXT NOT NULL, owner_epoch INTEGER NOT NULL,
  boot_id TEXT NOT NULL
) STRICT;
CREATE TABLE native_bindings (
  thread_id TEXT PRIMARY KEY,session_id TEXT NOT NULL UNIQUE
) STRICT;
