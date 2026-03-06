-- Migration 002: Add missing fields discovered from raw FCC data analysis
-- am.region_code: FCC administrative region (index 7, was misread as previous_operator_class)
-- en.frn: Real FRN from EN index 22 (numeric, distinct from licensee_id)
-- co.status_code/status_date: Comment status fields (indices 6, 7)

ALTER TABLE am ADD COLUMN region_code TEXT NOT NULL DEFAULT '';
ALTER TABLE en ADD COLUMN frn TEXT NOT NULL DEFAULT '';
ALTER TABLE co ADD COLUMN status_code TEXT NOT NULL DEFAULT '';
ALTER TABLE co ADD COLUMN status_date TEXT NOT NULL DEFAULT ''
