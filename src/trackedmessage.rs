use std::error::Error;
use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use tokio::sync::watch;
use tracing::{Instrument, debug, trace, warn};

use crate::{
    database::DbRecord,
    misc::{FolderData, sleep},
    task::mediatype::MediaType,
    task::{
        id::TaskId,
        traits::{HasChatId, HasTaskId},
    },
};

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
        filetype: MediaType,
        bot: Bot,
    ) -> Result<tokio::task::JoinHandle<()>, Box<dyn Error + Send + Sync>> {
        debug!("Starting poller task ...");

        let path_to_downloads =
            crate::task::download::construct_destination_path(self.task_id().to_string());
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
