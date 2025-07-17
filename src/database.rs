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
use crate::task::HasChatId;
use crate::task::HasTaskId;

/// Defines database operations for records that contain task and chat identifiers
///
/// This trait provides CRUD operations for SurrealDB records. It automatically handles:
/// - Table name derivation using the type name
/// - Query construction with task_id and chat_id parameters
/// - Tracing instrumentation for all operations
///
/// Requirements for implementors:
/// - Must be serializable/deserializable (Serde)
/// - Must contain task_id and chat_id fields (via HasTaskId/HasChatId)
/// - Must be thread-safe ('static lifetime)
pub trait DbRecord: Clone + Debug /*+ Display*/ + Serialize + DeserializeOwned + HasTaskId + HasChatId where Self: 'static {
    /// Inserts the current record into the database
    ///
    /// Uses the type name as the database table. Returns the created record
    /// with any database-generated fields.
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn intodb(&self, db: Surreal<DbClient>) -> Result<Option<Self>, Box<dyn Error + Send + Sync>> {
        // Derive table name from type
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);
        // Create record in the type-specific table
        let object_option: Option<Self> = db.create(type_name).content(self.clone()).await?;
        Ok(object_option)
    }

    /// Retrieves all records matching the current record's task_id
    ///
    /// Constructs a parameterized query using the type name as table
    /// and task_id as filter parameter.
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn select_by_task_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);

        // Manual query formatting required because SurrealDB's .bind() method
        // would wrap the type name in quotes, making it invalid as a table identifier
        let query_base = format!("SELECT * FROM {} WHERE task_id = $task_id_object", type_name);

        // Execute parameterized query
        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("task_id_object", self.task_id())).await?.take(0)?;
        Ok(object_array)
    }

    /// Retrieves all records matching the current record's chat_id
    ///
    /// Uses the same query construction approach as select_by_task_id
    /// but filters by chat_id instead.
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn select_by_chat_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);

        // See note in select_by_task_id about manual query formatting
        let query_base = format!("SELECT * FROM {} WHERE chat_id = $chat_id_object", type_name);

        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("chat_id_object", self.chat_id())).await?.take(0)?;
        Ok(object_array)
    }

    /// Deletes all records matching the current record's task_id
    ///
    /// Returns the deleted records for confirmation.
    #[tracing::instrument(skip(self, db), fields(task_id = %self.task_id()))]
    async fn delete_by_task_id(&self, db: Surreal<DbClient>) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
        let type_name = type_name(self)?;
        trace!("{} ...", type_name);

        // Manual DELETE query with task_id parameter
        let query_base = format!("DELETE FROM {} WHERE task_id = $task_id_object", type_name);

        let object_array: Vec<Self> = db.query(&query_base)
             .bind(("task_id_object", self.task_id())).await?.take(0)?;
        Ok(object_array)
    }
}

/// Initializes and configures the SurrealDB database connection
///
/// Performs:
/// 1. TCP connection to SurrealDB server
/// 2. Root authentication
/// 3. Namespace and database selection
///
/// Terminates application on any connection failure.
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
