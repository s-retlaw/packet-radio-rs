-- HD: License header (one per license)
CREATE TABLE IF NOT EXISTS hd (
    usi INTEGER PRIMARY KEY,
    call_sign TEXT NOT NULL,
    license_status TEXT NOT NULL DEFAULT '',
    radio_service_code TEXT NOT NULL DEFAULT '',
    grant_date TEXT NOT NULL DEFAULT '',
    expired_date TEXT NOT NULL DEFAULT '',
    cancellation_date TEXT NOT NULL DEFAULT '',
    last_action_date TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_hd_call_sign ON hd(call_sign);
CREATE INDEX IF NOT EXISTS idx_hd_status ON hd(license_status);

-- EN: Entity / name / address (one per license)
CREATE TABLE IF NOT EXISTS en (
    usi INTEGER PRIMARY KEY,
    entity_type TEXT NOT NULL DEFAULT '',
    licensee_id TEXT NOT NULL DEFAULT '',
    entity_name TEXT NOT NULL DEFAULT '',
    first_name TEXT NOT NULL DEFAULT '',
    mi TEXT NOT NULL DEFAULT '',
    last_name TEXT NOT NULL DEFAULT '',
    suffix TEXT NOT NULL DEFAULT '',
    street_address TEXT NOT NULL DEFAULT '',
    city TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT '',
    zip_code TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_en_name ON en(last_name, first_name);
CREATE INDEX IF NOT EXISTS idx_en_city_state ON en(state, city);
CREATE INDEX IF NOT EXISTS idx_en_zip ON en(zip_code);

-- AM: Amateur-specific data (one per license)
CREATE TABLE IF NOT EXISTS am (
    usi INTEGER PRIMARY KEY,
    operator_class TEXT NOT NULL DEFAULT '',
    group_code TEXT NOT NULL DEFAULT '',
    previous_operator_class TEXT NOT NULL DEFAULT '',
    previous_call_sign TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_am_class ON am(operator_class);

-- HS: History / status log (many per license)
CREATE TABLE IF NOT EXISTS hs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    usi INTEGER NOT NULL,
    log_date TEXT NOT NULL DEFAULT '',
    code TEXT NOT NULL DEFAULT '',
    UNIQUE(usi, log_date, code)
);
CREATE INDEX IF NOT EXISTS idx_hs_usi ON hs(usi);
CREATE UNIQUE INDEX IF NOT EXISTS idx_hs_unique ON hs(usi, log_date, code);

-- CO: Comments / notes (many per license)
CREATE TABLE IF NOT EXISTS co (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    usi INTEGER NOT NULL,
    comment_date TEXT NOT NULL DEFAULT '',
    comment TEXT NOT NULL DEFAULT '',
    UNIQUE(usi, comment_date, comment)
);
CREATE INDEX IF NOT EXISTS idx_co_usi ON co(usi);

-- Geocodes: lat/lon for each license (separate from EN for stale tracking)
CREATE TABLE IF NOT EXISTS geocodes (
    usi INTEGER PRIMARY KEY,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    geo_source TEXT NOT NULL DEFAULT '',
    geo_quality TEXT NOT NULL DEFAULT '',
    geo_stale INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_geocodes_latlon ON geocodes(lat, lon);
CREATE INDEX IF NOT EXISTS idx_geocodes_stale ON geocodes(geo_stale) WHERE geo_stale = 1;

-- ZIP centroids: fallback geocoding for PO Box addresses
CREATE TABLE IF NOT EXISTS zip_centroids (
    zip TEXT PRIMARY KEY,
    lat REAL NOT NULL,
    lon REAL NOT NULL
);

-- Sync audit log
CREATE TABLE IF NOT EXISTS fcc_sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_type TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    records_processed INTEGER,
    error_message TEXT
)
