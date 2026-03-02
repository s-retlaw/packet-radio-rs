-- Core lookup table: fast callsign -> position for any source
-- This is what the APRS viewer queries at packet-processing time
CREATE TABLE IF NOT EXISTS positions (
    callsign TEXT NOT NULL,
    source TEXT NOT NULL,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (callsign, source)
);
CREATE INDEX IF NOT EXISTS idx_positions_geo ON positions(lat, lon);

-- CWOP-specific metadata (joins to positions on callsign)
CREATE TABLE IF NOT EXISTS cwop_stations (
    callsign TEXT PRIMARY KEY,
    nwsid TEXT,
    city TEXT,
    region TEXT NOT NULL,
    elevation_m REAL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cwop_region ON cwop_stations(region);

-- Sync tracking per source+region
CREATE TABLE IF NOT EXISTS sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    region TEXT,
    synced_at TEXT NOT NULL,
    station_count INTEGER,
    duration_ms INTEGER
);
