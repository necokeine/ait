CREATE TABLE portable_catalog (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), catalog_id TEXT NOT NULL,
  feed_generation TEXT NOT NULL
) STRICT;
INSERT INTO portable_catalog VALUES(1, lower(hex(randomblob(16))), lower(hex(randomblob(16))));
CREATE TABLE IF NOT EXISTS record_locations (
  kind TEXT NOT NULL, id TEXT NOT NULL, project_id TEXT NOT NULL,
  PRIMARY KEY(kind, id)
) STRICT;
CREATE INDEX locations_project ON record_locations(project_id);
CREATE TABLE projected_events (
  project_id TEXT NOT NULL, stream_id TEXT NOT NULL, event_seq INTEGER NOT NULL,
  feed_cursor INTEGER UNIQUE,
  PRIMARY KEY(project_id, stream_id, event_seq)
) STRICT;
CREATE TABLE conversion_manifest (
  project_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL, phase TEXT NOT NULL
) STRICT;
CREATE TABLE projection_watermarks (
  project_id TEXT PRIMARY KEY, stream_id TEXT NOT NULL, revision INTEGER NOT NULL, event_seq INTEGER NOT NULL
) STRICT;
