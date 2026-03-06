use std::io::{Cursor, Read};
use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use crate::error::{FccError, Result};
use crate::parse::latin1_to_utf8;

const FCC_FULL_URL: &str = "https://data.fcc.gov/download/pub/uls/complete/l_amat.zip";

/// Day-of-week for daily update downloads.
#[derive(Debug, Clone, Copy)]
pub enum Day {
    Sun,
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
}

impl Day {
    pub fn suffix(&self) -> &str {
        match self {
            Day::Sun => "sun",
            Day::Mon => "mon",
            Day::Tue => "tue",
            Day::Wed => "wed",
            Day::Thu => "thu",
            Day::Fri => "fri",
            Day::Sat => "sat",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sun" | "sunday" => Some(Day::Sun),
            "mon" | "monday" => Some(Day::Mon),
            "tue" | "tuesday" => Some(Day::Tue),
            "wed" | "wednesday" => Some(Day::Wed),
            "thu" | "thursday" => Some(Day::Thu),
            "fri" | "friday" => Some(Day::Fri),
            "sat" | "saturday" => Some(Day::Sat),
            _ => None,
        }
    }

    pub fn from_chrono_weekday(wd: chrono::Weekday) -> Self {
        match wd {
            chrono::Weekday::Mon => Day::Mon,
            chrono::Weekday::Tue => Day::Tue,
            chrono::Weekday::Wed => Day::Wed,
            chrono::Weekday::Thu => Day::Thu,
            chrono::Weekday::Fri => Day::Fri,
            chrono::Weekday::Sat => Day::Sat,
            chrono::Weekday::Sun => Day::Sun,
        }
    }

    fn daily_url(&self) -> String {
        format!(
            "https://data.fcc.gov/download/pub/uls/daily/l_am_{}.zip",
            self.suffix()
        )
    }
}

/// Downloaded and extracted FCC data files.
pub struct ExtractedData {
    pub hd_data: Option<String>,
    pub en_data: Option<String>,
    pub am_data: Option<String>,
    pub hs_data: Option<String>,
    pub co_data: Option<String>,
}

/// Download the full FCC amateur database ZIP and extract .dat files.
pub async fn download_full() -> Result<ExtractedData> {
    download_and_extract(FCC_FULL_URL).await
}

/// Download a daily update ZIP and extract .dat files.
pub async fn download_daily(day: Day) -> Result<ExtractedData> {
    download_and_extract(&day.daily_url()).await
}

/// Download a URL with a progress bar, returning the raw bytes.
async fn download_bytes(url: &str) -> Result<Vec<u8>> {
    info!("Downloading {}", url);

    let client = reqwest::Client::new();
    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        return Err(FccError::Download(format!(
            "HTTP {} from {}",
            resp.status(),
            url
        )));
    }

    let total_size = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_message("Downloading");

    let bytes = resp.bytes().await?;
    pb.set_position(bytes.len() as u64);
    pb.finish_with_message("Download complete");

    Ok(bytes.to_vec())
}

/// Download a ZIP from a URL and write it to a local file.
pub async fn download_zip_to_file(url: &str, dest: &Path) -> Result<()> {
    let bytes = download_bytes(url).await?;
    info!("Writing {} bytes to {}", bytes.len(), dest.display());
    tokio::fs::write(dest, &bytes).await?;
    Ok(())
}

async fn download_and_extract(url: &str) -> Result<ExtractedData> {
    let bytes = download_bytes(url).await?;
    info!("Downloaded {} bytes, extracting ZIP", bytes.len());
    extract_zip(&bytes)
}

fn extract_zip(data: &[u8]) -> Result<ExtractedData> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut extracted = ExtractedData {
        hd_data: None,
        en_data: None,
        am_data: None,
        hs_data: None,
        co_data: None,
    };

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_uppercase();

        let target = if name.ends_with("HD.DAT") {
            &mut extracted.hd_data
        } else if name.ends_with("EN.DAT") {
            &mut extracted.en_data
        } else if name.ends_with("AM.DAT") {
            &mut extracted.am_data
        } else if name.ends_with("HS.DAT") {
            &mut extracted.hs_data
        } else if name.ends_with("CO.DAT") {
            &mut extracted.co_data
        } else {
            continue;
        };

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        *target = Some(latin1_to_utf8(&buf));
        info!("Extracted {} ({} bytes)", file.name(), buf.len());
    }

    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_day_suffix() {
        assert_eq!(Day::Mon.suffix(), "mon");
        assert_eq!(Day::Sat.suffix(), "sat");
    }

    #[test]
    fn test_day_from_str() {
        assert!(matches!(Day::parse("mon"), Some(Day::Mon)));
        assert!(matches!(Day::parse("Monday"), Some(Day::Mon)));
        assert!(Day::parse("invalid").is_none());
    }
}
