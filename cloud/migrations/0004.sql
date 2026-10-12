-- Ongoing private automatic tweet suggestions + editable voice preferences.
-- 0001..0003 are already applied remotely; this is additive only.
ALTER TABLE tweet_drafts ADD COLUMN kind TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE tweet_drafts ADD COLUMN approved INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tweet_drafts ADD COLUMN window_start INTEGER;
ALTER TABLE tweet_drafts ADD COLUMN window_end INTEGER;
CREATE INDEX tweet_drafts_kind ON tweet_drafts(kind, created_at);

-- One row per considered transcript window. The primary key makes window creation
-- idempotent and the claim_token/lease make model calls exactly-once under retries.
CREATE TABLE suggestion_windows (
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  window_index INTEGER NOT NULL,
  start_seq INTEGER NOT NULL,
  end_seq INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  claim_token TEXT,
  lease_until INTEGER NOT NULL DEFAULT 0,
  draft_id TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (recording_id, window_index)
);
CREATE INDEX suggestion_windows_recent ON suggestion_windows(created_at);
CREATE INDEX suggestion_windows_work ON suggestion_windows(status, lease_until);

-- Single-row owner voice preferences. Empty style/examples fall back to the built-in VOICE guide.
CREATE TABLE voice_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  style_instructions TEXT NOT NULL DEFAULT '',
  example_posts TEXT NOT NULL DEFAULT '',
  auto_enabled INTEGER NOT NULL DEFAULT 1,
  cadence_minutes INTEGER NOT NULL DEFAULT 5,
  updated_at INTEGER NOT NULL DEFAULT 0,
  revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO voice_settings(id) VALUES (1);
