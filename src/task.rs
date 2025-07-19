use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_type_name::type_name;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use teloxide::types::{InlineKeyboardMarkup, InputFile};
use tokio::sync::watch;
use tracing::{Instrument, debug, error, trace, warn};
use uuid::Uuid;

use crate::{
    database::DbRecord,
    misc::*,
    pirate::{self, FileType},
};

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", content = "data")]
pub enum TaskState {
    New(TaskSession),
    // Waitingforurl can't have no media type in reality but it program's logic it can, so I have to redecide what to put here but that's for later
    WaitingForUrl(TaskSession),
    Running(TaskSession),
    Success(TaskSession),
    Failure(TaskSession),
}
// add so that /clear deletes all new/waitingforurl tasks from the dadabaze
impl DbRecord for TaskState {
    #[tracing::instrument(skip(self, db), fields(chat_id = %self.chat_id()))]
    async fn select_by_chat_id(
        &self,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self).unwrap();
        trace!("{} ...", type_name);
        // query_base because type_name can't be in .bind() because .bind() adds single brackets '' thus searching in the wrong table
        // the only thing that's changed from the default trait function is that data.chat_id is used instead of simply chat_id
        let query_base = format!("SELECT * FROM {type_name} WHERE data.chat_id = $chat_id_object");
        let object_array: Vec<Self> = db
            .query(&query_base)
            .bind(("chat_id_object", self.chat_id()))
            .await?
            .take(0)?;
        Ok(object_array)
    }
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn delete_by_task_id(
        &self,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self).unwrap();
        trace!("{} ...", type_name);
        // query_base because type_name can't be in .bind() because .bind() adds single brackets '' thus searching in the wrong table
        // the only thing that's changed from the default trait function is that data.task_id is used instead of simply task_id
        let query_base = format!("DELETE FROM {type_name} WHERE data.task_id = $task_id_object");
        let object_array: Vec<Self> = db
            .query(&query_base)
            .bind(("task_id_object", self.task_id()))
            .await?
            .take(0)?;
        Ok(object_array)
    }
}
impl HasTaskId for TaskState {
    fn task_id(&self) -> TaskId {
        match self {
            TaskState::New(session)
            | TaskState::WaitingForUrl(session)
            | TaskState::Running(session)
            | TaskState::Success(session)
            | TaskState::Failure(session) => session.task_id(),
        }
    }
}
impl HasChatId for TaskState {
    fn chat_id(&self) -> ChatId {
        match self {
            TaskState::New(session)
            | TaskState::WaitingForUrl(session)
            | TaskState::Running(session)
            | TaskState::Success(session)
            | TaskState::Failure(session) => session.chat_id(),
        }
    }
}
impl TaskState {
    pub fn try_from(msg_from_user: &Message) -> Result<Self, String> {
        Ok(Self::New(TaskSession::try_from(msg_from_user)?))
    }
    pub async fn select_from_db_by_chat_id(
        chat_id: ChatId,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let dummy_task_session = TaskSession {
            task_id: TaskId::new(),
            chat_id,
            media_type: None,
        };
        let dummy_task_state = Self::New(dummy_task_session);
        return dummy_task_state.select_by_chat_id(db).await;
    }
    pub fn to_waiting_for_url(self) -> Self {
        let session = self.into_session();
        TaskState::WaitingForUrl(session)
    }

    pub fn to_running(self) -> Self {
        let session = self.into_session();
        TaskState::Running(session)
    }

    pub fn to_success(self) -> Self {
        let session = self.into_session();
        TaskState::Success(session)
    }

    pub fn to_failure(self) -> Self {
        let session = self.into_session();
        TaskState::Failure(session)
    }

    // Access session from any state
    pub fn session(&self) -> &TaskSession {
        match self {
            TaskState::New(session)
            | TaskState::WaitingForUrl(session)
            | TaskState::Running(session)
            | TaskState::Success(session)
            | TaskState::Failure(session) => session,
        }
    }
    pub fn as_mut_session(&mut self) -> &mut TaskSession {
        match self {
            TaskState::New(session)
            | TaskState::WaitingForUrl(session)
            | TaskState::Running(session)
            | TaskState::Success(session)
            | TaskState::Failure(session) => session,
        }
    }

    // Consume state to extract session
    pub fn into_session(self) -> TaskSession {
        match self {
            TaskState::New(session)
            | TaskState::WaitingForUrl(session)
            | TaskState::Running(session)
            | TaskState::Success(session)
            | TaskState::Failure(session) => session,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskStats {
    task_id: TaskId,
    chat_id: ChatId,
}

pub trait HasTaskId {
    fn task_id(&self) -> TaskId;
}
pub trait HasChatId {
    fn chat_id(&self) -> ChatId;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TaskId {
    pub uuid: Uuid,
}

impl TaskId {
    pub fn new() -> Self {
        TaskId {
            uuid: Uuid::new_v4(),
        }
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.uuid)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedMessage {
    task_id: TaskId,
    pub message_id: MessageId,
    chat_id: ChatId,
}

impl TrackedMessage {
    pub fn try_from(task_id: TaskId, message: &Message) -> Result<Self, String> {
        Ok(Self {
            task_id,
            message_id: message.id,
            chat_id: message.chat_id().ok_or("Message has no chat_id")?,
        })
    }

    pub async fn from_db_by_task_id(
        task_id: TaskId,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let dummy = Self {
            task_id,
            message_id: MessageId(0),
            chat_id: ChatId(0),
        };
        dummy.select_by_task_id(db).await
    }
    #[tracing::instrument(skip(self, rx, filetype, bot))]
    pub async fn directory_size_poller_and_mesage_updater(
        &self,
        rx: watch::Receiver<bool>,
        filetype: FileType,
        bot: Bot,
    ) -> Result<tokio::task::JoinHandle<()>, Box<dyn Error + Send + Sync>> {
        debug!("Starting poller task ...");

        let path_to_downloads = pirate::construct_destination_path(self.task_id().to_string());
        if let Err(e) = std::fs::create_dir_all(&path_to_downloads) {
            return Err(format!("Failed to create directory: {e}").into());
        }
        let owned_tracked_message = self.clone();
        let poller_span = tracing::info_span!(
            "directory_size_poller_task",
            task_id = ?self.task_id(),
        );

        let handle = tokio::task::spawn(
            {
                async move {
                    let mut previous_update_text = String::new();

                    while !*rx.borrow() {
                        sleep(5).await;
                        trace!("Polling data ...");

                        let folder_data = FolderData::from(&path_to_downloads, filetype);

                        trace!(
                            "File count: {}. Size: {}.",
                            folder_data.file_count,
                            folder_data.format_bytes_to_megabytes()
                        );

                        let update_text = format!(
                            "Downloading... Please wait.\nFiles to send: {}.\nTotal size: {}.",
                            folder_data.file_count,
                            folder_data.format_bytes_to_megabytes(),
                        );

                        if update_text != previous_update_text {
                            previous_update_text = update_text.clone();

                            if let Err(e) = bot
                                .clone()
                                .edit_message_text(
                                    owned_tracked_message.chat_id(),
                                    owned_tracked_message.message_id,
                                    &update_text,
                                )
                                .await
                            {
                                warn!("Failed to update message: {}", e);
                            }
                        }
                    }
                    trace!("Poller task done.");
                }
            }
            .instrument(poller_span),
        );

        Ok(handle)
    }
}

impl DbRecord for TrackedMessage {}
impl HasTaskId for TrackedMessage {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}

impl HasChatId for TrackedMessage {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSession {
    task_id: TaskId,
    chat_id: ChatId,
    pub media_type: Option<FileType>,
}

impl DbRecord for TaskSession {}
impl HasTaskId for TaskSession {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskSession {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}

impl TaskSession {
    #[tracing::instrument(skip(msg_from_user))]
    pub fn try_from(msg_from_user: &Message) -> Result<Self, String> {
        let obj = Self {
            task_id: TaskId::new(),
            chat_id: msg_from_user.chat_id().ok_or("Message has no chat_id")?,
            media_type: None,
        };
        Ok(obj)
    }
    #[tracing::instrument(skip(self), fields(task_id = %self.task_id()))]
    pub fn set_media_type(&mut self, media_type: FileType) {
        trace!("...");
        self.media_type = Some(media_type);
    }

    #[tracing::instrument(skip(self, db, bot), fields(task_id = %self.task_id()))]
    pub async fn send_and_remember_msg(
        &self,
        text: &str,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> Result<Vec<TrackedMessage>, Box<dyn Error + Send + Sync>> {
        let text_chunks = split_text(text);
        let mut tracked_messages = Vec::with_capacity(text_chunks.len());

        for (chunk_index, chunk) in text_chunks.into_iter().enumerate() {
            trace!(
                "Sending text message {} of length {} ...",
                chunk_index + 1,
                chunk.len()
            );

            match bot.send_message(self.chat_id, &chunk).await {
                Ok(message) => {
                    let trackedmsg = TrackedMessage::try_from(self.task_id, &message)
                        .map_err(|e| e.to_string())?;

                    trackedmsg.intodb(db.clone()).await?;
                    tracked_messages.push(trackedmsg);
                }
                Err(msg_error) => {
                    warn!("Failed to send message: {}", msg_error);
                }
            }
        }

        Ok(tracked_messages)
    }

    #[tracing::instrument(skip(self, db, bot, keyboard))]
    pub async fn send_and_remember_msg_with_keyboard(
        &self,
        text: &str,
        keyboard: InlineKeyboardMarkup,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> HandlerResult {
        debug!("Sending message with keyboard ...");

        match bot
            .send_message(self.chat_id, text)
            .reply_markup(keyboard.clone())
            .await
        {
            Ok(message) => {
                let trackedmsg =
                    TrackedMessage::try_from(self.task_id, &message).map_err(|e| e.to_string())?;

                trackedmsg.intodb(db.clone()).await?;
                Ok(())
            }
            Err(msg_error) => {
                warn!("Failed to send message: {}", msg_error);
                Err(msg_error.into())
            }
        }
    }
    #[tracing::instrument(skip_all)]
    pub async fn remember_related_message(
        &self,
        msg: &Message,
        db: Surreal<DbClient>,
    ) -> Result<TrackedMessage, Box<dyn Error + Send + Sync>> {
        let trackedmsg = TrackedMessage::try_from(self.task_id, msg).map_err(|e| e.to_string())?;

        trackedmsg.intodb(db.clone()).await?;
        Ok(trackedmsg)
    }

    #[tracing::instrument(skip(self, bot, db), fields(task_id = %self.task_id()))]
    async fn delete_messages_by_task_id(&self, bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
        let tracked_messages =
            TrackedMessage::from_db_by_task_id(self.task_id(), db.clone()).await?;

        for tracked_message in tracked_messages {
            trace!("Deleting message {}...", tracked_message.message_id);
            if let Err(e) = bot
                .delete_message(tracked_message.chat_id, tracked_message.message_id)
                .await
            {
                error!("Failed to delete message: {}", e);
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip_all, fields(task_id = %self.task_id()))]
    pub async fn process_request(
        &self,
        url: String,
        filetype: FileType,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> HandlerResult {
        debug!("Processing request ...");
        let tracked_messages = self
            .send_and_remember_msg("Downloading... Please wait.", bot.clone(), db.clone())
            .await?;

        let last_message = tracked_messages[0].clone();

        let (tx, rx) = watch::channel(false);

        let poller_handle = last_message
            .directory_size_poller_and_mesage_updater(rx, filetype, bot.clone())
            .await?;

        let downloads_result = match &filetype {
            FileType::Mp3 => {
                tokio::task::spawn_blocking(move || {
                    pirate::mp3(url, last_message.task_id().to_string())
                })
                .await
            }
            FileType::Mp4 => {
                tokio::task::spawn_blocking(move || {
                    pirate::mp4(url, last_message.task_id().to_string())
                })
                .await
            }
            FileType::Voice => {
                tokio::task::spawn_blocking(move || {
                    pirate::ogg(url, last_message.task_id().to_string())
                })
                .await
            }
        }?;

        match downloads_result {
            Err(error) => {
                warn!("Download failed: {}", error);
                self.send_and_remember_msg(&error.to_string(), bot.clone(), db)
                    .await?;
                Err(error)
            }
            Ok(files) => {
                trace!(
                    "All files ready. Stopping poller task for Chat ID {} ...",
                    self.chat_id
                );

                let _ = tx.send(true);
                poller_handle.await?;

                for path in files.paths.iter() {
                    if let Err(e) = self
                        .send_file(path, &filetype, bot.clone(), db.clone())
                        .await
                    {
                        warn!("Failed to send file: {}", e);
                    }
                }

                self.delete_messages_by_task_id(bot.clone(), db.clone())
                    .await?;
                cleanup(files.folder);
                info!("Success!");
                Ok(())
            }
        }
    }
    #[tracing::instrument(skip_all, fields(task_id = %self.task_id()))]
    async fn send_file(
        &self,
        path: &PathBuf,
        filetype: &FileType,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> HandlerResult {
        let file = InputFile::file(path);
        let filename_display = path.display().to_string();
        let max_retries = 10;

        for attempt in 1..=max_retries {
            let result = match filetype {
                FileType::Mp3 => bot.send_audio(self.chat_id, file.clone()).await,
                FileType::Mp4 => bot.send_video(self.chat_id, file.clone()).await,
                FileType::Voice => bot.send_voice(self.chat_id, file.clone()).await,
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
                }
            }
        }

        Err(format!("Failed to send file after {max_retries} attempts: {filename_display}").into())
    }
}
