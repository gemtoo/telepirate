use crate::misc::die;
use crate::bot::getuser;
use crate::CRATE_NAME;
use std::error::Error;
use serde::{Deserialize, Serialize};
use surrealdb::{
    engine::remote::ws::{Client as DbClient, Ws},
    Surreal,
};
use teloxide::types::{ChatId, Message, MessageId};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RequestId(Uuid);
impl RequestId {
    pub fn new() -> Self {
        RequestId(Uuid::new_v4())
    }
}
use std::fmt;
impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelepirateDbRecord {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub username: String,
    pub request_id: RequestId,
}
impl TelepirateDbRecord {
    pub fn from(message: Message) -> Self {
        TelepirateDbRecord {
            chat_id: message.chat.id,
            message_id: message.id,
            username: getuser(&message),
            request_id: RequestId::new(),
        }
    }
    pub async fn intodb(&self, db: &Surreal<DbClient>) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!("Recording Request ID {}, Message ID {}, Chat ID {} into DB ...", self.request_id, self.message_id, self.chat_id);
        let _: Vec<MessageId> = db.create(CRATE_NAME).content(self).await?;
        Ok(())
    }
    pub async fn fromdb(&self, db: &Surreal<DbClient>) -> Vec<Self> {
        trace!("Retrieving all messages with Request ID {} and Chat ID {} ...", self.request_id, self.chat_id);
        vec![]
    }
}

pub async fn initialize() -> &'static Surreal<DbClient> {
    debug!("Initializing database ...");
    let db_result = Surreal::new::<Ws>("surrealdb:8000").await;
    match db_result {
        Ok(db) => {
            info!("Database is ready.");
            db.use_ns(CRATE_NAME).use_db(CRATE_NAME).await.unwrap();
            let boxed_db = Box::new(db);
            return Box::leak(boxed_db);
        }
        Err(e) => {
            die(e.to_string());
        }
    }
}

pub async fn intodb(
    chat_id: ChatId,
    msg_id: MessageId,
    db: &Surreal<DbClient>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let database_name = generate_database_name_from_chat(chat_id);
    trace!(
        "Recording Message ID {} from Chat ID {} into DB ...",
        msg_id.0,
        chat_id.0
    );
    let _: Vec<MessageId> = db.create(database_name).content(msg_id).await?;
    Ok(())
}

pub async fn get_trash_message_ids(
    chat_id: ChatId,
    db: &Surreal<DbClient>,
) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
    let database_name = generate_database_name_from_chat(chat_id);
    let message_ids: Vec<MessageId> = db.select(database_name).await?;
    Ok(message_ids)
}

pub async fn forget_deleted_messages(
    chat_id: ChatId,
    db: &Surreal<DbClient>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    trace!("Forgetting deleted messages for Chat ID {} ...", chat_id.0);
    let database_name = generate_database_name_from_chat(chat_id);
    let _: Vec<MessageId> = db.delete(database_name).await?;
    Ok(())
}

pub async fn get_last_message_id(
    chat_id: ChatId,
    db: &Surreal<DbClient>,
) -> Result<MessageId, Box<dyn Error + Send + Sync>> {
    let database_name = generate_database_name_from_chat(chat_id);
    let sql = &format!("math::max(SELECT VALUE message_id FROM {});", database_name);
    trace!(
        "Retrieving last Message ID of Chat ID {} from the DB ...",
        chat_id.0
    );
    let mut query_response = db.query(sql).await.unwrap();
    let last_message_id = query_response.take::<Option<i32>>(0)?.unwrap();
    Ok(MessageId(last_message_id))
}

fn generate_database_name_from_chat(chat_id: ChatId) -> String {
    return format!("chat_{}", chat_id.0.to_string());
}
