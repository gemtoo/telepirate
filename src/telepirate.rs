use teloxide::types::ChatId;
use teloxide::types::MessageId;
use teloxide::types::Message;
use crate::database::RequestId;
use crate::misc::*;
use teloxide::prelude::*;
use crate::bot::TelepirateSession;
pub struct TelepirateRequest {
    pub chat_id: ChatId,
    pub message_ids: Vec<MessageId>,
    pub username: String,
    pub request_id: RequestId,
}

// Shits gonna be a problem because now we have to work with vectors in db rather than oneshot records
impl TelepirateRequest {
    pub fn from(message: Message) -> Self {
        TelepirateRequest {
            chat_id: message.chat.id,
            message_ids: vec![message.id],
            username: getuser(&message),
            request_id: RequestId::new(),
        }
    }
    pub async fn reply(&mut self, session: &TelepirateSession, text: &str) {
        let text_chunks = split_text(text);
        let mut text_chunk_index: usize = 0;
        trace!("Message chunks to send: {}.", text_chunks.len());
        for text_chunk in text_chunks {
            text_chunk_index += 1;
            trace!(
                "Sending text message {} of length {} ...",
                text_chunk_index,
                text_chunk.len()
            );
            let message_result = session.bot.send_message(self.chat_id, text_chunk).await;
            match message_result {
                Ok(message) => {
                    /*let new_dbrecord = TelepirateDbRecord::from(message, reference.request_id.clone());
                    new_dbrecord.intodb(self.db).await.unwrap_or_else(
                        |warning| warn!("Failed create a DB record: {}", warning)
                    );*/
                }
                Err(msg_error) => {
                    warn!("Failed to send message: {}", msg_error);
                }
            }
        }
    }
}