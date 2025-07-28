use super::download::*;
use super::id::TaskId;
use super::mediatype::*;
use super::traits::*;
use serde::{Deserialize, Serialize};
use std::error::Error;
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSimple {
    pub task_id: TaskId,
    pub chat_id: ChatId,
}

impl HasTaskId for TaskSimple {
    fn task_id(&self) -> TaskId {
        self.task_id
    }
}
impl HasChatId for TaskSimple {
    fn chat_id(&self) -> ChatId {
        self.chat_id
    }
}
impl Task for TaskSimple {}
impl TaskSimple {
    pub fn try_from(msg_from_user: &Message) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let obj = Self {
            task_id: TaskId::new(),
            chat_id: msg_from_user.chat_id().ok_or("Message has no chat_id")?,
        };
        Ok(obj)
    }
    pub fn to_task_download(&self, media_type: MediaType) -> TaskDownload {
        TaskDownload {
            task_id: self.task_id(),
            chat_id: self.chat_id(),
            media_type,
            url: None,
        }
    }
}
