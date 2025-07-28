use super::id::TaskId;
use super::mediatype::MediaType;
use super::stats::*;
use super::traits::*;
use crate::misc::*;
use glob::glob;
use humantime::format_rfc3339_seconds as timestamp;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::path::PathBuf;
use std::time::SystemTime;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::sync::watch;
use url::Url;
use ytd_rs::{Arg, YoutubeDL};

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDownload {
    pub task_id: TaskId,
    pub chat_id: ChatId,
    pub media_type: MediaType,
    // Option because at the intermediate stage WaitingForUrl it is known that the task is Download but initial URL is None.
    pub url: Option<Url>,
    //started_at: Utc,
}
impl HasTaskId for TaskDownload {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskDownload {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}
impl Task for TaskDownload {}
impl TaskDownload {
    fn url(&self) -> Option<Url> {
        return self.url.clone();
    }
    pub fn set_url(&mut self, url: Url) {
        self.url = Some(url);
    }
    fn media_type(&self) -> MediaType {
        self.media_type
    }
    pub fn to_task_stats(&self) -> TaskStats {
        TaskStats {
            task_id: self.task_id(),
            chat_id: self.chat_id(),
            media_type: self.media_type(),
            // This unwrap is safe because TaskState::Running is not possible without URL.
            url: self.url().unwrap(),
        }
    }
    #[tracing::instrument(skip_all)]
    pub async fn process_request(&self, bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
        debug!("Processing request ...");
        let tracked_messages = self
            .send_and_remember_msg("Downloading... Please wait.", bot.clone(), db.clone())
            .await?;

        let last_message = tracked_messages[0].clone();

        let (tx, rx) = watch::channel(false);

        let poller_handle = last_message
            .directory_size_poller_and_mesage_updater(rx, self.media_type(), bot.clone())
            .await?;
        let downloads_result = self.download_and_send_files(bot.clone(), db.clone()).await;

        match downloads_result {
            Err(error) => {
                warn!("Download failed: {}", error);
                let _ = tx.send(true);
                poller_handle.await?;
                self.send_and_remember_msg(&error.to_string(), bot.clone(), db)
                    .await?;
                Err(error)
            }
            Ok(_) => {
                trace!(
                    "All files ready. Stopping poller task for Chat ID {} ...",
                    self.chat_id
                );

                let _ = tx.send(true);
                poller_handle.await?;

                self.delete_messages_by_task_id(bot.clone(), db.clone())
                    .await?;
                // Cleanup is done to save space on disk and to remove pirating evidence
                Ok(())
            }
        }
    }
    #[tracing::instrument(skip_all)]
    async fn send_file(&self, path: &PathBuf, bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
        let file = InputFile::file(path);
        let filename_display = path.display().to_string();
        let max_retries = 10;

        Ok(for attempt in 1..=max_retries {
            let result = match self.media_type() {
                MediaType::Mp3 => bot.send_audio(self.chat_id(), file.clone()).await,
                MediaType::Mp4 => {
                    // The backend downloads videos in .mp4 and places .jpg thumbnail next to the video in the same folder with the same base name.
                    let video_metadata = get_video_metadata(path);
                    let mut thumbnail_path = path.with_extension("jpg");
                    let thumbnail_file = tokio::task::spawn_blocking(move || {
                        compress_thumbnail(&mut thumbnail_path).unwrap();
                        InputFile::file(thumbnail_path)
                    })
                    .await
                    .unwrap();
                    bot.send_video(self.chat_id(), file.clone())
                        .thumbnail(thumbnail_file)
                        .duration(video_metadata.duration)
                        .height(video_metadata.height)
                        .width(video_metadata.width)
                        .await
                }
                MediaType::Voice => bot.send_voice(self.chat_id(), file.clone()).await,
            };

            match result {
                Ok(_) => {
                    info!("File '{filename_display}' sent successfully.");
                    return Ok(());
                }
                Err(error) => {
                    sleep(10).await;
                    let error_text = format!(
                        "Attempt {attempt}/{max_retries} at sending '{filename_display}' failed: {error}"
                    );
                    warn!("{}", error_text);

                    if attempt < max_retries {
                        self.send_and_remember_msg(&error_text, bot.clone(), db.clone())
                            .await?;
                    }
                    //Err(format!("Failed to send file after {max_retries} attempts: {filename_display}").into())
                }
            }
        })
    }
    #[tracing::instrument(skip_all, fields(task_id = %self.task_id()))]
    async fn download_and_send_files(&self, bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
        let yt_dlp_args = generate_yt_dlp_args(self.media_type);
        debug!("Downloading ...");
        // UUID is used to name path so that a second concurrent Tokio task can gather info from that path.
        let absolute_destination_path = &construct_destination_path(self.task_id().to_string());
        // Cleanup here is needed in case the task was respawned after interruption.
        // We need to start from 0 because existing artifacts result in corrupted downloads.
        cleanup(absolute_destination_path.into());
        let path = PathBuf::from(absolute_destination_path);
        let ytd = YoutubeDL::new(&path, yt_dlp_args, self.url().unwrap().as_str())?;
        let ytdresult = tokio::task::spawn_blocking(move || ytd.download()).await?;
        let mut paths: Vec<PathBuf> = Vec::new();
        let regex = Regex::new(r"(.*)(\.opus)").unwrap();
        let filepaths = glob(&format!(
            "{absolute_destination_path}/*{}",
            self.media_type().as_str()
        ))?;
        for entry in filepaths {
            if let Ok(mut file_path) = entry {
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
                        let newname = format!("{absolute_destination_path}/audio_{timestamp}.ogg");
                        std::fs::rename(oldname, &newname)?;
                        file_path = PathBuf::from(newname);
                    }
                    paths.push(file_path);
                } else {
                    trace!("Skipping large file {filename}");
                }
            }
        }
        let file_amount = paths.len();
        trace!("{file_amount} {}(s) to send.", self.media_type());
        if file_amount == 0 {
            cleanup(absolute_destination_path.into());
            let error_text;
            match ytdresult {
                Ok(traceback) => {
                    error_text =
                        format!("{traceback:?}\n\nFiles larger than 2GB are not supported.");
                }
                Err(e) => {
                    error_text = e.to_string();
                }
            }
            return Err(error_text.into());
        }
        for path in paths {
            self.send_file(&path, bot.clone(), db.clone()).await?;
        }
        cleanup(absolute_destination_path.into());
        Ok(())
    }
}

use crate::FILE_STORAGE;
pub fn construct_destination_path(task_id: String) -> String {
    format!("{FILE_STORAGE}/{task_id}")
}

fn generate_yt_dlp_args(media_type: MediaType) -> Vec<Arg> {
    match media_type {
        MediaType::Mp3 => {
            vec![
                Arg::new_with_arg("--concurrent-fragments", "1"),
                Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
                Arg::new_with_arg("--output", "%(title)s.mp3"),
                Arg::new("--windows-filenames"),
                Arg::new("--no-write-info-json"),
                Arg::new("--no-embed-metadata"),
                Arg::new("--extract-audio"),
                Arg::new_with_arg("--audio-format", "mp3"),
                Arg::new_with_arg("--audio-quality", "0"),
            ]
        }
        MediaType::Mp4 => {
            vec![
                Arg::new_with_arg("--concurrent-fragments", "1"),
                Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
                Arg::new_with_arg("--max-filesize", "2000M"),
                Arg::new_with_arg("--output", "%(title)s.mp4"),
                Arg::new("--windows-filenames"),
                Arg::new("--no-write-info-json"),
                Arg::new("--no-embed-metadata"),
                Arg::new("--write-thumbnail"),
                Arg::new_with_arg("--convert-thumbnails", "jpg"),
                Arg::new_with_arg("--format", "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]"),
            ]
        }
        MediaType::Voice => {
            vec![
                Arg::new_with_arg("--concurrent-fragments", "1"),
                Arg::new_with_arg("--skip-playlist-after-errors", "5000"),
                Arg::new("--windows-filenames"),
                Arg::new("--no-write-info-json"),
                Arg::new("--no-embed-metadata"),
                Arg::new("--extract-audio"),
                Arg::new_with_arg("--audio-format", "opus"),
                Arg::new_with_arg("--audio-quality", "64K"),
            ]
        }
    }
}
