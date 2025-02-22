use crate::bot::getuser;
use crate::misc::die;
use crate::CRATE_NAME;
use serde::{Deserialize, Serialize};
use std::error::Error;
use surrealdb::{
    engine::remote::ws::{Client as DbClient, Ws},
    Surreal,
    opt::auth::Root,
};
use teloxide::types::{ChatId, Message, MessageId};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestId(String);
impl RequestId {
    pub fn new() -> Self {
        RequestId(Uuid::new_v4().to_string())
    }
}
use std::fmt;
impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
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
    pub fn from(message: Message, request_id: RequestId) -> Self {
        TelepirateDbRecord {
            chat_id: message.chat.id,
            message_id: message.id,
            username: getuser(&message),
            request_id,
        }
    }
    pub async fn intodb(&self, db: &Surreal<DbClient>) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(
            "Recording Request ID {}, Message ID {}, Chat ID {} into DB ...",
            self.request_id,
            self.message_id,
            self.chat_id
        );
        let _: Option<TelepirateDbRecord> = db.create(CRATE_NAME).content(self.clone()).await?;
        Ok(())
    }
    pub async fn msg_ids_fromdb_by_request_id(
        &self,
        db: &Surreal<DbClient>,
    ) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
        trace!(
            "Retrieving all messages with Request ID {} and Chat ID {} from DB ...",
            self.request_id,
            self.chat_id
        );
        let sql = format!(
            "SELECT VALUE message_id FROM {} WHERE request_id = s'{}';",
            CRATE_NAME, self.request_id
        );
        let mut query_response = db.query(sql).await?;
        let messages_with_request_id = query_response.take::<Vec<MessageId>>(0)?;
        Ok(messages_with_request_id)
    }
    pub async fn msg_ids_fromdb_by_chat_id(
        &self,
        db: &Surreal<DbClient>,
    ) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
        trace!(
            "Retrieving all messages of Chat ID {} from DB ...",
            self.chat_id
        );
        let sql = format!(
            "SELECT VALUE message_id FROM {} WHERE chat_id = {};",
            CRATE_NAME, self.chat_id
        );
        let mut query_response = db.query(sql).await?;
        let all_messages_from_chat = query_response.take::<Vec<MessageId>>(0)?;
        Ok(all_messages_from_chat)
    }
    pub async fn msg_id_fromdb_last(
        &self,
        db: &Surreal<DbClient>,
    ) -> Result<Option<MessageId>, Box<dyn Error + Send + Sync>> {
        trace!("Retrieving last message of Chat ID {} ...", self.chat_id);
        let sql = format!(
            "math::max(SELECT VALUE message_id.message_id FROM {} WHERE chat_id = {});",
            CRATE_NAME, self.chat_id
        );
        let mut query_response = db.query(sql).await?;
        let last_message_from_chat = query_response.take::<Option<i32>>(0)?.map(MessageId);
        Ok(last_message_from_chat)
    }
    pub async fn fromdb_delete_all_by_chat_id(
        &self,
        db: &Surreal<DbClient>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(
            "Deleting all records related to Chat ID {} from the DB ...",
            self.chat_id
        );
        let sql = format!("DELETE {} WHERE chat_id = {};", CRATE_NAME, self.chat_id);
        let mut query_response = db.query(sql).await?;
        let _ = query_response.take::<Vec<Self>>(0)?;
        Ok(())
    }
    pub async fn fromdb_delete_all_by_request_id(
        &self,
        db: &Surreal<DbClient>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(
            "Deleting all records related to Request ID {} from the DB ...",
            self.request_id
        );
        let sql = format!(
            "DELETE {} WHERE request_id = s'{}';",
            CRATE_NAME, self.request_id
        );
        let mut query_response = db.query(sql).await?;
        let _ = query_response.take::<Vec<Self>>(0)?;
        Ok(())
    }
}

pub async fn initialize() -> &'static Surreal<DbClient> {
    debug!("Initializing database ...");
    let db_result = Surreal::new::<Ws>("surrealdb:8000").await;
    match db_result {
        Ok(db) => {
            info!("Database is ready.");
            db.signin( Root {
                username: "root",
                password: "root",
            }).await.unwrap();
            db.use_ns(CRATE_NAME).use_db(CRATE_NAME).await.unwrap();
            let boxed_db = Box::new(db);
            return Box::leak(boxed_db);
        }
        Err(e) => {
            die(e.to_string());
        }
    }
}
