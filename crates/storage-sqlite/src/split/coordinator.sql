CREATE TABLE IF NOT EXISTS split_metadata (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  coordinator_id TEXT NOT NULL, migrated INTEGER NOT NULL
) STRICT;
INSERT OR IGNORE INTO split_metadata VALUES (1, lower(hex(randomblob(16))), 0);
CREATE TABLE IF NOT EXISTS record_locations (
  kind TEXT NOT NULL, id TEXT NOT NULL, project_id TEXT NOT NULL,
  PRIMARY KEY (kind, id)
) STRICT;
CREATE INDEX IF NOT EXISTS record_locations_project ON record_locations(project_id, kind);
CREATE TABLE IF NOT EXISTS pending_commit (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  body_json TEXT NOT NULL CHECK (json_valid(body_json))
) STRICT;
