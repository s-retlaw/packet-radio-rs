-- Weather time-series history
CREATE TABLE IF NOT EXISTS weather_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    callsign TEXT NOT NULL,
    ssid INTEGER NOT NULL DEFAULT 0,
    temperature INTEGER,
    wind_speed INTEGER,
    wind_direction INTEGER,
    wind_gust INTEGER,
    humidity INTEGER,
    barometric_pressure INTEGER,
    rain_last_hour INTEGER,
    rain_24h INTEGER,
    luminosity INTEGER,
    recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_weather_history_call
    ON weather_history(callsign, ssid, recorded_at);

-- Source tracking: how we received each packet
ALTER TABLE packets ADD COLUMN source_type TEXT NOT NULL DEFAULT 'unknown';

-- Station source tracking
ALTER TABLE stations ADD COLUMN heard_via TEXT NOT NULL DEFAULT '';
ALTER TABLE stations ADD COLUMN last_source_type TEXT NOT NULL DEFAULT 'unknown';
