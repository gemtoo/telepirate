use super::id::TaskId;
use super::mediatype::*;
use super::traits::*;
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    pub task_id: TaskId,
    pub chat_id: ChatId,
    pub media_type: MediaType,
    pub url: Url,
    //started_at: Utc,
    //finished_at: Utc,
    //downloaded_size;
}
impl HasTaskId for TaskStats {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskStats {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}
impl Task for TaskStats {}
