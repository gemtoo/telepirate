use super::download::*;
use super::id::*;
use super::mediatype::*;
use super::simple::*;
use super::stats::*;
use super::traits::*;
use crate::database::*;
use crate::misc::die;
use serde::{Deserialize, Serialize};
use serde_type_name::type_name;
use tokio_util::sync::CancellationToken;
use std::error::Error;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::prelude::*;
use url::Url;

use super::cancellation::TASK_REGISTRY;

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
        let query_base = format!(
            "UPDATE {table_name} CONTENT $self_object WHERE data.task_id = $task_id_object"
        );

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
    // pub async fn from_db_all(
    //     db: Surreal<DbClient>,
    // ) -> Result<Vec<Self>, Box<dyn Error + Send + Sync>> {
    //     let dummy_task_simple = TaskSimple {
    //         task_id: TaskId::new(),
    //         chat_id: ChatId(0),
    //     };
    //     let dummy_task_state = Self::New(dummy_task_simple);
    //     return dummy_task_state.fromdb(db).await;
    // }
    pub async fn to_waiting_for_url(&mut self, media_type: MediaType, db: Surreal<DbClient>) {
        if let TaskState::New(task_simple) = self {
            let new_state = TaskState::WaitingForUrl(task_simple.to_task_download(media_type));
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state
        } else {
            die("Only TaskState::New can use to_waiting_for_url method.");
        }
    }

    pub async fn to_running(&mut self, url: Url, db: Surreal<DbClient>, cancellation_token: CancellationToken) {
        if let TaskState::WaitingForUrl(task_download) = self {
            task_download.set_url(url);
            let new_state = TaskState::Running(task_download.clone());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
            // Register task in the CancellationRegistry
            TASK_REGISTRY.register_task(self.task_id(), cancellation_token);
        } else {
            die("Only TaskState::WaitingForUrl can use to_running method.");
        }
    }

    pub async fn to_success(&mut self, db: Surreal<DbClient>) {
        if let TaskState::Running(task_download) = self {
            let new_state = TaskState::Success(task_download.to_task_stats());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
            TASK_REGISTRY.remove_task(self.task_id());
        } else {
            die("Only TaskState::Running can use to_success method.");
        }
    }

    pub async fn to_failure(&mut self, db: Surreal<DbClient>) {
        if let TaskState::Running(task_download) = self {
            let new_state = TaskState::Failure(task_download.to_task_stats());
            new_state.update_by_task_id(db).await.unwrap();
            *self = new_state;
            TASK_REGISTRY.remove_task(self.task_id());
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

    // pub fn get_inner_task_stats(&self) -> Option<&TaskStats> {
    //     match self {
    //         Self::Success(v) | Self::Failure(v) => Some(v),
    //         _ => None,
    //     }
    // }
}
