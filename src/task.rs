use std::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_type_name::type_name;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile};
use tokio::sync::watch;
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

use crate::trackedmessage::TrackedMessage;
use crate::{
    database::{DbRecord, table_name},
    misc::*,
    pirate::{self, MediaType},
};

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

pub trait Task: Clone + Debug + HasTaskId + HasChatId
where
    Self: 'static,
{
    #[tracing::instrument(skip(self, db, bot), fields(task_id = %self.task_id()))]
    async fn send_and_remember_msg(
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

            match bot.send_message(self.chat_id(), &chunk).await {
                Ok(message) => {
                    let trackedmsg = TrackedMessage::try_from(self.task_id(), &message)
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
    async fn send_and_remember_msg_with_keyboard(
        &self,
        text: &str,
        keyboard: InlineKeyboardMarkup,
        bot: Bot,
        db: Surreal<DbClient>,
    ) -> HandlerResult {
        debug!("Sending message with keyboard ...");

        match bot
            .send_message(self.chat_id(), text)
            .reply_markup(keyboard.clone())
            .await
        {
            Ok(message) => {
                let trackedmsg = TrackedMessage::try_from(self.task_id(), &message)
                    .map_err(|e| e.to_string())?;

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
    async fn remember_related_message(
        &self,
        msg: &Message,
        db: Surreal<DbClient>,
    ) -> Result<TrackedMessage, Box<dyn Error + Send + Sync>> {
        let trackedmsg =
            TrackedMessage::try_from(self.task_id(), msg).map_err(|e| e.to_string())?;

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
                .delete_message(tracked_message.chat_id(), tracked_message.message_id)
                .await
            {
                error!("Failed to delete message: {}", e);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSimple {
    task_id: TaskId,
    chat_id: ChatId,
}

impl HasTaskId for TaskSimple {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskSimple {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}
impl Task for TaskSimple {}
impl TaskSimple {
    pub fn try_from(msg_from_user: &Message) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let obj = Self {
            task_id: TaskId::new(),
            chat_id: msg_from_user.chat_id().ok_or("Message has no chat_id")?,
        };
        Ok(obj)
    }
    fn to_task_download(&self, media_type: MediaType) -> TaskDownload {
        TaskDownload {
            task_id: self.task_id(),
            chat_id: self.chat_id(),
            media_type,
            url: None,
        }
    }
}

use url::Url;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDownload {
    task_id: TaskId,
    chat_id: ChatId,
    media_type: MediaType,
    // Option because at the intermediate stage WaitingForUrl it is known that the task is Download but initial URL is None.
    url: Option<Url>,
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
    fn set_url(&mut self, url: Url) {
        self.url = Some(url);
    }
    fn media_type(&self) -> MediaType {
        self.media_type
    }
    fn to_task_stats(&self) -> TaskStats {
        TaskStats {
            task_id: self.task_id(),
            chat_id: self.chat_id(),
            media_type: self.media_type(),
            // This unwrap is safe because TaskState::Running is not possible without URL.
            url: self.url().unwrap(),
        }
    }
    #[tracing::instrument(skip_all, fields(task_id = %self.task_id()))]
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
        let url_string = self.url().unwrap().to_string();
        let downloads_result = match self.media_type() {
            MediaType::Mp3 => {
                tokio::task::spawn_blocking(move || {
                    pirate::mp3(url_string, last_message.task_id().to_string())
                })
                .await
            }
            MediaType::Mp4 => {
                tokio::task::spawn_blocking(move || {
                    pirate::mp4(url_string, last_message.task_id().to_string())
                })
                .await
            }
            MediaType::Voice => {
                tokio::task::spawn_blocking(move || {
                    pirate::ogg(url_string, last_message.task_id().to_string())
                })
                .await
            }
        }?;

        match downloads_result {
            Err(error) => {
                warn!("Download failed: {}", error);
                let _ = tx.send(true);
                poller_handle.await?;
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
                    if let Err(e) = self.send_file(path, bot.clone(), db.clone()).await {
                        warn!("Failed to send file: {}", e);
                    }
                }

                self.delete_messages_by_task_id(bot.clone(), db.clone())
                    .await?;
                // Cleanup is done to save space on disk and to remove pirating evidence
                cleanup(files.folder);
                info!("Success!");
                Ok(())
            }
        }
    }
    #[tracing::instrument(skip_all, fields(task_id = %self.task_id()))]
    async fn send_file(&self, path: &PathBuf, bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
        let file = InputFile::file(path);
        let filename_display = path.display().to_string();
        let max_retries = 10;

        for attempt in 1..=max_retries {
            let result = match self.media_type() {
                MediaType::Mp3 => bot.send_audio(self.chat_id(), file.clone()).await,
                MediaType::Mp4 => bot.send_video(self.chat_id(), file.clone()).await,
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
                }
            }
        }

        Err(format!("Failed to send file after {max_retries} attempts: {filename_display}").into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    task_id: TaskId,
    chat_id: ChatId,
    media_type: MediaType,
    url: Url,
    //started_at: Utc,
    //finished_at: Utc,
    //downloaded_size;
}
impl HasTaskId for TaskStats {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskStats {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}
impl Task for TaskStats {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", content = "data")]
pub enum TaskState {
    New(TaskSimple),
    // Waitingforurl can't have no media type in reality but it program's logic it can, so I have to redecide what to put here but that's for later
    WaitingForUrl(TaskDownload),
    Running(TaskDownload),
    Success(TaskStats),
    Failure(TaskStats),
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
        let table_name = table_name(type_name);
        // query_base because type_name can't be in .bind() because .bind() adds single brackets '' thus searching in the wrong table
        // the only thing that's changed from the default trait function is that data.chat_id is used instead of simply chat_id
        let query_base = format!("SELECT * FROM {table_name} WHERE data.chat_id = $chat_id_object");
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
        let table_name = table_name(type_name);
        // query_base because type_name can't be in .bind() because .bind() adds single brackets '' thus searching in the wrong table
        // the only thing that's changed from the default trait function is that data.task_id is used instead of simply task_id
        let query_base = format!("DELETE FROM {table_name} WHERE data.task_id = $task_id_object");
        let object_array: Vec<Self> = db
            .query(&query_base)
            .bind(("task_id_object", self.task_id()))
            .await?
            .take(0)?;
        Ok(object_array)
    }
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn update_by_task_id(
        &self,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // query_base because type_name can't be in .bind() because .bind() adds single brackets '' thus searching in the wrong table
        // the only thing that's changed from the default trait function is that data.task_id is used instead of simply task_id
        let query_base =
            format!("UPDATE {table_name} CONTENT $self_object WHERE data.task_id = $task_id_object");

        let object_array: Vec<Self> = db
            .query(&query_base)
            .bind(("self_object", self.clone()))
            .bind(("task_id_object", self.task_id()))
            .await?
            .take(0)?;
        Ok(object_array)
    }
}
impl HasTaskId for TaskState {
    fn task_id(&self) -> TaskId {
        match self {
            TaskState::New(task_simple) => task_simple.task_id(),
            TaskState::WaitingForUrl(task_simple) => task_simple.task_id(),
            TaskState::Running(task_download) => task_download.task_id(),
            TaskState::Success(task_stats) => task_stats.task_id(),
            TaskState::Failure(task_stats) => task_stats.task_id(),
        }
    }
}
impl HasChatId for TaskState {
    fn chat_id(&self) -> ChatId {
        match self {
            TaskState::New(task_simple) => task_simple.chat_id(),
            TaskState::WaitingForUrl(task_simple) => task_simple.chat_id(),
            TaskState::Running(task_download) => task_download.chat_id(),
            TaskState::Success(task_stats) => task_stats.chat_id(),
            TaskState::Failure(task_stats) => task_stats.chat_id(),
        }
    }
}
impl TaskState {
    pub fn try_from(msg_from_user: &Message) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Ok(Self::New(TaskSimple::try_from(msg_from_user)?))
    }
    pub async fn from_db_by_chat_id(
        chat_id: ChatId,
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let dummy_task_simple = TaskSimple {
            task_id: TaskId::new(),
            chat_id,
        };
        let dummy_task_state = Self::New(dummy_task_simple);
        return dummy_task_state.select_by_chat_id(db).await;
    }
    pub async fn from_db_all(
        db: Surreal<DbClient>,
    ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let dummy_task_simple = TaskSimple {
            task_id: TaskId::new(),
            chat_id: ChatId(0),
        };
        let dummy_task_state = Self::New(dummy_task_simple);
        return dummy_task_state.fromdb(db).await;
    }
    pub async fn to_waiting_for_url(&mut self, media_type: MediaType, db: Surreal<DbClient>) {
        if let TaskState::New(task_simple) = self {
            let new_state = TaskState::WaitingForUrl(task_simple.to_task_download(media_type));
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state
        } else {
            die("Only TaskState::New can use to_waiting_for_url method.");
        }
    }

    pub async fn to_running(&mut self, url: Url, db: Surreal<DbClient>) {
        if let TaskState::WaitingForUrl(task_download) = self {
            task_download.set_url(url);
            let new_state = TaskState::Running(task_download.clone());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
        } else {
            die("Only TaskState::WaitingForUrl can use to_running method.");
        }
    }

    pub async fn to_success(&mut self, db: Surreal<DbClient>) {
        if let TaskState::Running(task_download) = self {
            let new_state = TaskState::Success(task_download.to_task_stats());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
        } else {
            die("Only TaskState::Running can use to_success method.");
        }
    }

    pub async fn to_failure(&mut self, db: Surreal<DbClient>) {
        if let TaskState::Running(task_download) = self {
            let new_state = TaskState::Failure(task_download.to_task_stats());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
        } else {
            die("Only TaskState::Running can use to_failure method.");
        }
    }
    pub fn get_inner_task_simple(&self) -> Option<&TaskSimple> {
        if let Self::New(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn get_inner_task_download(&self) -> Option<&TaskDownload> {
        match self {
            Self::WaitingForUrl(v) | Self::Running(v) => Some(v),
            _ => None,
        }
    }

    pub fn get_inner_task_stats(&self) -> Option<&TaskStats> {
        match self {
            Self::Success(v) | Self::Failure(v) => Some(v),
            _ => None,
        }
    }
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
