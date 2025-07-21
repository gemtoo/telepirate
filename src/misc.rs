use std::fs::remove_dir_all;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::pirate::MediaType;

#[tracing::instrument]
pub fn cleanup(absolute_destination_path: PathBuf) {
    trace!("Deleting files ...");
    remove_dir_all(absolute_destination_path).unwrap();
}

pub fn update() {
    stdout().flush().unwrap();
}

#[tracing::instrument]
pub fn boot() {
    use crate::tracing;
    tracing::init();
    check_dependency("yt-dlp");
    check_dependency("ffmpeg");
    let _ = ctrlc::set_handler(move || {
        info!("Stopping ...");
        update();
        std::process::exit(0);
    });
}

#[tracing::instrument(skip_all)]
fn check_dependency(dep: &str) {
    trace!("{} ...", dep);
    let result_output = std::process::Command::new(dep).arg("--help").output();
    if let Err(e) = result_output
        && let std::io::ErrorKind::NotFound = e.kind()
    {
        error!("{dep} is not found. Please install {dep} first.");
        std::process::exit(1);
    }
}

pub async fn sleep(secs: u32) {
    let time = Duration::from_secs(secs.into());
    tokio::time::sleep(time).await;
}

pub fn die(reason: impl Into<String>) -> ! {
    error!("{}", reason.into());
    std::process::exit(1);
}
pub struct FolderData {
    pub size_in_bytes: usize,
    pub file_count: usize,
}

impl FolderData {
    // This function counts files and their respective size.
    pub fn from(path_to_directory: &str, extension: MediaType) -> Self {
        let extension_str = extension.as_str();
        // Collect all files of a certain extension.
        let files: Vec<DirEntry>;
        if extension_str.contains("mp3") {
            files = WalkDir::new(path_to_directory)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_file())
                .filter(|entry| {
                    entry.path().extension() == Some(std::ffi::OsStr::new(extension_str))
                })
                .collect();
        } else {
            files = WalkDir::new(path_to_directory)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_file())
                // Filtering by extension is not used in non-mp3 cases because of how yt-dlp handles these files.
                .collect();
        }
        let file_count = files.len();
        let mut size_in_bytes = 0;

        for entry in files {
            if entry.file_type().is_file() {
                // This unwrap is ok as long as we run as root in Docker.
                size_in_bytes += std::fs::metadata(entry.path()).unwrap().len() as usize;
            }
        }
        FolderData {
            size_in_bytes,
            file_count,
        }
    }
    pub fn format_bytes_to_megabytes(&self) -> String {
        format!("{:.2} MB", self.size_in_bytes as f64 / (1024.0 * 1024.0))
    }
}

// Telegram limits message length to 4096 chars. Thus the message is split into sendable chunks.
pub fn split_text(text: &str) -> Vec<String> {
    if text.len() > 4096 * 4 {
        let stringvec =
            vec!["Error traceback is too large. Read the logs for more info.".to_string()];
        return stringvec;
    } else if text.len() > 4096 {
        let stringvec = text
            .as_bytes()
            .chunks(4096)
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<Vec<String>>();
        return stringvec;
    }
    let stringvec = vec![text.to_string()];
    stringvec
}
