-- Board sync with the signed-in account (src/board_sync.rs, arduloops-api /v1/boards/*).
-- When the note and the profile were last edited, so the newer edit wins across devices.
-- 0 means "never edited here".
ALTER TABLE controllers ADD COLUMN comment_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE controllers ADD COLUMN profile_at INTEGER NOT NULL DEFAULT 0;
-- Notes and profiles written before this migration count as written when the board was last seen,
-- so a board named on one device before sync existed keeps its name against an empty cloud record.
UPDATE controllers SET comment_at = last_seen WHERE comment_at = 0 AND comment != '';
UPDATE controllers SET profile_at = last_seen WHERE profile_at = 0 AND profile != '';
-- The account's boards as last listed, one row per hardware id: the copy shown for a board that
-- was never linked here (and offline), and the record of what the account already holds.
-- `params` is the parameter snapshot as JSON, fetched when the account's is newer than the one
-- here; it is never written to param_cache, which holds only what this device read from a vehicle.
-- `deleted_at` > 0 marks a board removed from the account.
CREATE TABLE IF NOT EXISTS cloud_boards (
  key TEXT PRIMARY KEY,
  board_id INTEGER NOT NULL DEFAULT 0,
  board_name TEXT NOT NULL DEFAULT '',
  vehicle TEXT NOT NULL DEFAULT '',
  first_seen INTEGER NOT NULL DEFAULT 0,
  last_seen INTEGER NOT NULL DEFAULT 0,
  comment TEXT NOT NULL DEFAULT '',
  comment_at INTEGER NOT NULL DEFAULT 0,
  profile TEXT NOT NULL DEFAULT '',
  profile_at INTEGER NOT NULL DEFAULT 0,
  params_at INTEGER NOT NULL DEFAULT 0,
  params_count INTEGER NOT NULL DEFAULT 0,
  params_frame TEXT NOT NULL DEFAULT '',
  params TEXT NOT NULL DEFAULT '',
  params_fetched_at INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0
);
