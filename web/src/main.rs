use axum::routing::{delete, get, put};
use axum::Router;
use packet_radio_web::server::{
    aprs_is, cleanup, config::WebConfig, ingest, state::AppState, tiles, ws,
};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

// Embed the public/ directory into the binary
#[derive(rust_embed::Embed)]
#[folder = "public/"]
struct Assets;

// Embed the style/ directory
#[derive(rust_embed::Embed)]
#[folder = "style/"]
#[prefix = "style/"]
struct StyleAssets;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Load config
    let config_path = "aprs-viewer.toml";
    let config = WebConfig::load_or_default(config_path);
    let listen_addr = config.listen_addr.clone();

    // Init SQLite
    let db_url = format!("sqlite:{}?mode=rwc", config.db_path);
    let pool = SqlitePool::connect(&db_url)
        .await
        .expect("Failed to connect to SQLite");

    // Run migrations
    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await
        .expect("Failed to run migrations");

    // Broadcast channel for WebSocket events
    let (packet_tx, _) = broadcast::channel::<String>(256);

    // Config change notification channel
    let (config_notify_tx, _config_notify_rx) = tokio::sync::watch::channel(());

    // Open reference database (CWOP station positions, etc.)
    let reference_db = {
        let ref_config = &config.reference;
        let ref_path = if ref_config.db_path.is_empty() {
            reference::db::default_db_path()
        } else {
            std::path::PathBuf::from(&ref_config.db_path)
        };
        match reference::ReferenceDb::open(&ref_path).await {
            Ok(db) => {
                tracing::info!("Reference DB: {}", ref_path.display());
                Some(Arc::new(db))
            }
            Err(e) => {
                tracing::warn!("Failed to open reference DB at {}: {}", ref_path.display(), e);
                None
            }
        }
    };

    // Create app state
    let config_arc = Arc::new(RwLock::new(config));
    let app_state = AppState {
        db: pool.clone(),
        packet_tx: packet_tx.clone(),
        config: config_arc.clone(),
        config_path: config_path.to_string(),
        config_notify: Arc::new(config_notify_tx),
        reference_db: reference_db.clone(),
    };

    // Spawn KISS TNC ingest task
    {
        let pool = pool.clone();
        let tx = packet_tx.clone();
        let config_arc = config_arc.clone();
        let reference_db = reference_db.clone();
        let mut config_rx = app_state.config_notify.subscribe();
        tokio::spawn(async move {
            loop {
                let (enabled, host, port) = {
                    let cfg = config_arc.read().await;
                    (cfg.tnc.enabled, cfg.tnc.host.clone(), cfg.tnc.port)
                };
                if enabled {
                    tokio::select! {
                        _ = ingest::run_kiss_ingest(&host, port, pool.clone(), tx.clone(), reference_db.clone()) => {},
                        _ = config_rx.changed() => {
                            tracing::info!("TNC config changed, reconnecting...");
                        }
                    }
                } else {
                    if config_rx.changed().await.is_err() {
                        break;
                    }
                }
            }
        });
    }

    // Spawn APRS-IS client task
    {
        let pool = pool.clone();
        let tx = packet_tx.clone();
        let config_arc = config_arc.clone();
        let reference_db = reference_db.clone();
        let mut config_rx = app_state.config_notify.subscribe();
        tokio::spawn(async move {
            loop {
                let (enabled, host, port, callsign, passcode, filter) = {
                    let cfg = config_arc.read().await;
                    (
                        cfg.aprs_is.enabled,
                        cfg.aprs_is.host.clone(),
                        cfg.aprs_is.port,
                        cfg.aprs_is.callsign.clone(),
                        cfg.aprs_is.passcode.clone(),
                        cfg.aprs_is.filter.clone(),
                    )
                };
                if enabled {
                    tokio::select! {
                        _ = aprs_is::run_aprs_is_client(&host, port, &callsign, &passcode, &filter, pool.clone(), tx.clone(), reference_db.clone()) => {},
                        _ = config_rx.changed() => {
                            tracing::info!("APRS-IS config changed, reconnecting...");
                        }
                    }
                } else {
                    if config_rx.changed().await.is_err() {
                        break;
                    }
                }
            }
        });
    }

    // Spawn CWOP reference data sync task
    if let Some(ref ref_db) = reference_db {
        let ref_db = ref_db.clone();
        let config_arc = config_arc.clone();
        tokio::spawn(async move {
            let sync_interval_hours = {
                let cfg = config_arc.read().await;
                cfg.reference.cwop_sync_interval_hours
            };
            if sync_interval_hours == 0 {
                tracing::info!("CWOP sync disabled (cwop_sync_interval_hours = 0)");
                return;
            }

            let max_age = std::time::Duration::from_secs(sync_interval_hours as u64 * 3600);
            let fetcher = reference::cwop::fetcher::HttpFetcher::new();

            // We need to clone the inner pool to create a new ReferenceDb for CwopSource.
            // CwopSource takes ownership, so we re-open at the same path.
            let db_path = ref_db.path().to_path_buf();
            let cwop_db = match reference::ReferenceDb::open(&db_path).await {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!("Failed to open reference DB for CWOP sync: {}", e);
                    return;
                }
            };

            let source = reference::cwop::CwopSource::new(fetcher, cwop_db);

            loop {
                match source.sync_if_stale(max_age).await {
                    Ok(Some(result)) => {
                        tracing::info!(
                            "CWOP sync: {} stations from {} regions",
                            result.total_stations,
                            result.total_regions
                        );
                    }
                    Ok(None) => {} // Fresh, skipped
                    Err(e) => {
                        tracing::error!("CWOP sync error: {}", e);
                    }
                }

                // Check again after the configured interval
                tokio::time::sleep(std::time::Duration::from_secs(
                    sync_interval_hours as u64 * 3600,
                ))
                .await;
            }
        });
    }

    // Spawn cleanup task
    {
        let pool = pool.clone();
        let config_arc = config_arc.clone();
        tokio::spawn(async move {
            loop {
                let (max_station, max_track) = {
                    let cfg = config_arc.read().await;
                    (cfg.max_station_age_hours, cfg.max_track_age_hours)
                };
                cleanup::run_cleanup_once(pool.clone(), max_station, max_track).await;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }

    // Build router
    let app = Router::new()
        // WebSocket
        .route("/ws/packets", get(ws::ws_handler))
        // Tiles
        .route("/tiles/{*path}", get(tiles::tiles_handler))
        // API endpoints
        .route("/api/stations", get(api_get_stations))
        .route("/api/stations/{call}/track", get(api_get_station_track))
        .route("/api/stations/{call}/weather", get(api_get_station_weather))
        .route("/api/packets", get(api_get_packets))
        .route("/api/config", get(api_get_config).put(api_put_config))
        .route("/api/maps", get(api_list_maps))
        .route("/api/maps/{name}", delete(api_delete_map))
        .route("/api/maps/download", put(api_download_map))
        // Static files fallback
        .fallback(static_handler)
        .with_state(app_state);

    let addr: std::net::SocketAddr = listen_addr.parse().unwrap_or_else(|_| {
        tracing::warn!("Invalid listen_addr '{}', using 127.0.0.1:3000", listen_addr);
        "127.0.0.1:3000".parse().unwrap()
    });

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let url = format!("http://{}", &addr);
    tracing::info!("listening on {}", &url);

    // Auto-open browser unless --no-browser is passed
    let no_browser = std::env::args().any(|a| a == "--no-browser");
    if !no_browser {
        if let Err(e) = open::that(&url) {
            tracing::warn!("Failed to open browser: {}", e);
        }
    }

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

// === Static File Handler ===

async fn static_handler(
    uri: axum::http::Uri,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, Response, StatusCode};

    let path = uri.path().trim_start_matches('/');

    // Try exact path first, then fall back to index.html
    let (file, effective_path) = if path.is_empty() {
        (Assets::get("index.html"), "index.html")
    } else {
        (Assets::get(path).or_else(|| StyleAssets::get(path)), path)
    };

    let (content, mime) = match file {
        Some(c) => (c, mime_for_path(effective_path)),
        None => match Assets::get("index.html") {
            Some(c) => (c, "text/html; charset=utf-8"),
            None => {
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("Not found"))
                    .unwrap();
            }
        },
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .header(header::PRAGMA, "no-cache")
        .body(Body::from(content.data.into_owned()))
        .unwrap()
}

// === API Handlers ===

async fn api_get_stations(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;
    match packet_radio_web::server::db::get_stations(&state.db).await {
        Ok(stations) => axum::Json(stations).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get stations: {}", e),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct TrackQuery {
    hours: Option<u32>,
}

async fn api_get_station_track(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(call): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<TrackQuery>,
) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;

    let hours = query.hours.unwrap_or(24);
    let (callsign, ssid) = parse_callsign_ssid(&call);

    match packet_radio_web::server::db::get_station_track(&state.db, &callsign, ssid, hours).await {
        Ok(track) => axum::Json(track).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get track: {}", e),
        )
            .into_response(),
    }
}

async fn api_get_station_weather(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(call): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;

    let (callsign, ssid) = parse_callsign_ssid(&call);

    match packet_radio_web::server::db::get_station_by_callsign(&state.db, &callsign, ssid).await {
        Ok(Some(station)) => axum::Json(station.weather).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "Station not found").into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get weather: {}", e),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct PacketsQuery {
    limit: Option<i64>,
}

async fn api_get_packets(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PacketsQuery>,
) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;

    let limit = query.limit.unwrap_or(200).min(1000);

    match packet_radio_web::server::db::get_recent_packets(&state.db, limit).await {
        Ok(packets) => axum::Json(packets).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get packets: {}", e),
        )
            .into_response(),
    }
}

async fn api_get_config(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    let config = state.config.read().await;
    axum::Json(config.clone())
}

async fn api_put_config(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(new_config): axum::Json<WebConfig>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    if let Err(e) = new_config.validate() {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Err(e) = new_config.save(&state.config_path) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save config: {}", e))
            .into_response();
    }

    {
        let mut config = state.config.write().await;
        *config = new_config;
    }

    let _ = state.config_notify.send(());
    (StatusCode::OK, "Config saved").into_response()
}

async fn api_list_maps(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;
    let maps_dir = {
        let cfg = state.config.read().await;
        cfg.maps_dir.clone()
    };
    match packet_radio_web::server::map_manager::list_installed_packs(&maps_dir).await {
        Ok(packs) => axum::Json(packs).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list maps: {}", e),
        )
            .into_response(),
    }
}

async fn api_delete_map(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let maps_dir = {
        let cfg = state.config.read().await;
        cfg.maps_dir.clone()
    };
    match packet_radio_web::server::map_manager::delete_pack(&maps_dir, &name).await {
        Ok(()) => (StatusCode::OK, "Deleted").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{}", e)).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct DownloadRequest {
    url: String,
    filename: String,
}

async fn api_download_map(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(req): axum::Json<DownloadRequest>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let maps_dir = {
        let cfg = state.config.read().await;
        cfg.maps_dir.clone()
    };
    match packet_radio_web::server::map_manager::download_pack(&req.url, &req.filename, &maps_dir)
        .await
    {
        Ok(()) => (StatusCode::OK, "Downloaded").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Download failed: {}", e),
        )
            .into_response(),
    }
}

fn mime_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("pmtiles") => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

/// Parse "N0CALL-9" into ("N0CALL", 9) or "N0CALL" into ("N0CALL", 0).
fn parse_callsign_ssid(call: &str) -> (String, u8) {
    if let Some((cs, ssid_str)) = call.rsplit_once('-') {
        if let Ok(ssid) = ssid_str.parse::<u8>() {
            return (cs.to_string(), ssid);
        }
    }
    (call.to_string(), 0)
}
