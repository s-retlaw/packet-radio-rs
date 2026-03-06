use crate::models::{AmRecord, CoRecord, EnRecord, HdRecord, HsRecord};

/// Parse an HD.dat line (pipe-delimited).
/// Fields: record_type|usi|uls_file_number|ebf_number|call_sign|license_status|
///         radio_service_code|grant_date|expired_date|cancellation_date|...
/// We only extract the fields we need.
pub fn parse_hd_line(line: &str) -> Option<HdRecord> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 10 || fields[0] != "HD" {
        return None;
    }
    let usi = fields[1].trim().parse::<i64>().ok()?;
    Some(HdRecord {
        usi,
        call_sign: fields[4].trim().to_string(),
        license_status: fields[5].trim().to_string(),
        radio_service_code: fields[6].trim().to_string(),
        grant_date: fields[7].trim().to_string(),
        expired_date: fields[8].trim().to_string(),
        cancellation_date: fields[9].trim().to_string(),
        last_action_date: fields.get(43).unwrap_or(&"").trim().to_string(),
    })
}

/// Parse an EN.dat line (pipe-delimited).
/// Fields: record_type|usi|uls_file_number|ebf_number|call_sign|entity_type|
///         licensee_id|entity_name|first_name|mi|last_name|suffix|...
///         street_address|city|state|zip_code|...
pub fn parse_en_line(line: &str) -> Option<EnRecord> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 19 || fields[0] != "EN" {
        return None;
    }
    let usi = fields[1].trim().parse::<i64>().ok()?;
    Some(EnRecord {
        usi,
        entity_type: fields[5].trim().to_string(),
        licensee_id: fields[6].trim().to_string(),
        entity_name: fields[7].trim().to_string(),
        first_name: fields[8].trim().to_string(),
        mi: fields[9].trim().to_string(),
        last_name: fields[10].trim().to_string(),
        suffix: fields[11].trim().to_string(),
        street_address: fields[15].trim().to_string(),
        city: fields[16].trim().to_string(),
        state: fields[17].trim().to_string(),
        zip_code: fields[18].trim().to_string(),
        frn: fields.get(22).unwrap_or(&"").trim().to_string(),
    })
}

/// Parse an AM.dat line (pipe-delimited).
/// Fields: record_type|usi|uls_file_number|ebf_number|call_sign|
///         operator_class[5]|group_code[6]|region_code[7]|...|previous_call_sign[15]|previous_operator_class[16]
pub fn parse_am_line(line: &str) -> Option<AmRecord> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 9 || fields[0] != "AM" {
        return None;
    }
    let usi = fields[1].trim().parse::<i64>().ok()?;
    Some(AmRecord {
        usi,
        operator_class: fields[5].trim().to_string(),
        group_code: fields.get(6).unwrap_or(&"").trim().to_string(),
        region_code: fields.get(7).unwrap_or(&"").trim().to_string(),
        previous_operator_class: fields.get(16).unwrap_or(&"").trim().to_string(),
        previous_call_sign: fields.get(15).unwrap_or(&"").trim().to_string(),
    })
}

/// Parse an HS.dat line (pipe-delimited).
/// Fields: record_type|usi|uls_file_number|call_sign|log_date|code
pub fn parse_hs_line(line: &str) -> Option<HsRecord> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 6 || fields[0] != "HS" {
        return None;
    }
    let usi = fields[1].trim().parse::<i64>().ok()?;
    Some(HsRecord {
        usi,
        log_date: fields[4].trim().to_string(),
        code: fields[5].trim().to_string(),
    })
}

/// Parse a CO.dat line (pipe-delimited).
/// Fields: record_type|usi|uls_file_number|call_sign|comment_date|comment|status_code|status_date
pub fn parse_co_line(line: &str) -> Option<CoRecord> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 6 || fields[0] != "CO" {
        return None;
    }
    let usi = fields[1].trim().parse::<i64>().ok()?;
    let comment = fields[4].trim().to_string();
    let comment_text = fields[5].trim().to_string();
    if comment_text.is_empty() {
        return None;
    }
    Some(CoRecord {
        usi,
        comment_date: comment,
        comment: comment_text,
        status_code: fields.get(6).unwrap_or(&"").trim().to_string(),
        status_date: fields.get(7).unwrap_or(&"").trim().to_string(),
    })
}

/// Convert Latin-1 (ISO-8859-1) bytes to UTF-8 string.
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    let (text, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hd_line() {
        // Real FCC record: AA0GV renewal
        let line = "HD|215148|0011928619||AA0GV|A|HA|03/04/2026|05/02/2036||||||||||N||||||||||N||GAIL|E|HURD||||||||||03/04/2026|03/04/2026|||||||||||||||";
        let rec = parse_hd_line(line).unwrap();
        assert_eq!(rec.usi, 215148);
        assert_eq!(rec.call_sign, "AA0GV");
        assert_eq!(rec.license_status, "A");
        assert_eq!(rec.radio_service_code, "HA");
        assert_eq!(rec.grant_date, "03/04/2026");
        assert_eq!(rec.expired_date, "05/02/2036");
    }

    #[test]
    fn test_parse_en_line() {
        // Real FCC record: AA0GV entity
        let line = "EN|215148|||AA0GV|L|L00612755|HURD, GAIL E|GAIL|E|HURD|||||52527 849th Rd|NELIGH|NE|68756|||000|0008143463|I||||||";
        let rec = parse_en_line(line).unwrap();
        assert_eq!(rec.usi, 215148);
        assert_eq!(rec.entity_type, "L");
        assert_eq!(rec.licensee_id, "L00612755");
        assert_eq!(rec.entity_name, "HURD, GAIL E");
        assert_eq!(rec.first_name, "GAIL");
        assert_eq!(rec.mi, "E");
        assert_eq!(rec.last_name, "HURD");
        assert_eq!(rec.street_address, "52527 849th Rd");
        assert_eq!(rec.city, "NELIGH");
        assert_eq!(rec.state, "NE");
        assert_eq!(rec.zip_code, "68756");
        assert_eq!(rec.frn, "0008143463");
    }

    #[test]
    fn test_parse_am_line() {
        // Real FCC record: AB7TH extra class, previous call KK7CY
        // AM.dat has 18 fields; previous_call_sign at index 15, previous_operator_class at index 16
        let line = "AM|222575|||AB7TH|E|A|7||||||||KK7CY|A|";
        let rec = parse_am_line(line).unwrap();
        assert_eq!(rec.usi, 222575);
        assert_eq!(rec.operator_class, "E");
        assert_eq!(rec.group_code, "A");
        assert_eq!(rec.region_code, "7");
        assert_eq!(rec.previous_operator_class, "A");
        assert_eq!(rec.previous_call_sign, "KK7CY");
    }

    #[test]
    fn test_parse_am_line_no_previous() {
        // Record with no previous callsign (empty at index 15)
        let line = "AM|215148|||AA0GV|E|A|10||||||||||";
        let rec = parse_am_line(line).unwrap();
        assert_eq!(rec.usi, 215148);
        assert_eq!(rec.operator_class, "E");
        assert_eq!(rec.previous_call_sign, "");
    }

    #[test]
    fn test_parse_hs_line() {
        // Real FCC record: AA0GV upgrade
        let line = "HS|215148||AA0GV|01/17/2003|LIAUA ";
        let rec = parse_hs_line(line).unwrap();
        assert_eq!(rec.usi, 215148);
        assert_eq!(rec.log_date, "01/17/2003");
        assert_eq!(rec.code, "LIAUA");

        // Real FCC record: AA0GV renewal
        let line2 = "HS|215148||AA0GV|03/21/2006|LIREN ";
        let rec2 = parse_hs_line(line2).unwrap();
        assert_eq!(rec2.usi, 215148);
        assert_eq!(rec2.log_date, "03/21/2006");
        assert_eq!(rec2.code, "LIREN");
    }

    #[test]
    fn test_parse_co_line() {
        let line = "CO|258535||K6USA|03/04/2026|Per case 1598925 licensee deceased on 12/18/2025.  cpg||";
        let rec = parse_co_line(line).unwrap();
        assert_eq!(rec.usi, 258535);
        assert_eq!(rec.comment_date, "03/04/2026");
        assert_eq!(
            rec.comment,
            "Per case 1598925 licensee deceased on 12/18/2025.  cpg"
        );
    }

    #[test]
    fn test_parse_co_line_empty_comment() {
        // Empty comment text should return None
        let line = "CO|258535||K6USA|03/04/2026|||";
        assert!(parse_co_line(line).is_none());
    }

    #[test]
    fn test_parse_bad_lines() {
        assert!(parse_hd_line("").is_none());
        assert!(parse_hd_line("EN|123|").is_none());
        assert!(parse_en_line("HD|123|").is_none());
        assert!(parse_am_line("not a record").is_none());
        assert!(parse_hs_line("HS|bad_usi|").is_none());
    }

    #[test]
    fn test_latin1_to_utf8() {
        // Plain ASCII
        assert_eq!(latin1_to_utf8(b"hello"), "hello");
        // Latin-1 byte 0xE9 = é
        assert_eq!(latin1_to_utf8(&[0x4A, 0x6F, 0x73, 0xE9]), "José");
    }
}
