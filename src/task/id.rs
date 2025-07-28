use std::fmt;
use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TaskId {
    pub uuid: Uuid,
}

impl TaskId {
    pub fn new() -> Self {
        TaskId {
            uuid: Uuid::new_v4(),
        }
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.uuid)
    }
}
