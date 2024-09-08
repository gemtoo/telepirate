use crate::CRATE_NAME;
use std::error::Error;
use serde::{Serialize, Deserialize};
use surrealdb::{engine::local::Db, engine::local::Mem, Surreal};
use teloxide::types::{ChatId, MessageId};

// Static lifetime is OK because the DB should live as long as the program.
pub async fn initialize() -> &'static Surreal<Db> {
    debug!("Initializing database ...");
    let db_result = Surreal::new::<Mem>(()).await;
    match db_result {
        Ok(db) => {
            info!("Database is ready.");
            db.use_ns(CRATE_NAME).use_db(CRATE_NAME).await.unwrap();
            let boxed_db = Box::new(db);
            return Box::leak(boxed_db);
        }
        Err(e) => {
            error!("Database error: {}", e);
            std::process::exit(1);
        }
    }
}

use crate::bot::TelepirateRequest;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    request_id: String,
    msg_id: MessageId,
}

impl Record {
    fn from_telepirate_request(telepirate_request: &TelepirateRequest) -> Self {
        return Record {
            request_id: telepirate_request.request_id(),
            msg_id: telepirate_request.msg_id(),
        };
    }
}

pub async fn intodb(telepirate_request: &TelepirateRequest) -> Result<(), Box<dyn Error + Send + Sync>> {
    let db = telepirate_request.db();
    trace!(
        "Recording Message ID {} from Chat ID {} into DB ...",
        telepirate_request.msg_id().0,
        telepirate_request.chat_id().0
    );
    let record = Record::from_telepirate_request(&telepirate_request);
    let database_name = generate_database_name_from_chat(telepirate_request.chat_id()); 
    let _: Vec<Record> = db.create(database_name).content(record).await?;
    Ok(())
}

pub async fn get_trash_messages_of_current_request_id(telepirate_request: &TelepirateRequest) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
    let db = telepirate_request.db();
    let database_name = generate_database_name_from_chat(telepirate_request.chat_id());
    let request_id = telepirate_request.request_id();
    trace!("Selecting Message IDs of a Request ID {} from the database ...", &request_id);
    let sql = &format!("SELECT msg_id.message_id FROM {} WHERE request_id == {};", database_name, request_id);
    // TODO replace unwrap with ?
    let mut query_response = db.query(sql).await.unwrap();
    let message_ids = query_response.take::<Vec<MessageId>>(0)?;
    Ok(message_ids)
}

//pub async fn get_trash_messages_all()

// remove trash messages for current session id and for total /c are separate funcs
pub async fn get_trash_message_ids(
    chat_id: ChatId,
    db: &Surreal<Db>,
) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
    let database_name = generate_database_name_from_chat(chat_id);
    let message_ids: Vec<MessageId> = db.select(database_name).await?;
    Ok(message_ids)
}

pub async fn forget_deleted_messages(
    chat_id: ChatId,
    db: &Surreal<Db>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    trace!("Forgetting deleted messages for Chat ID {} ...", chat_id.0);
    let database_name = generate_database_name_from_chat(chat_id);
    let _: Vec<MessageId> = db.delete(database_name).await?;
    Ok(())
}

pub async fn get_last_message_id(
    chat_id: ChatId,
    db: &Surreal<Db>,
) -> Result<MessageId, Box<dyn Error + Send + Sync>> {
    let database_name = generate_database_name_from_chat(chat_id);
    let sql = &format!("math::max(SELECT VALUE message_id FROM {});", database_name);
    trace!(
        "Retrieving last Message ID of Chat ID {} from the DB ...",
        chat_id.0
    );
    let mut query_response = db.query(sql).await?;
    let last_message_id = query_response.take::<Option<i32>>(0)?.unwrap();
    Ok(MessageId(last_message_id))
}

fn generate_database_name_from_chat(chat_id: ChatId) -> String {
    return format!("chat_{}", chat_id.0.to_string());
}
