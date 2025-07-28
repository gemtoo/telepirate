use serde::{Deserialize, Serialize};
#[derive(Default, Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(tag = "filetype")]
pub enum MediaType {
    #[default]
    Mp3,
    Mp4,
    Voice,
}

impl MediaType {
    pub fn as_str<'a>(&self) -> &'a str {
        match self {
            MediaType::Mp3 => "mp3",
            MediaType::Mp4 => "mp4",
            MediaType::Voice => "opus",
        }
    }
    pub fn from_callback_data(data: &str) -> Option<Self> {
        match data {
            "Audio" => Some(MediaType::Mp3),
            "Video" => Some(MediaType::Mp4),
            "Audio as voice message" => Some(MediaType::Voice),
            _ => None,
        }
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            MediaType::Mp3 => write!(f, "audio"),
            MediaType::Mp4 => write!(f, "video"),
            MediaType::Voice => write!(f, "audio as voice message"),
        }
    }
}
