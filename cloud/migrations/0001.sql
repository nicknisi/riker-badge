CREATE TABLE recordings (
  id TEXT PRIMARY KEY,
  device_id TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  expected_chunks INTEGER,
  total_samples INTEGER,
  reason TEXT,
  sealed_at INTEGER,
  status TEXT NOT NULL DEFAULT 'open',
  notes TEXT,
  raw_cleaned INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER
);
CREATE TABLE chunks (
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  seq INTEGER NOT NULL,
  object_key TEXT NOT NULL UNIQUE,
  sha256 TEXT NOT NULL,
  samples INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'receiving',
  attempts INTEGER NOT NULL DEFAULT 0,
  claim_token TEXT,
  lease_until INTEGER NOT NULL DEFAULT 0,
  words TEXT,
  error TEXT,
  PRIMARY KEY (recording_id, seq)
);
CREATE INDEX chunks_work ON chunks(status, lease_until);
CREATE TABLE maintenance (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE summaries (
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  level INTEGER NOT NULL,
  part INTEGER NOT NULL,
  input_hash TEXT NOT NULL,
  output TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  claim_token TEXT,
  lease_until INTEGER NOT NULL DEFAULT 0,
  error TEXT,
  PRIMARY KEY (recording_id, level, part)
);
