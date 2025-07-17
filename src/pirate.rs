use std::error::Error;
use std::path::PathBuf;
use std::time::SystemTime;

use glob::glob;
use humantime::format_rfc3339_seconds as timestamp;
use regex::Regex;
use serde::{Deserialize, Serialize};
use ytd_rs::{Arg, YoutubeDL};

use crate::FILE_STORAGE;
use crate::misc::cleanup;

type DownloadsResult = Result<Downloads, Box<dyn Error + Send + Sync>>;

#[derive(Default, Debug, Clone)]
pub struct Downloads {
    pub paths: Vec<PathBuf>,
    pub folder: PathBuf,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "filetype")]
pub enum FileType {
    #[default]
    Mp3,
    Mp4,
    Voice,
}

impl FileType {
    pub fn as_str<'a>(&self) -> &'a str {
        return match self {
            FileType::Mp3 => "mp3",
            FileType::Mp4 => "mp4",
            FileType::Voice => "opus",
        };
    }
    pub fn from_callback_data(data: &str) -> Option<Self> {
        match data {
            "Audio" => Some(FileType::Mp3),
            "Video" => Some(FileType::Mp4),
            "Audio as voice message" => Some(FileType::Voice),
            _ => None,
        }
    }
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FileType::Mp3 => write!(f, "audio"),
            FileType::Mp4 => write!(f, "video"),
            FileType::Voice => write!(f, "voice message"),
        }
    }
}

pub fn mp3(url: String, download_id: String) -> DownloadsResult {
    let args = vec![
        Arg::new_with_arg("--concurrent-fragments", "100000"),
        Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
        Arg::new_with_arg("--output", "%(title)s.mp3"),
        Arg::new("--windows-filenames"),
        Arg::new("--no-write-info-json"),
        Arg::new("--no-embed-metadata"),
        Arg::new("--extract-audio"),
        Arg::new_with_arg("--audio-format", "mp3"),
        Arg::new_with_arg("--audio-quality", "0"),
    ];
    let filetype = FileType::Mp3;
    let downloaded = dl(url, args, filetype, download_id)?;
    Ok(downloaded)
}

pub fn mp4(url: String, download_id: String) -> DownloadsResult {
    let args = vec![
        Arg::new_with_arg("--concurrent-fragments", "100000"),
        Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
        Arg::new_with_arg("--max-filesize", "2000M"),
        Arg::new_with_arg("--output", "%(title)s.mp4"),
        Arg::new("--windows-filenames"),
        Arg::new("--no-write-info-json"),
        Arg::new("--no-embed-metadata"),
        Arg::new_with_arg("--format", "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]"),
    ];
    let filetype = FileType::Mp4;
    let downloaded = dl(url, args, filetype, download_id)?;
    Ok(downloaded)
}

pub fn ogg(url: String, download_id: String) -> DownloadsResult {
    let args = vec![
        Arg::new_with_arg("--concurrent-fragments", "100000"),
        Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
        Arg::new("--windows-filenames"),
        Arg::new("--no-write-info-json"),
        Arg::new("--no-embed-metadata"),
        Arg::new("--extract-audio"),
        Arg::new_with_arg("--audio-format", "opus"),
        Arg::new_with_arg("--audio-quality", "64K"),
    ];
    let filetype = FileType::Voice;
    let downloaded = dl(url, args, filetype, download_id)?;
    Ok(downloaded)
}

pub fn construct_destination_path(download_id: String) -> String {
    return format!("{}/{}", FILE_STORAGE, download_id);
}

fn dl(url: String, args: Vec<Arg>, filetype: FileType, download_id: String) -> DownloadsResult {
    debug!("Downloading {}(s) from {} ...", filetype.as_str(), url);
    // UUID is used to name path so that a second concurrent Tokio task can gather info from that path.
    let absolute_destination_path = &construct_destination_path(download_id);
    let path = PathBuf::from(absolute_destination_path);
    let ytd = YoutubeDL::new(&path, args, &url)?;
    let ytdresult = ytd.download();
    let mut paths: Vec<PathBuf> = Vec::new();
    let regex = Regex::new(r"(.*)(\.opus)").unwrap();
    let fileformat = filetype.as_str();
    let filepaths = glob(&format!("{}/*{}", absolute_destination_path, fileformat))?;
    for entry in filepaths {
        match entry {
            Ok(mut file_path) => {
                let filename = file_path.to_str().unwrap();
                // Local Telegram API allows bots sending only files under 2 GB.
                let filesize = file_path.metadata()?.len();
                if filesize < 2_000_000_000 {
                    // Rename .opus into .ogg because Telegram requires so to display wave pattern.
                    if let Some(captures) = regex.captures(filename) {
                        let oldname = captures.get(0).unwrap().as_str();
                        let timestamp = timestamp(SystemTime::now())
                            .to_string()
                            .replace(":", "-")
                            .replace("T", "_")
                            .replace("Z", "");
                        // Filename formatting that is used by Telegram when sending voice messages.
                        let newname =
                            format!("{}/audio_{}.ogg", absolute_destination_path, timestamp);
                        std::fs::rename(oldname, &newname)?;
                        file_path = PathBuf::from(newname);
                    }
                    paths.push(file_path);
                } else {
                    trace!("Skipping large file {}", filename);
                }
            }
            _ => {}
        }
    }
    let file_amount = paths.len();
    trace!("{} {}(s) to send.", file_amount, filetype.as_str());
    if file_amount == 0 {
        cleanup(absolute_destination_path.into());
        let error_text;
        match ytdresult {
            Ok(traceback) => {
                error_text = format!(
                    "{:?}\n\nFiles larger than 2GB are not supported.",
                    traceback
                );
            }
            Err(e) => {
                error_text = e.to_string();
            }
        }
        return Err(error_text.into());
    }
    let downloads = Downloads {
        paths,
        folder: absolute_destination_path.into(),
    };
    Ok(downloads)
}
