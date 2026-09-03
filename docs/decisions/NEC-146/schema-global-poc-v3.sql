-- NEC-146 POC v3: global catalog database.
-- Preserved from the accepted design work in NEC-146.
-- Location: <Documents>/metafab/metafab.sqlite3

PRAGMA application_id = 0x4d464731; -- "MFG1"
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

-- Registry and upper-level Project metadata only. Message/Session/Run data is
-- physically stored in <root_path>/.metafab/project.sqlite3.
CREATE TABLE projects (
  id                       TEXT PRIMARY KEY,
  name                     TEXT NOT NULL,
  description              TEXT NOT NULL DEFAULT '',
  root_path                TEXT NOT NULL UNIQUE,
  project_db_relative_path TEXT NOT NULL DEFAULT '.metafab/project.sqlite3'
    CHECK (project_db_relative_path = '.metafab/project.sqlite3'),
  git_initialized_by_manager INTEGER NOT NULL CHECK (git_initialized_by_manager IN (0, 1)),
  instruction_revision     INTEGER NOT NULL DEFAULT 1 CHECK (instruction_revision >= 1),
  instruction_digest       TEXT CHECK (instruction_digest IS NULL OR length(instruction_digest) = 64),
  metadata_json            TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  status                   TEXT NOT NULL DEFAULT 'active'
    CHECK (status IN ('active', 'archived', 'missing', 'unavailable')),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL
) STRICT;

CREATE TABLE agents (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;

-- connection_name selects a non-secret connection block in config.toml.
-- Authentication fields live only in <Documents>/metafab/secrets.toml.
CREATE TABLE agent_revisions (
  agent_id                TEXT NOT NULL,
  revision                INTEGER NOT NULL CHECK (revision >= 1),
  driver_type             TEXT NOT NULL,
  connection_name         TEXT NOT NULL,
  model                   TEXT NOT NULL,
  capabilities_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_json)),
  default_parameters_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(default_parameters_json)),
  tool_policy_json        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(tool_policy_json)),
  config_digest           TEXT NOT NULL CHECK (length(config_digest) = 64),
  created_at_ms           INTEGER NOT NULL,
  PRIMARY KEY (agent_id, revision),
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_agent_defaults (
  project_id TEXT PRIMARY KEY,
  agent_id   TEXT NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE RESTRICT
) STRICT;

-- base_message_id is a soft cross-database reference. It is validated against
-- the registered Project DB on create/update/fire; SQLite cannot enforce a FK
-- into another database file.
CREATE TABLE crons (
  id                 TEXT PRIMARY KEY,
  name               TEXT NOT NULL,
  project_id         TEXT NOT NULL,
  base_message_id    TEXT NOT NULL,
  agent_id           TEXT NOT NULL,
  schedule           TEXT NOT NULL,
  timezone           TEXT NOT NULL,
  enabled            INTEGER NOT NULL CHECK (enabled IN (0, 1)),
  concurrency_policy TEXT NOT NULL CHECK (concurrency_policy IN ('allow', 'forbid', 'replace')),
  misfire_policy     TEXT NOT NULL CHECK (misfire_policy IN ('skip', 'run_once', 'catch_up')),
  next_run_at_ms     INTEGER,
  last_run_at_ms     INTEGER,
  version            INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE RESTRICT
) STRICT;

-- Global-to-Project Run creation is an idempotent saga, not a cross-DB TX.
-- local_run_id points into the Project DB and is intentionally not a FK.
CREATE TABLE cron_fires (
  cron_id          TEXT NOT NULL,
  scheduled_at_ms  INTEGER NOT NULL,
  project_id       TEXT NOT NULL,
  state            TEXT NOT NULL
    CHECK (state IN ('claimed', 'started', 'skipped', 'blocked', 'failed')),
  local_run_id     TEXT,
  error_json       TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
  claimed_at_ms    INTEGER NOT NULL,
  updated_at_ms    INTEGER NOT NULL,
  PRIMARY KEY (cron_id, scheduled_at_ms),
  FOREIGN KEY (cron_id) REFERENCES crons(id) ON DELETE RESTRICT,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
  CHECK ((state = 'started' AND local_run_id IS NOT NULL) OR state <> 'started')
) STRICT;

CREATE TABLE global_events (
  seq            INTEGER PRIMARY KEY,
  event_type     TEXT NOT NULL,
  entity_type    TEXT NOT NULL,
  entity_id      TEXT NOT NULL,
  payload_json   TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
  created_at_ms  INTEGER NOT NULL
) STRICT;

CREATE INDEX projects_status_idx ON projects(status, updated_at_ms DESC);
CREATE INDEX agents_enabled_idx ON agents(enabled, updated_at_ms DESC);
CREATE INDEX agent_revisions_latest_idx ON agent_revisions(agent_id, revision DESC);
CREATE INDEX crons_due_idx ON crons(next_run_at_ms) WHERE enabled = 1;
CREATE INDEX cron_fires_state_idx ON cron_fires(state, updated_at_ms);
CREATE INDEX global_events_entity_idx ON global_events(entity_type, entity_id, seq);

CREATE TRIGGER agent_revisions_no_update BEFORE UPDATE ON agent_revisions BEGIN
  SELECT RAISE(ABORT, 'AGENT_REVISION_IMMUTABLE');
END;
CREATE TRIGGER agent_revisions_no_delete BEFORE DELETE ON agent_revisions BEGIN
  SELECT RAISE(ABORT, 'AGENT_REVISION_IMMUTABLE');
END;

CREATE TRIGGER cron_fire_project_guard BEFORE INSERT ON cron_fires BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM crons c
     WHERE c.id = new.cron_id AND c.project_id = new.project_id
  ) THEN RAISE(ABORT, 'CRON_FIRE_PROJECT_MISMATCH') END;
END;

INSERT INTO schema_migrations(version, name, sha256, app_version, applied_at_ms)
VALUES (3, '0003_global_catalog_poc', lower(hex(zeroblob(32))), 'design-reference', 0);
