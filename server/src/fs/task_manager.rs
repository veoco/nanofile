use std::collections::HashMap;
use std::sync::RwLock;

/// Current state of a copy/move task.
#[derive(Clone, Debug, PartialEq)]
pub enum TaskState {
    Pending,
    Processing,
    Completed,
    Failed(String),
}

/// A copy or move task tracked by the TaskManager.
#[derive(Clone, Debug)]
pub struct CopyMoveTask {
    pub task_id: String,
    pub state: TaskState,
    pub operation: String,
    pub total: usize,
    pub done_count: usize,
    pub created_at: i64,
    pub description: String,
}

/// In-memory task manager for async copy/move operations.
///
/// Tasks auto-expire after `CLEANUP_TTL_SECS` to prevent memory leaks.
pub struct TaskManager {
    tasks: RwLock<HashMap<String, CopyMoveTask>>,
}

const CLEANUP_TTL_SECS: i64 = 3600;

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    /// Create a new empty TaskManager.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new task in Pending state. Returns the task_id.
    pub fn create_task(
        &self,
        task_id: String,
        operation: &str,
        total: usize,
        description: &str,
    ) -> String {
        self.cleanup_expired();
        let now = chrono::Utc::now().timestamp();
        let task = CopyMoveTask {
            task_id: task_id.clone(),
            state: TaskState::Pending,
            operation: operation.to_string(),
            total,
            done_count: 0,
            created_at: now,
            description: description.to_string(),
        };
        if let Ok(mut tasks) = self.tasks.write() {
            tasks.insert(task_id.clone(), task);
        }
        task_id
    }

    /// Retrieve a task by its ID.
    pub fn get_task(&self, task_id: &str) -> Option<CopyMoveTask> {
        if let Ok(tasks) = self.tasks.read() {
            tasks.get(task_id).cloned()
        } else {
            None
        }
    }

    /// Transition a task from Pending to Processing.
    pub fn start_processing(&self, task_id: &str) {
        self.update_state(task_id, |t| {
            if t.state == TaskState::Pending {
                t.state = TaskState::Processing;
            }
        });
    }

    /// Update the done_count for progress tracking.
    pub fn update_progress(&self, task_id: &str, done_count: usize) {
        self.update_state(task_id, |t| {
            t.done_count = done_count;
        });
    }

    /// Transition a task from Processing to Completed.
    pub fn complete_task(&self, task_id: &str) {
        self.update_state(task_id, |t| {
            t.state = TaskState::Completed;
            t.done_count = t.total;
        });
    }

    /// Transition a task from Processing to Failed with an error message.
    pub fn fail_task(&self, task_id: &str, error: String) {
        self.update_state(task_id, |t| {
            t.state = TaskState::Failed(error);
        });
    }

    /// Remove expired completed/failed tasks.
    fn cleanup_expired(&self) {
        let now = chrono::Utc::now().timestamp();
        if let Ok(mut tasks) = self.tasks.write() {
            tasks.retain(|_, t| {
                if matches!(t.state, TaskState::Completed | TaskState::Failed(_)) {
                    now - t.created_at < CLEANUP_TTL_SECS
                } else {
                    // Keep pending/processing tasks
                    true
                }
            });
        }
    }

    fn update_state(&self, task_id: &str, f: impl FnOnce(&mut CopyMoveTask)) {
        if let Ok(mut tasks) = self.tasks.write()
            && let Some(task) = tasks.get_mut(task_id)
        {
            f(task);
        }
    }
}
