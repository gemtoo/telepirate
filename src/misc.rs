use std::fs::remove_dir_all;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

use std::ffi::OsStr;
use std::process::Command;
use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::task::mediatype::MediaType;

#[tracing::instrument(skip_all)]
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
    check_dependency("magick");
    check_dependency("jpegoptim");
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
    pub fn from(path_to_directory: &str, extension: MediaType) -> Self {
        let extension_str = extension.as_str();

        // For counting video downloads, use jpg thumbnails, as yt-dlp intermediate objects are hard to track
        let count_extension = if extension_str == "mp4" {
            "jpg"
        } else {
            extension_str
        };

        // Collect files for counting (thumbnails for mp4, actual files for others)
        let files_to_count_amount: Vec<DirEntry> = WalkDir::new(path_to_directory)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .filter(|entry| entry.path().extension() != Some(OsStr::new("part")))
            .filter(|entry| entry.path().extension() == Some(OsStr::new(count_extension)))
            .collect();

        let file_count = files_to_count_amount.len();

        // Calculate total size: use mp4 files for size if media type is mp4, otherwise use counted files
        let mut size_in_bytes: usize = 0;

        for entry in WalkDir::new(path_to_directory)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                // As per documentation, this unwrap returns errors for path values 
                // that the program does not have permissions to access or if the path does not exist.
                // This is not our case so this unwrap is safe.
                size_in_bytes += entry.metadata().unwrap().len() as usize;
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

// This function compresses a thumbnail to adhere to Telegram's thumbnail requirements
#[tracing::instrument(skip_all)]
pub fn compress_thumbnail(path: &mut PathBuf) -> Result<(), String> {
    debug!("Compressing ...");
    // Create new path with .jpeg extension
    let new_path = path.with_extension("jpeg");

    // Create temporary path with .tmp.jpeg extension
    let temp_path = {
        let mut temp = path.clone();
        temp.set_file_name(format!(
            ".{}.tmp.jpeg",
            path.file_stem()
                .and_then(OsStr::to_str)
                .ok_or_else(|| "Invalid filename".to_string())?
        ));
        temp
    };

    // Execute conversion pipeline
    let status = Command::new("sh")
        .arg("-c")
        .arg(
            r#"
            {
                convert "$1" -auto-orient -resize '320x320>' -strip - 2>/dev/null | \
                jpegoptim --size=199k --stdin --stdout > "$2" 2>/dev/null && \
                mv -f "$2" "$3" 2>/dev/null
            } >/dev/null 2>&1
            "#,
        )
        .arg("--") // End of options marker
        .arg(path.as_os_str()) // $1: Original .jpg file
        .arg(temp_path.as_os_str()) // $2: Temp file
        .arg(new_path.as_os_str()) // $3: New .jpeg file
        .status()
        .map_err(|e| format!("Command execution failed: {}", e))?;

    if status.success() {
        // Update original path to point to the new .jpeg file
        *path = new_path;
        Ok(())
    } else {
        // Clean up temp file if conversion failed
        let _ = std::fs::remove_file(&temp_path);
        Err(format!(
            "Processing failed with exit code: {}",
            status.code().unwrap_or(-1)
        ))
    }
}

#[derive(Debug, Default)]
pub struct Metadata {
    pub width: u32,
    pub height: u32,
    pub duration: u32,
}

pub fn get_video_metadata(path: &PathBuf) -> Metadata {
    // Try to get path as string, return defaults on failure
    let path_str = match path.as_os_str().to_str() {
        Some(p) => p,
        None => return Metadata::default(),
    };

    // Execute ffprobe command
    let output = match Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            path_str,
        ])
        .output()
    {
        Ok(out) => out,
        Err(_) => return Metadata::default(),
    };

    // Check if command executed successfully
    if !output.status.success() {
        return Metadata::default();
    }

    // Parse JSON output
    let json_output = match str::from_utf8(&output.stdout) {
        Ok(json) => json,
        Err(_) => return Metadata::default(),
    };

    parse_ffprobe_output(json_output).unwrap_or_default()
}

fn parse_ffprobe_output(json: &str) -> Result<Metadata, ()> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Err(()),
    };

    // Extract width and height with error handling
    let width = value["streams"][0]["width"]
        .as_u64()
        .map(|w| w as u32)
        .unwrap_or(0);

    let height = value["streams"][0]["height"]
        .as_u64()
        .map(|h| h as u32)
        .unwrap_or(0);

    // Extract duration and convert to u32 seconds
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| d.round() as u32)
        .unwrap_or(0);

    Ok(Metadata {
        width,
        height,
        duration,
    })
}
