CREATE TABLE tweet_drafts (
  id TEXT PRIMARY KEY,
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  created_at INTEGER NOT NULL,
  source_text TEXT NOT NULL,
  personal_take TEXT NOT NULL,
  payload TEXT,
  error TEXT,
  attempts INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX tweet_drafts_recording ON tweet_drafts(recording_id,created_at);
