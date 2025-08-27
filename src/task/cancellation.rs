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
    #[tracing::instrument(skip_all)]
    pub fn new() -> Self {
        trace!("Initializing cancellation registry ...");
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }
    #[tracing::instrument(skip(self, token))]
    pub fn register_task(&self, task_id: TaskId, token: CancellationToken) {
        trace!("Registering a new task ...");
        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(task_id, token);
    }
    #[tracing::instrument(skip(self))]
    pub fn cancel_task(&self, task_id: TaskId) -> bool {
        trace!("Cancelling an existing task ...");
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
    #[tracing::instrument(skip(self))]
    pub fn get_token(&self, task_id: TaskId) -> Option<CancellationToken> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).cloned()
    }
    #[tracing::instrument(skip(self))]
    pub fn remove_task(&self, task_id: TaskId) {
        trace!("Deregistering finished task ...");
        let mut tasks = self.tasks.lock().unwrap();
        tasks.remove(&task_id);
    }
}