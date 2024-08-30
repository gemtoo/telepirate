use crate::CRATE_NAME;
use std::error::Error;
use surrealdb::{engine::local::Db, engine::local::Mem, Surreal};
use teloxide::types::{ChatId, MessageId};

pub async fn initialize() -> Surreal<Db> {
    debug!("Initializing database ...");
    let db_result = Surreal::new::<Mem>(()).await;
    match db_result {
        Ok(db) => {
            info!("Database is ready.");
            db.use_ns(CRATE_NAME).use_db(CRATE_NAME).await.unwrap();
            return db;
        }
        Err(e) => {
            error!("Database error: {}", e);
            std::process::exit(1);
        }
    }
}

pub async fn intodb(
    chat_id: ChatId,
    msg_id: MessageId,
    db: &Surreal<Db>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let chat: String = chat_id.0.to_string();
    trace!(
        "Recording message ID {} from Chat {} into DB ...",
        msg_id.0,
        &chat
    );
    let _: Vec<MessageId> = db.create(chat).content(msg_id).await?;
    Ok(())
}

pub async fn get_trash_message_ids(
    chat_id: ChatId,
    db: &Surreal<Db>,
) -> Result<Vec<MessageId>, Box<dyn Error + Send + Sync>> {
    let message_ids: Vec<MessageId> = db.select(chat_id.0.to_string()).await?;
    Ok(message_ids)
}

pub async fn forget_deleted_messages(
    chat_id: ChatId,
    db: &Surreal<Db>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    trace!("Forgetting deleted messages for Chat {} ...", chat_id.0);
    let _: Vec<MessageId> = db.delete(chat_id.to_string()).await?;
    Ok(())
}
