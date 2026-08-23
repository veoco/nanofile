use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use base::error::AppError;

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
/// Tasks auto-expire after `CLEANUP_TTL_SECS` to prevent memory leaks. The
/// number of active (Pending/Processing) tasks is bounded by `max_active_tasks`
/// — `create_task` returns 429 when the cap is reached — and terminal task
/// retention is bounded by `MAX_RETAINED_TASKS`.
///
/// Residual risk: if a spawned task panics before calling `complete_task`/
/// `fail_task`, it stays active forever and permanently occupies an active
/// slot. Guarding that is out of scope.
pub struct TaskManager {
    tasks: RwLock<HashMap<String, CopyMoveTask>>,
    max_active_tasks: u64,
}

const CLEANUP_TTL_SECS: i64 = 3600;

/// Maximum number of tasks retained in the map (all states). Terminal
/// (Completed/Failed) tasks beyond this are evicted oldest-first on the next
/// `create_task`; active tasks are never evicted.
const MAX_RETAINED_TASKS: usize = 1000;

impl Default for TaskManager {
    fn default() -> Self {
        Self::new(100)
    }
}

impl TaskManager {
    /// Create a new TaskManager bounding the number of simultaneously active
    /// (Pending/Processing) tasks to `max_active_tasks` (0 = unlimited).
    pub fn new(max_active_tasks: u64) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            max_active_tasks,
        }
    }

    /// Create a new task in Pending state, returning its task_id.
    ///
    /// Rejects with [`AppError::TooManyRequests`] (HTTP 429) when the number
    /// of active tasks already equals `max_active_tasks`.
    pub fn create_task(
        &self,
        task_id: String,
        operation: &str,
        total: usize,
        description: &str,
    ) -> Result<String, AppError> {
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

        // Cleanup, the active-count check and the insert all happen under a
        // single write lock so the cap is enforced atomically.
        let mut tasks = self.tasks.write().unwrap_or_else(PoisonError::into_inner);
        cleanup_tasks(&mut tasks, now);
        evict_oldest_terminal(&mut tasks, MAX_RETAINED_TASKS);

        let active = tasks
            .values()
            .filter(|t| matches!(t.state, TaskState::Pending | TaskState::Processing))
            .count();
        if self.max_active_tasks > 0 && active as u64 >= self.max_active_tasks {
            return Err(AppError::TooManyRequests);
        }

        tasks.insert(task_id.clone(), task);
        Ok(task_id)
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

    fn update_state(&self, task_id: &str, f: impl FnOnce(&mut CopyMoveTask)) {
        if let Ok(mut tasks) = self.tasks.write()
            && let Some(task) = tasks.get_mut(task_id)
        {
            f(task);
        }
    }
}

/// Evict terminal (Completed/Failed) tasks older than `CLEANUP_TTL_SECS`.
/// Active (Pending/Processing) tasks are always kept.
fn cleanup_tasks(tasks: &mut HashMap<String, CopyMoveTask>, now: i64) {
    tasks.retain(|_, t| {
        if matches!(t.state, TaskState::Completed | TaskState::Failed(_)) {
            now - t.created_at < CLEANUP_TTL_SECS
        } else {
            true
        }
    });
}

/// Cap the total number of tasks at `cap` by evicting the oldest terminal
/// (Completed/Failed) tasks. Active (Pending/Processing) tasks are never
/// evicted, so the cap is a best-effort bound on terminal-task retention.
fn evict_oldest_terminal(tasks: &mut HashMap<String, CopyMoveTask>, cap: usize) {
    if tasks.len() <= cap {
        return;
    }
    let mut terminal: Vec<(i64, String)> = tasks
        .iter()
        .filter(|(_, t)| matches!(t.state, TaskState::Completed | TaskState::Failed(_)))
        .map(|(k, t)| (t.created_at, k.clone()))
        .collect();
    terminal.sort_unstable_by_key(|(ts, _)| *ts);
    let mut to_drop = tasks.len() - cap;
    for (_, key) in terminal {
        if to_drop == 0 {
            break;
        }
        if tasks.remove(&key).is_some() {
            to_drop -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_task(id: &str, state: TaskState, created_at: i64) -> CopyMoveTask {
        CopyMoveTask {
            task_id: id.to_string(),
            state,
            operation: "copy".to_string(),
            total: 1,
            done_count: 0,
            created_at,
            description: String::new(),
        }
    }

    #[test]
    fn create_task_rejects_when_active_limit_reached() {
        let tm = TaskManager::new(1);
        assert!(tm.create_task("a".into(), "copy", 1, "a").is_ok());
        assert!(matches!(
            tm.create_task("b".into(), "copy", 1, "b"),
            Err(AppError::TooManyRequests)
        ));
    }

    #[test]
    fn create_task_allows_after_completion() {
        let tm = TaskManager::new(1);
        tm.create_task("a".into(), "copy", 1, "a").unwrap();
        tm.complete_task("a");
        assert!(tm.create_task("b".into(), "copy", 1, "b").is_ok());
    }

    #[test]
    fn evicts_oldest_terminal_over_cap() {
        let mut tasks = HashMap::new();
        tasks.insert("old".into(), mk_task("old", TaskState::Completed, 100));
        tasks.insert("mid".into(), mk_task("mid", TaskState::Completed, 200));
        tasks.insert("new".into(), mk_task("new", TaskState::Completed, 300));
        tasks.insert("active".into(), mk_task("active", TaskState::Pending, 400));

        evict_oldest_terminal(&mut tasks, 2);

        assert_eq!(tasks.len(), 2);
        assert!(
            tasks.contains_key("active"),
            "active task must never be evicted"
        );
        assert!(
            !tasks.contains_key("old"),
            "oldest terminal task should be evicted first"
        );
        assert!(
            !tasks.contains_key("mid"),
            "second-oldest terminal task should be evicted too"
        );
        assert!(tasks.contains_key("new"));
    }
}
