-- APRS Viewer initial schema

CREATE TABLE IF NOT EXISTS stations (
    callsign TEXT NOT NULL,
    ssid INTEGER NOT NULL DEFAULT 0,
    station_type TEXT NOT NULL DEFAULT 'Unknown',
    lat REAL,
    lon REAL,
    speed REAL,
    course REAL,
    altitude REAL,
    comment TEXT,
    symbol_table TEXT,
    symbol_code TEXT,
    weather_json TEXT,
    packet_count INTEGER NOT NULL DEFAULT 0,
    last_heard TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (callsign, ssid)
);

CREATE TABLE IF NOT EXISTS packets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    source_ssid INTEGER NOT NULL DEFAULT 0,
    dest TEXT NOT NULL,
    path TEXT,
    packet_type TEXT,
    raw_info TEXT NOT NULL,
    summary TEXT,
    received_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS position_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    callsign TEXT NOT NULL,
    ssid INTEGER NOT NULL DEFAULT 0,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    altitude REAL,
    speed REAL,
    course REAL,
    recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_call TEXT NOT NULL,
    to_call TEXT NOT NULL,
    message_text TEXT NOT NULL,
    message_no TEXT,
    acked INTEGER NOT NULL DEFAULT 0,
    received_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_packets_source ON packets(source);
CREATE INDEX IF NOT EXISTS idx_packets_received ON packets(received_at);
CREATE INDEX IF NOT EXISTS idx_position_history_call ON position_history(callsign, ssid, recorded_at);
CREATE INDEX IF NOT EXISTS idx_messages_from ON messages(from_call);
CREATE INDEX IF NOT EXISTS idx_messages_to ON messages(to_call);
CREATE INDEX IF NOT EXISTS idx_stations_last_heard ON stations(last_heard);
