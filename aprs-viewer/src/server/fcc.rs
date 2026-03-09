use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::state::AppState;

/// Strip SSID from an APRS callsign: "W1ABC-9" -> "W1ABC"
fn strip_ssid(call: &str) -> &str {
    call.split('-').next().unwrap_or(call)
}

/// FCC license lookup response.
#[derive(Serialize)]
struct FccLookupResponse {
    callsign: String,
    name: String,
    operator_class: String,
    city: String,
    state: String,
    zip_code: String,
    grant_date: String,
    expired_date: String,
    license_status: String,
    previous_call_sign: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_source: Option<String>,
}

/// FCC nearby station response (compact).
#[derive(Serialize)]
struct FccNearbyStation {
    callsign: String,
    name: String,
    operator_class: String,
    lat: f64,
    lon: f64,
}

/// History chain entry.
#[derive(Serialize)]
struct FccHistoryEntry {
    callsign: String,
    operator_class: String,
    status: String,
    grant_date: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

/// GET /api/stations/{call}/fcc
pub async fn fcc_lookup(
    State(state): State<AppState>,
    Path(call): Path<String>,
) -> impl IntoResponse {
    let fcc_db = match &state.fcc_db {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "fcc_db_unavailable",
                }),
            )
                .into_response()
        }
    };

    let bare_call = strip_ssid(&call);

    match fcc_db.lookup_callsign(bare_call).await {
        Ok(Some(rec)) => {
            let resp = FccLookupResponse {
                callsign: rec.call_sign.clone(),
                name: rec.display_name(),
                operator_class: fcc_data::models::OperatorClass::from_code(&rec.operator_class)
                    .to_string(),
                city: rec.city.clone(),
                state: rec.state.clone(),
                zip_code: rec.zip_code.clone(),
                grant_date: rec.grant_date.clone(),
                expired_date: rec.expired_date.clone(),
                license_status: rec.license_status.clone(),
                previous_call_sign: rec.previous_call_sign.clone(),
                lat: rec.lat,
                lon: rec.lon,
                geo_source: rec.geo_source.clone(),
            };
            Json(resp).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "not_found",
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("FCC lookup error for {}: {}", bare_call, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal_error",
                }),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct BboxQuery {
    south: f64,
    north: f64,
    west: f64,
    east: f64,
}

/// GET /api/fcc/bbox?south=X&north=X&west=X&east=X&limit=N
pub async fn fcc_bbox(
    State(state): State<AppState>,
    Query(query): Query<BboxQuery>,
) -> impl IntoResponse {
    let fcc_db = match &state.fcc_db {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "fcc_db_unavailable",
                }),
            )
                .into_response()
        }
    };

    match fcc_db
        .stations_in_bbox(query.south, query.north, query.west, query.east)
        .await
    {
        Ok(records) => {
            let stations: Vec<FccNearbyStation> = records
                .into_iter()
                .filter_map(|rec| {
                    let lat = rec.lat?;
                    let lon = rec.lon?;
                    let name = rec.display_name();
                    let class =
                        fcc_data::models::OperatorClass::from_code(&rec.operator_class).to_string();
                    Some(FccNearbyStation {
                        callsign: rec.call_sign,
                        name,
                        operator_class: class,
                        lat,
                        lon,
                    })
                })
                .collect();
            Json(stations).into_response()
        }
        Err(e) => {
            tracing::error!("FCC bbox error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal_error",
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/stations/{call}/fcc/history
pub async fn fcc_history(
    State(state): State<AppState>,
    Path(call): Path<String>,
) -> impl IntoResponse {
    let fcc_db = match &state.fcc_db {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "fcc_db_unavailable",
                }),
            )
                .into_response()
        }
    };

    let bare_call = strip_ssid(&call);

    match fcc_db.callsign_history_chain(bare_call).await {
        Ok(chain) => {
            let entries: Vec<FccHistoryEntry> = chain
                .into_iter()
                .map(|(cs, oc, status, grant)| FccHistoryEntry {
                    callsign: cs,
                    operator_class: fcc_data::models::OperatorClass::from_code(&oc).to_string(),
                    status,
                    grant_date: grant,
                })
                .collect();
            Json(entries).into_response()
        }
        Err(e) => {
            tracing::error!("FCC history error for {}: {}", bare_call, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal_error",
                }),
            )
                .into_response()
        }
    }
}
