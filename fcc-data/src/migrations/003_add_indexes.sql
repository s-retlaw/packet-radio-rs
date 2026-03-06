-- Migration 003: Add indexes for common query patterns
-- am.previous_call_sign: used by callsign_history_chain (full table scan without)
-- en.licensee_id: used by related_by_licensee (full table scan without)

CREATE INDEX IF NOT EXISTS idx_am_prev_call ON am(previous_call_sign);
CREATE INDEX IF NOT EXISTS idx_en_licensee_id ON en(licensee_id)
