//! Shared data types for downloadable media.

use serde::{Deserialize, Serialize};

/// MP3 bitrates offered in the UI, in kbps.
pub const MP3_BITRATES: [u32; 4] = [128, 192, 256, 320];
pub const DEFAULT_MP3_KBPS: u32 = 192;
/// Height ladder used when the real formats are not known (search results).
pub const DEFAULT_HEIGHTS: [u32; 5] = [1440, 1080, 720, 480, 360];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Audio,
    Video,
}

/// Output format of one download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    Mp3 {
        kbps: u32,
    },
    /// MP4 video. `None` means the best available height.
    Video {
        max_height: Option<u32>,
    },
}

impl Format {
    pub fn kind(self) -> Kind {
        match self {
            Format::Mp3 { .. } => Kind::Audio,
            Format::Video { .. } => Kind::Video,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Mp3 { .. } => "mp3",
            Format::Video { .. } => "mp4",
        }
    }

    pub fn label(self) -> String {
        match self {
            Format::Mp3 { kbps } => format!("MP3 {kbps} kbps"),
            Format::Video {
                max_height: Some(h),
            } => format!("MP4 {h}p"),
            Format::Video { max_height: None } => "MP4 best".to_string(),
        }
    }
}

/// One video or song, from a search, a playlist or a single URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub url: String,
    pub channel: Option<String>,
    pub duration_secs: Option<u64>,
    pub thumbnail: Option<String>,
}

impl Entry {
    /// Key used to detect duplicate downloads.
    pub fn key(&self) -> &str {
        if self.id.is_empty() {
            &self.url
        } else {
            &self.id
        }
    }
}

/// Result of resolving a URL or a search query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub entries: Vec<Entry>,
    /// Video heights offered by a single video, highest first. Empty for lists.
    pub heights: Vec<u32>,
}

pub fn format_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_speed(bytes_per_sec: f64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec / MB)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds() {
        assert_eq!(format_speed(512.0 * 1024.0), "512 KB/s");
        assert_eq!(format_speed(3.5 * 1024.0 * 1024.0), "3.5 MB/s");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(5), "0:05");
        assert_eq!(format_duration(242), "4:02");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn format_properties() {
        assert_eq!(Format::Mp3 { kbps: 192 }.extension(), "mp3");
        assert_eq!(Format::Video { max_height: None }.kind(), Kind::Video);
        assert_eq!(
            Format::Video {
                max_height: Some(720)
            }
            .label(),
            "MP4 720p"
        );
    }
}
