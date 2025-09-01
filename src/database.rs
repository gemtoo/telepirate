use std::error::Error;
use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_type_name::type_name;
use surrealdb::{
    Surreal,
    engine::remote::ws::{Client as DbClient, Ws},
    opt::auth::Root,
};

use crate::CRATE_NAME;
use crate::misc::die;
use crate::task::traits::{HasChatId, HasTaskId};

pub trait DbRecord: Clone + Debug /*+ Display*/ + Serialize + DeserializeOwned + HasTaskId + HasChatId where Self: 'static {
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn intodb(&self, db: Surreal<DbClient>) -> Result<Option<Self>, Box<dyn Error + Send + Sync>> {
        // Derive table name from type
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // Create record in the type-specific table
        let object_option: Option<Self> = db.create(&table_name).content(self.clone()).await?;
        Ok(object_option)
    }

    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn from_db(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // Manual query formatting required because SurrealDB's .bind() method
        // would wrap the type name in quotes, making it invalid as a table identifier
        let query_base = format!("SELECT * FROM {table_name}");

        // Execute parameterized query
        let object_array: Vec<Self> = db.query(&query_base).await?.take(0)?;
        Ok(object_array)
    }

    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn select_by_task_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // Manual query formatting required because SurrealDB's .bind() method
        // would wrap the type name in quotes, making it invalid as a table identifier
        let query_base = format!("SELECT * FROM {table_name} WHERE task_id = $task_id_object");

        // Execute parameterized query
        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("task_id_object", self.task_id())).await?.take(0)?;
        Ok(object_array)
    }

    #[tracing::instrument(skip(self, db), fields(chat_id = %self.chat_id()))]
    async fn select_by_chat_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // See note in select_by_task_id about manual query formatting
        let query_base = format!("SELECT * FROM {table_name} WHERE chat_id = $chat_id_object");

        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("chat_id_object", self.chat_id())).await?.take(0)?;
        Ok(object_array)
    }

    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn delete_by_task_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // Manual DELETE query with task_id parameter
        let query_base = format!("DELETE FROM {table_name} WHERE task_id = $task_id_object");

        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("task_id_object", self.task_id())).await?.take(0)?;
        Ok(object_array)
    }
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn update_by_task_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        let table_name = table_name(type_name);
        // Manual DELETE query with task_id parameter
        let query_base = format!("UPDATE {table_name} CONTENT $self_object WHERE task_id = $task_id_object");

        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("self_object", self.clone()))
             .bind(("task_id_object", self.task_id()))
             .await?.take(0)?;
        Ok(object_array)
    }
}

#[tracing::instrument]
pub async fn db_init() -> Surreal<DbClient> {
    debug!("Initializing database connection...");

    // Establish WebSocket connection to SurrealDB
    let db = Surreal::new::<Ws>("surrealdb:8000")
        .await
        .unwrap_or_else(|e| die(e.to_string()));

    info!("Database connection established.");

    // Authenticate as root user
    db.signin(Root {
        username: "root",
        password: "root",
    })
    .await
    .unwrap_or_else(|e| die(e.to_string()));

    // Select namespace and database (uses crate name)
    db.use_ns(CRATE_NAME)
        .use_db(CRATE_NAME)
        .await
        .unwrap_or_else(|e| die(e.to_string()));

    return db;
}

// Append -dev to table name to not mix prod and dev if using the same DB instance.
pub fn table_name(type_name: &str) -> String {
    if cfg!(debug_assertions) {
        return format!("{}_dev", type_name);
    } else {
        return format!("{}", type_name);
    };
}
