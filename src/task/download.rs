use super::id::TaskId;
use super::mediatype::MediaType;
use super::stats::*;
use super::traits::*;
use crate::misc::*;
use crate::trackedmessage::TrackedMessage;
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
use tokio_util::sync::CancellationToken;
use url::Url;
use crate::task::cancellation::TASK_REGISTRY;

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
            .send_and_remember_msg("Preparing the download...", bot.clone(), db.clone())
            .await?;

        let last_message = tracked_messages[0].clone();

        let downloads_result = self
            .download_and_send_files(last_message, bot.clone(), db.clone())
            .await;
        match downloads_result {
            Err(error) => {
                warn!("{error}");
                self.send_and_remember_msg(&error.to_string(), bot.clone(), db)
                    .await?;
                Err(error)
            }
            Ok(_) => {
                trace!(
                    "All files ready. Stopping poller task for Chat ID {} ...",
                    self.chat_id
                );

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
    async fn download_and_send_files(
        &self,
        last_message: TrackedMessage,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> HandlerResult {
        let poller_cancellation_token_tx = CancellationToken::new();
        let poller_cancellation_token_rx = poller_cancellation_token_tx.clone();
        let bot_for_poller = bot.clone();
        let poller_handle = tokio::spawn(async move {
            if let Err(e) = last_message.directory_size_poller_and_message_updater(poller_cancellation_token_rx, bot_for_poller).await {
                warn!("{}", e);
            }
        });
        let yt_dlp_args = generate_yt_dlp_args(self.media_type, self.url.clone().unwrap());
        // UUID is used to name path so that a second concurrent Tokio task can gather info from that path.
        let absolute_destination_path = &construct_destination_path(self.task_id().to_string());
        // Cleanup here is needed in case the task was respawned after interruption.
        // We need to start from 0 because existing artifacts result in corrupted downloads.
        cleanup(absolute_destination_path.into());
        let path = PathBuf::from(absolute_destination_path);
        // This unwrap should work as long as the registry is implemented correctly
        let task_cancellation_token = TASK_REGISTRY.get_token(self.task_id()).unwrap();
        let ytdresult = yt_dlp(path, yt_dlp_args, task_cancellation_token).await;
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
        // If count of files is 0 then it is an error even if yt-dlp doesn't think so.
        // For example a file can be larger than 2GB thus not sendable by the bot.
        if file_amount == 0 {
            poller_cancellation_token_tx.cancel();
            // Await poller handle before cleanup to avoid sending incorrect data to user.
            poller_handle.await?;
            cleanup(absolute_destination_path.into());
            let error_text;
            match ytdresult {
                Ok(traceback) => {
                    error_text = format!("{traceback:?}");
                    return Err(error_text.into());
                }
                Err(traceback) => {
                    return Err(traceback);
                }
            }
        }
        // Stop poller task here.
        poller_cancellation_token_tx.cancel();
        // Send files in alphabetic order.
        for path in paths {
            self.send_file(&path, bot.clone(), db.clone()).await?;
        }
        // Await poller handle before cleanup to avoid sending incorrect data to user.
        poller_handle.await?;
        cleanup(absolute_destination_path.into());
        Ok(())
    }
}

use crate::FILE_STORAGE;
pub fn construct_destination_path(task_id: String) -> String {
    format!("{FILE_STORAGE}/{task_id}")
}

fn generate_yt_dlp_args(media_type: MediaType, url: Url) -> Vec<String> {
    match media_type {
        MediaType::Mp3 => {
            vec![
                String::from("--concurrent-fragments"),
                String::from("1"),
                String::from("--skip-playlist-after-errors"),
                String::from("5000"),
                String::from("--output"),
                String::from("%(title)s.mp3"),
                String::from("--windows-filenames"),
                String::from("--no-write-info-json"),
                String::from("--no-embed-metadata"),
                String::from("--extract-audio"),
                String::from("--write-thumbnail"),
                String::from("--convert-thumbnails"),
                String::from("jpg"),
                String::from("--audio-format"),
                String::from("mp3"),
                String::from("--audio-quality"),
                String::from("0"),
                String::from(url),
            ]
        }
        MediaType::Mp4 => {
            vec![
                String::from("--concurrent-fragments"),
                String::from("1"),
                String::from("--skip-playlist-after-errors"),
                String::from("5000"),
                String::from("--max-filesize"),
                String::from("2000M"),
                String::from("--output"),
                String::from("%(title)s.mp4"),
                String::from("--windows-filenames"),
                String::from("--no-write-info-json"),
                String::from("--no-embed-metadata"),
                String::from("--write-thumbnail"),
                String::from("--convert-thumbnails"),
                String::from("jpg"),
                String::from("--format"),
                String::from("bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]"),
                String::from(url),
            ]
        }
        MediaType::Voice => {
            vec![
                String::from("--concurrent-fragments"),
                String::from("1"),
                String::from("--skip-playlist-after-errors"),
                String::from("5000"),
                String::from("--windows-filenames"),
                String::from("--no-write-info-json"),
                String::from("--no-embed-metadata"),
                String::from("--extract-audio"),
                String::from("--write-thumbnail"),
                String::from("--convert-thumbnails"),
                String::from("jpg"),
                String::from("--audio-format"),
                String::from("opus"),
                String::from("--audio-quality"),
                String::from("64K"),
                String::from(url),
            ]
        }
    }
}

use tokio::process::Command;
use tokio::io::AsyncReadExt;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tracing::instrument(skip_all)]
async fn yt_dlp(
    path: PathBuf,
    args: Vec<String>,
    cancellation_token: CancellationToken,
) -> Result<std::process::Output, Box<dyn Error + Send + Sync>> {
    debug!("Downloading ...");
    let mut cmd = Command::new("yt-dlp");
    std::fs::create_dir_all(&path)?;
    cmd.current_dir(&path)
        .env("LC_ALL", "en_US.UTF-8")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Add all arguments
    for arg in args {
        cmd.arg(arg);
    }

    // Spawn the child process
    let mut child = cmd.spawn()?;
    // Get handles to stdout and stderr
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let stderr = child.stderr.take().expect("Failed to capture stderr");

    // Create readers for the streams
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();
    let current_span_1 = tracing::Span::current();
    let current_span_2 = tracing::Span::current();

    // Read from both streams concurrently
    let stdout_task = tokio::spawn(async move {
        while let Some(line) = stdout_reader.next_line().await.unwrap() {
            tracing::trace!(parent: current_span_1.clone(), "stdout: {}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        while let Some(line) = stderr_reader.next_line().await.unwrap() {
            tracing::warn!(parent: current_span_2.clone(), "stderr: {}", line);
        }
    });

    // Wait for the output processing tasks to complete
    let _ = tokio::join!(stdout_task, stderr_task);

    // Use select! to wait for either completion or cancellation
    tokio::select! {
        // Wait for the process to complete normally
        status = child.wait() => {
            match status {
                Ok(exit_status) => {
                    // Read stdout and stderr
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    
                    if let Some(mut out) = child.stdout.take() {
                        out.read_to_end(&mut stdout).await?;
                    }
                    
                    if let Some(mut err) = child.stderr.take() {
                        err.read_to_end(&mut stderr).await?;
                    }
                    
                    Ok(std::process::Output {
                        status: exit_status,
                        stdout,
                        stderr,
                    })
                }
                Err(e) => Err(Box::new(e)),
            }
        }
        // Handle cancellation
        _ = cancellation_token.cancelled() => {
            // Kill the child process
            if let Err(e) = child.kill().await {
                warn!("Failed to kill child process: {}", e);
            }
            
            // Wait for the process to exit to avoid zombies
            let _ = child.wait_with_output().await;
            
            Err("Operation cancelled.".into())
        }
    }
}