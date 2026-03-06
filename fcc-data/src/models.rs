use serde::{Deserialize, Serialize};

/// FCC license status codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LicenseStatus {
    Active,
    Cancelled,
    Expired,
    Terminated,
    Other(String),
}

impl LicenseStatus {
    pub fn from_code(code: &str) -> Self {
        match code.trim().to_uppercase().as_str() {
            "A" => Self::Active,
            "C" => Self::Cancelled,
            "E" => Self::Expired,
            "T" => Self::Terminated,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn to_code(&self) -> &str {
        match self {
            Self::Active => "A",
            Self::Cancelled => "C",
            Self::Expired => "E",
            Self::Terminated => "T",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Amateur radio operator class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperatorClass {
    Technician,
    General,
    Extra,
    Novice,
    Advanced,
    Other(String),
}

impl OperatorClass {
    pub fn from_code(code: &str) -> Self {
        match code.trim().to_uppercase().as_str() {
            "T" => Self::Technician,
            "G" => Self::General,
            "E" => Self::Extra,
            "N" => Self::Novice,
            "A" => Self::Advanced,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn to_code(&self) -> &str {
        match self {
            Self::Technician => "T",
            Self::General => "G",
            Self::Extra => "E",
            Self::Novice => "N",
            Self::Advanced => "A",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for OperatorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Technician => write!(f, "Technician"),
            Self::General => write!(f, "General"),
            Self::Extra => write!(f, "Extra"),
            Self::Novice => write!(f, "Novice"),
            Self::Advanced => write!(f, "Advanced"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

/// HD record — license header.
#[derive(Debug, Clone)]
pub struct HdRecord {
    pub usi: i64,
    pub call_sign: String,
    pub license_status: String,
    pub radio_service_code: String,
    pub grant_date: String,
    pub expired_date: String,
    pub cancellation_date: String,
    pub last_action_date: String,
}

/// EN record — entity/name/address.
#[derive(Debug, Clone)]
pub struct EnRecord {
    pub usi: i64,
    pub entity_type: String,
    pub licensee_id: String,
    pub entity_name: String,
    pub first_name: String,
    pub mi: String,
    pub last_name: String,
    pub suffix: String,
    pub street_address: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
    pub frn: String,
}

/// AM record — amateur-specific data.
#[derive(Debug, Clone)]
pub struct AmRecord {
    pub usi: i64,
    pub operator_class: String,
    pub group_code: String,
    pub region_code: String,
    pub previous_operator_class: String,
    pub previous_call_sign: String,
}

/// HS record — history/status log.
#[derive(Debug, Clone)]
pub struct HsRecord {
    pub usi: i64,
    pub log_date: String,
    pub code: String,
}

/// CO record — comments/notes.
#[derive(Debug, Clone)]
pub struct CoRecord {
    pub usi: i64,
    pub comment_date: String,
    pub comment: String,
    pub status_code: String,
    pub status_date: String,
}

/// Full license record joined from HD+EN+AM for search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseRecord {
    pub usi: i64,
    pub call_sign: String,
    pub license_status: String,
    pub operator_class: String,
    pub first_name: String,
    pub last_name: String,
    pub entity_name: String,
    pub street_address: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
    pub grant_date: String,
    pub expired_date: String,
    pub previous_call_sign: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub geo_source: Option<String>,
    pub frn: String,
    pub licensee_id: String,
    pub mi: String,
    pub suffix: String,
    pub previous_operator_class: String,
    pub cancellation_date: String,
    pub last_action_date: String,
    pub radio_service_code: String,
    pub region_code: String,
    pub entity_type: String,
    pub geo_quality: Option<String>,
}

/// Search query parameters.
#[derive(Debug, Default, Clone)]
pub struct SearchQuery {
    pub call_sign: Option<String>,
    pub name: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip_code: Option<String>,
    pub operator_class: Option<String>,
    pub license_status: Option<String>,
    pub limit: Option<i64>,
}

/// Geographic proximity search.
#[derive(Debug, Clone)]
pub struct GeoQuery {
    pub lat: f64,
    pub lon: f64,
    pub radius_km: f64,
    pub limit: Option<i64>,
}

/// Geocode result from Census Bureau or ZIP centroid.
#[derive(Debug, Clone)]
pub struct GeocodeResult {
    pub lat: f64,
    pub lon: f64,
    pub source: String,
    pub quality: String,
}

/// ZIP code centroid record.
#[derive(Debug, Clone, Deserialize)]
pub struct ZipCentroid {
    pub zip: String,
    pub lat: f64,
    pub lng: f64,
}

/// Sync log entry.
#[derive(Debug, Clone)]
pub struct SyncLogEntry {
    pub id: i64,
    pub sync_type: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub records_processed: Option<i64>,
    pub error_message: Option<String>,
}

/// Detection of PO Box addresses.
pub fn is_po_box(address: &str) -> bool {
    let upper = address.to_uppercase();
    let upper = upper.trim();
    upper.starts_with("PO BOX")
        || upper.starts_with("P.O. BOX")
        || upper.starts_with("P O BOX")
        || upper.starts_with("POB ")
        || upper.starts_with("P.O.B ")
        || upper == "PO BOX"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_license_status_roundtrip() {
        assert_eq!(LicenseStatus::from_code("A"), LicenseStatus::Active);
        assert_eq!(LicenseStatus::from_code("a"), LicenseStatus::Active);
        assert_eq!(LicenseStatus::Active.to_code(), "A");
    }

    #[test]
    fn test_operator_class_roundtrip() {
        assert_eq!(OperatorClass::from_code("E"), OperatorClass::Extra);
        assert_eq!(OperatorClass::Extra.to_code(), "E");
        assert_eq!(format!("{}", OperatorClass::General), "General");
    }

    #[test]
    fn test_is_po_box() {
        assert!(is_po_box("PO BOX 123"));
        assert!(is_po_box("P.O. BOX 456"));
        assert!(is_po_box("P O BOX 789"));
        assert!(is_po_box("POB 100"));
        assert!(!is_po_box("123 Main Street"));
        assert!(!is_po_box(""));
    }
}
