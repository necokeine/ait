PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS control_metadata (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1), revision INTEGER NOT NULL
) STRICT;
INSERT OR IGNORE INTO control_metadata VALUES (1, 0);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL CHECK (project_id = id),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE UNIQUE INDEX IF NOT EXISTS projects_workdir_unique ON projects(json_extract(body_json, '$.workdir'));
CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY, project_id TEXT, body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX IF NOT EXISTS agents_provider ON agents(json_extract(body_json, '$.config.provider_id'));
CREATE TABLE IF NOT EXISTS agent_providers (
  id TEXT PRIMARY KEY, project_id TEXT, body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE TABLE IF NOT EXISTS provider_credentials (
  id TEXT PRIMARY KEY, project_id TEXT, body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE TABLE IF NOT EXISTS crons (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE INDEX IF NOT EXISTS crons_project ON crons(project_id);
CREATE TABLE IF NOT EXISTS settings (
  id TEXT PRIMARY KEY CHECK (id = 'settings'), project_id TEXT,
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
CREATE TABLE IF NOT EXISTS durable_events (
  cursor INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, entity_id TEXT,
  body_json TEXT NOT NULL CHECK (json_valid(body_json)), created_at INTEGER NOT NULL,
  project_id TEXT
) STRICT;
