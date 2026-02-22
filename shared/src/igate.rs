//! APRS-IS Client — connect to the APRS Internet System.
//!
//! APRS-IS is a network of servers that distribute APRS packets worldwide.
//! An IGate (Internet Gateway) bridges packets between RF and APRS-IS.
//!
//! Connection: TCP to rotate.aprs2.net:14580
//! Login: `user CALLSIGN pass PASSCODE vers SOFTWARE VERSION filter FILTER`
//!
//! Passcode algorithm: hash of callsign (without SSID)

/// Compute the APRS-IS passcode for a given callsign.
///
/// The passcode is a simple hash of the callsign (without SSID).
/// This is NOT a security measure — it's just a basic verification.
pub fn compute_passcode(callsign: &str) -> i16 {
    let callsign_upper = callsign.split('-').next().unwrap_or(callsign);
    let mut hash: i16 = 0x73e2u16 as i16;

    let bytes = callsign_upper.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        hash ^= (bytes[i] as i16) << 8;
        hash ^= bytes[i + 1] as i16;
        i += 2;
    }
    if i < bytes.len() {
        hash ^= (bytes[i] as i16) << 8;
    }

    hash & 0x7FFF
}

// TODO: Implement APRS-IS TCP client
// TODO: Implement IGate logic:
//   - RF → IS: Forward heard packets to APRS-IS (with q construct)
//   - IS → RF: Forward messages addressed to local stations
//   - Duplicate suppression (30-second window)
//   - Rate limiting

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passcode() {
        // Known passcode values for testing
        // TODO: Add known callsign/passcode pairs
        let code = compute_passcode("N0CALL");
        assert!(code >= 0); // Passcode is always positive
    }
}
