use std::collections::HashMap;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

// Global task registry
lazy_static::lazy_static! {
    pub static ref TASK_REGISTRY: CancellationRegistry = CancellationRegistry::new();
}
use crate::task::download::construct_destination_path;
use crate::misc::cleanup;
use crate::task::id::TaskId;

// Global registry to track currently Running tasks and their cancellation tokens.
// Because cancellation tokens can't be stored in a DB and these are runtime only variables that don't need persistence.
pub struct CancellationRegistry {
    tasks: Mutex<HashMap<TaskId, CancellationToken>>,
}

impl CancellationRegistry {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_task(&self, task_id: TaskId, token: CancellationToken) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(task_id, token);
    }

    pub fn cancel_task(&self, task_id: TaskId) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(token) = tasks.remove(&task_id) {
            token.cancel();
            let downloads_path = construct_destination_path(task_id.to_string());
            cleanup(downloads_path.into());
            true
        } else {
            false
        }
    }

    pub fn get_token(&self, task_id: TaskId) -> Option<CancellationToken> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).cloned()
    }

    pub fn remove_task(&self, task_id: TaskId) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.remove(&task_id);
    }
}