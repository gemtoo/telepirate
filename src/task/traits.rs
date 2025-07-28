use std::error::Error;
use std::fmt::Debug;

use super::id::TaskId;
use crate::database::*;
use crate::misc::*;
use crate::trackedmessage::*;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::prelude::*;
use teloxide::types::InlineKeyboardMarkup;
use tracing::{debug, error, trace, warn};

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

pub trait HasTaskId {
    fn task_id(&self) -> TaskId;
}
pub trait HasChatId {
    fn chat_id(&self) -> ChatId;
}

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
