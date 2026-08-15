use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Pausing,
    Paused,
    Cancelling,
    AwaitingConfirmation,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSnapshot {
    pub id: String,
    pub kind: String,
    pub status: TaskStatus,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub progress: f64,
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub bytes_processed: u64,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
    pub attempts: u32,
    pub stage: String,
    pub payload: Value,
    pub preview: Option<Value>,
    pub confirm_decision: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<TaskFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskManagerError {
    NotFound,
    InvalidTransition { from: TaskStatus, to: TaskStatus },
    Persistence { message: String },
}

impl std::fmt::Display for TaskManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("task not found"),
            Self::InvalidTransition { from, to } => {
                write!(formatter, "cannot transition task from {from:?} to {to:?}")
            }
            Self::Persistence { message } => {
                write!(formatter, "task persistence failed: {message}")
            }
        }
    }
}

impl std::error::Error for TaskManagerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStoreError {
    message: String,
}

impl TaskStoreError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TaskStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TaskStoreError {}

impl From<TaskStoreError> for TaskManagerError {
    fn from(error: TaskStoreError) -> Self {
        Self::Persistence {
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskEvent {
    pub sequence: u64,
    pub task_id: String,
    pub revision: u64,
    pub event: String,
    pub task: TaskSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskEventReplay {
    pub events: Vec<TaskEvent>,
    pub requires_resync: bool,
    pub latest_sequence: u64,
}

const TASK_PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_millis(250);

pub trait TaskStore: Clone + Send + Sync + 'static {
    fn insert(&self, task: TaskSnapshot) -> Result<(), TaskStoreError>;
    fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError>;
    fn update(&self, task: TaskSnapshot) -> Result<(), TaskStoreError>;
    fn delete(&self, id: &str) -> Result<bool, TaskStoreError>;
    fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError>;
}

#[cfg(test)]
#[derive(Clone, Default)]
pub struct MemoryTaskStore {
    tasks: Arc<Mutex<HashMap<String, TaskSnapshot>>>,
}

#[cfg(test)]
impl TaskStore for MemoryTaskStore {
    fn insert(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
        self.tasks
            .lock()
            .map_err(|_| TaskStoreError::new("task store poisoned"))?
            .insert(task.id.clone(), task);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError> {
        Ok(self
            .tasks
            .lock()
            .map_err(|_| TaskStoreError::new("task store poisoned"))?
            .get(id)
            .cloned())
    }

    fn update(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
        self.insert(task)
    }

    fn delete(&self, id: &str) -> Result<bool, TaskStoreError> {
        Ok(self
            .tasks
            .lock()
            .map_err(|_| TaskStoreError::new("task store poisoned"))?
            .remove(id)
            .is_some())
    }

    fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError> {
        Ok(self
            .tasks
            .lock()
            .map_err(|_| TaskStoreError::new("task store poisoned"))?
            .values()
            .cloned()
            .collect())
    }
}

#[derive(Clone)]
pub struct SqliteTaskStore {
    database: Arc<crate::database::Database>,
}

impl SqliteTaskStore {
    pub fn new(database: Arc<crate::database::Database>) -> Self {
        Self { database }
    }

    fn persist_snapshot(&self, task: &TaskSnapshot) -> Result<(), TaskStoreError> {
        let progress = serde_json::json!({
            "progress": task.progress,
            "speed_bytes_per_sec": task.speed_bytes_per_sec,
            "eta_seconds": task.eta_seconds,
            "attempts": task.attempts,
            "preview": task.preview,
            "stage": task.stage,
            "confirm_decision": task.confirm_decision,
            "created_at_unix": task.created_at,
            "updated_at_unix": task.updated_at,
            "total_items": task.total_items,
        });
        let error = task
            .error
            .as_ref()
            .and_then(|failure| serde_json::to_value(failure).ok());
        self.database
            .update_task_snapshot(
                &task.id,
                task_status_name(task.status),
                task.revision as i64,
                &progress,
                task.result.as_ref(),
                error.as_ref(),
                task.total_items.unwrap_or(0) as i64,
                task.completed_items as i64,
                0,
                task.bytes_processed as i64,
                task.kind == "download",
            )
            .map(|_| ())
            .map_err(|error| TaskStoreError::new(error.to_string()))
    }
}

impl TaskStore for SqliteTaskStore {
    fn insert(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
        self.database
            .create_task(
                &task.id,
                &task.kind,
                &task.payload,
                task_status_name(task.status),
            )
            .map_err(|error| TaskStoreError::new(error.to_string()))?;
        self.persist_snapshot(&task)
    }

    fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError> {
        self.database
            .get_task(id)
            .map(|record| record.and_then(task_from_record))
            .map_err(|error| TaskStoreError::new(error.to_string()))
    }

    fn update(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
        self.persist_snapshot(&task)
    }

    fn delete(&self, id: &str) -> Result<bool, TaskStoreError> {
        self.database
            .delete_task(id)
            .map_err(|error| TaskStoreError::new(error.to_string()))
    }

    fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError> {
        self.database
            .list_all_tasks()
            .map(|records| records.into_iter().filter_map(task_from_record).collect())
            .map_err(|error| TaskStoreError::new(error.to_string()))
    }
}

pub(crate) fn task_from_record(record: crate::database::TaskRecord) -> Option<TaskSnapshot> {
    let status = parse_task_status(&record.status)?;
    let now = unix_timestamp();
    Some(TaskSnapshot {
        id: record.id,
        kind: record.kind,
        status,
        revision: record.revision.max(0) as u64,
        created_at: record
            .progress
            .get("created_at_unix")
            .and_then(Value::as_u64)
            .unwrap_or(now),
        updated_at: record
            .progress
            .get("updated_at_unix")
            .and_then(Value::as_u64)
            .unwrap_or(now),
        progress: record
            .progress
            .get("progress")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        completed_items: record.items_completed.max(0) as u64,
        total_items: record
            .progress
            .get("total_items")
            .and_then(Value::as_u64)
            .or_else(|| (record.items_total > 0).then_some(record.items_total as u64)),
        bytes_processed: record.bytes_processed.max(0) as u64,
        speed_bytes_per_sec: record
            .progress
            .get("speed_bytes_per_sec")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        eta_seconds: record.progress.get("eta_seconds").and_then(Value::as_u64),
        attempts: record
            .progress
            .get("attempts")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        stage: record
            .progress
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        payload: record.payload,
        preview: record
            .progress
            .get("preview")
            .cloned()
            .filter(|value| !value.is_null()),
        confirm_decision: record
            .progress
            .get("confirm_decision")
            .cloned()
            .filter(|value| !value.is_null()),
        result: record.result,
        error: record
            .error
            .and_then(|value| serde_json::from_value(value).ok()),
    })
}

fn task_status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Pausing => "pausing",
        TaskStatus::Paused => "paused",
        TaskStatus::Cancelling => "cancelling",
        TaskStatus::AwaitingConfirmation => "awaiting_confirmation",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn parse_task_status(status: &str) -> Option<TaskStatus> {
    match status {
        "queued" => Some(TaskStatus::Queued),
        "running" => Some(TaskStatus::Running),
        "pausing" => Some(TaskStatus::Pausing),
        "paused" => Some(TaskStatus::Paused),
        "cancelling" => Some(TaskStatus::Cancelling),
        "awaiting_confirmation" => Some(TaskStatus::AwaitingConfirmation),
        "completed" => Some(TaskStatus::Completed),
        "failed" => Some(TaskStatus::Failed),
        "cancelled" => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

#[derive(Clone)]
pub struct TaskManager<S: TaskStore> {
    store: S,
    snapshots: Arc<Mutex<HashMap<String, TaskSnapshot>>>,
    mutations: Arc<Mutex<()>>,
    progress_persisted_at: Arc<Mutex<HashMap<String, Instant>>>,
    events: tokio::sync::broadcast::Sender<TaskEvent>,
    sequence: Arc<AtomicU64>,
    replay: Arc<Mutex<VecDeque<TaskEvent>>>,
}

impl<S: TaskStore> TaskManager<S> {
    pub fn new(store: S) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(1_024);
        let initial_tasks = store.list().unwrap_or_default();
        let initial_sequence = initial_tasks
            .iter()
            .fold(0_u64, |total, task| total.saturating_add(task.revision));
        Self {
            store,
            snapshots: Arc::new(Mutex::new(
                initial_tasks
                    .into_iter()
                    .map(|task| (task.id.clone(), task))
                    .collect(),
            )),
            mutations: Arc::new(Mutex::new(())),
            progress_persisted_at: Arc::new(Mutex::new(HashMap::new())),
            events,
            sequence: Arc::new(AtomicU64::new(initial_sequence)),
            replay: Arc::new(Mutex::new(VecDeque::with_capacity(4_096))),
        }
    }

    pub fn create(
        &self,
        kind: impl Into<String>,
        payload: Value,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let now = unix_timestamp();
        let task = TaskSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            kind: kind.into(),
            status: TaskStatus::Queued,
            revision: 1,
            created_at: now,
            updated_at: now,
            progress: 0.0,
            completed_items: 0,
            total_items: None,
            bytes_processed: 0,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            attempts: 0,
            stage: String::new(),
            payload,
            preview: None,
            confirm_decision: None,
            result: None,
            error: None,
        };
        self.store.insert(task.clone())?;
        self.cache_task(&task);
        self.emit("created", &task);
        Ok(task)
    }

    pub fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskManagerError> {
        self.load_task(id)
    }

    /// Removes a terminal task after its owner has safely cleaned up any
    /// external files. A deleted event carries the final snapshot so all
    /// connected clients can remove the task without a polling delay.
    pub fn delete_terminal(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if !matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Cancelled,
            });
        }
        if !self.store.delete(id)? {
            return Err(TaskManagerError::NotFound);
        }
        self.snapshots
            .lock()
            .expect("task snapshot cache lock poisoned")
            .remove(id);
        self.clear_progress_throttle(id);
        self.emit("deleted", &task);
        Ok(task)
    }

    pub fn snapshot(&self) -> Result<Vec<TaskSnapshot>, TaskManagerError> {
        let mut tasks = self
            .snapshots
            .lock()
            .expect("task snapshot cache lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(tasks)
    }

    pub fn snapshot_with_sequence(&self) -> Result<(Vec<TaskSnapshot>, u64), TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        Ok((self.snapshot()?, self.sequence.load(Ordering::SeqCst)))
    }

    pub fn recover_interrupted(&self) -> Result<Vec<TaskSnapshot>, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut recovered = Vec::new();
        for mut task in self.snapshot()? {
            match task.status {
                TaskStatus::Pausing => {
                    task.status = TaskStatus::Paused;
                    task.error = None;
                }
                TaskStatus::Cancelling => {
                    task.status = TaskStatus::Cancelled;
                    task.error = None;
                }
                // Training state is written at safe reporting boundaries by the
                // telemetry bridge. On application restart it remains
                // resumable rather than being treated as an unrecoverable job.
                TaskStatus::Running if matches!(task.kind.as_str(), "download" | "training" | "reindex_library") => {
                    task.status = TaskStatus::Paused;
                    task.error = None;
                }
                TaskStatus::Running => {
                    task.status = TaskStatus::Failed;
                    task.error = Some(TaskFailure {
                        code: "interrupted".to_string(),
                        message: "应用退出时任务仍在运行".to_string(),
                        retryable: false,
                    });
                }
                _ => continue,
            }
            task.revision += 1;
            task.updated_at = unix_timestamp();
            self.persist_task(&task)?;
            self.emit("recovered", &task);
            recovered.push(task);
        }
        Ok(recovered)
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TaskEvent> {
        self.events.subscribe()
    }

    #[cfg(test)]
    pub fn last_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub fn clear_replay_for_test(&self) {
        self.replay
            .lock()
            .expect("task replay buffer poisoned")
            .clear();
    }

    #[cfg(test)]
    pub fn events_after(&self, sequence: u64) -> Vec<TaskEvent> {
        self.replay_after(sequence).events
    }

    pub fn replay_after(&self, sequence: u64) -> TaskEventReplay {
        let latest_sequence = self.sequence.load(Ordering::SeqCst);
        let replay = self.replay.lock().expect("task replay buffer poisoned");
        let requires_resync = sequence < latest_sequence
            && replay
                .front()
                .is_none_or(|event| sequence.saturating_add(1) < event.sequence);
        let events = replay
            .iter()
            .filter(|event| event.sequence > sequence)
            .cloned()
            .collect();
        TaskEventReplay {
            events,
            requires_resync,
            latest_sequence,
        }
    }

    pub fn start(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let task = self.transition(id, &[TaskStatus::Queued], TaskStatus::Running, "started")?;
        self.clear_progress_throttle(id);
        Ok(task)
    }

    pub fn pause(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let (target, event) = match task.status {
            TaskStatus::Queued => (TaskStatus::Paused, "paused"),
            TaskStatus::Running => (TaskStatus::Pausing, "pause_requested"),
            status => {
                return Err(TaskManagerError::InvalidTransition {
                    from: status,
                    to: TaskStatus::Pausing,
                });
            }
        };
        task.status = target;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit(event, &task);
        Ok(task)
    }

    pub fn resume(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let task = self.transition(
            id,
            &[TaskStatus::Paused, TaskStatus::Pausing],
            TaskStatus::Queued,
            "resumed",
        )?;
        self.clear_progress_throttle(id);
        Ok(task)
    }

    pub fn cancel(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let (target, event) = match task.status {
            TaskStatus::Running | TaskStatus::Pausing => {
                (TaskStatus::Cancelling, "cancel_requested")
            }
            TaskStatus::Queued | TaskStatus::Paused | TaskStatus::AwaitingConfirmation => {
                (TaskStatus::Cancelled, "cancelled")
            }
            status => {
                return Err(TaskManagerError::InvalidTransition {
                    from: status,
                    to: TaskStatus::Cancelling,
                });
            }
        };
        task.status = target;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit(event, &task);
        Ok(task)
    }

    pub fn acknowledge_stop(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let (target, event) = match task.status {
            TaskStatus::Pausing => (TaskStatus::Paused, "paused"),
            TaskStatus::Cancelling => (TaskStatus::Cancelled, "cancelled"),
            status => {
                return Err(TaskManagerError::InvalidTransition {
                    from: status,
                    to: TaskStatus::Paused,
                });
            }
        };
        task.status = target;
        task.speed_bytes_per_sec = 0;
        task.eta_seconds = None;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit(event, &task);
        Ok(task)
    }

    pub fn progress(
        &self,
        id: &str,
        completed_items: u64,
        total_items: u64,
        bytes_processed: u64,
        speed_bytes_per_sec: u64,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if task.status != TaskStatus::Running {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Running,
            });
        }
        task.completed_items = completed_items.min(total_items);
        task.total_items = Some(total_items);
        task.progress = if total_items == 0 {
            0.0
        } else {
            task.completed_items as f64 / total_items as f64
        };
        task.bytes_processed = bytes_processed;
        task.speed_bytes_per_sec = speed_bytes_per_sec;
        task.eta_seconds = if task.completed_items > 0 && speed_bytes_per_sec > 0 {
            let remaining_items = total_items.saturating_sub(task.completed_items);
            let estimated_remaining_bytes =
                bytes_processed.saturating_mul(remaining_items) / task.completed_items;
            Some(estimated_remaining_bytes.div_ceil(speed_bytes_per_sec))
        } else {
            None
        };
        if !self.should_persist_progress(id) {
            return Ok(task);
        }
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        if completed_items > 0 || bytes_processed > 0 {
            self.mark_progress_persisted(id);
        } else {
            self.clear_progress_throttle(id);
        }
        self.emit("progress", &task);
        Ok(task)
    }

    pub fn stream_progress(
        &self,
        id: &str,
        bytes_processed: u64,
        speed_bytes_per_sec: u64,
        eta_seconds: Option<u64>,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if task.status != TaskStatus::Running {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Running,
            });
        }
        let first_live_transfer = task.bytes_processed == 0 && bytes_processed > 0;
        task.bytes_processed = task.bytes_processed.max(bytes_processed);
        task.speed_bytes_per_sec = speed_bytes_per_sec;
        task.eta_seconds = eta_seconds;
        if !first_live_transfer && !self.should_persist_progress(id) {
            return Ok(task);
        }
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.mark_progress_persisted(id);
        self.emit("progress", &task);
        Ok(task)
    }

    pub fn complete(&self, id: &str, result: Value) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if task.status != TaskStatus::Running {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Completed,
            });
        }
        task.status = TaskStatus::Completed;
        task.progress = 1.0;
        if let Some(total) = task.total_items {
            task.completed_items = total;
        }
        task.eta_seconds = Some(0);
        task.result = Some(result);
        task.error = None;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit("completed", &task);
        Ok(task)
    }

    pub fn fail(&self, id: &str, failure: TaskFailure) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if !matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Failed,
            });
        }
        task.status = TaskStatus::Failed;
        task.error = Some(failure);
        task.eta_seconds = None;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit("failed", &task);
        Ok(task)
    }

    pub fn retry(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let retryable = task.error.as_ref().is_some_and(|failure| failure.retryable);
        if task.status != TaskStatus::Failed || !retryable {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Queued,
            });
        }
        task.status = TaskStatus::Queued;
        task.progress = 0.0;
        task.completed_items = 0;
        task.bytes_processed = 0;
        task.speed_bytes_per_sec = 0;
        task.eta_seconds = None;
        task.attempts += 1;
        task.error = None;
        task.result = None;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit("retried", &task);
        Ok(task)
    }

    pub fn await_confirmation(
        &self,
        id: &str,
        preview: Value,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if task.status != TaskStatus::Running {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::AwaitingConfirmation,
            });
        }
        task.status = TaskStatus::AwaitingConfirmation;
        task.preview = Some(preview);
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit("confirmation_required", &task);
        Ok(task)
    }

    pub fn confirm(&self, id: &str) -> Result<TaskSnapshot, TaskManagerError> {
        self.transition(
            id,
            &[TaskStatus::AwaitingConfirmation],
            TaskStatus::Queued,
            "confirmed",
        )
    }

    /// Resolves an in-flight confirmation by moving the task back to
    /// Running (the worker is alive and waiting for this decision) and
    /// recording the user's choice. The worker picks it up via
    /// [`TaskManager::take_confirm_decision`].
    pub fn confirm_with_decision(
        &self,
        id: &str,
        decision: Option<Value>,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if task.status != TaskStatus::AwaitingConfirmation {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: TaskStatus::Running,
            });
        }
        task.status = TaskStatus::Running;
        task.confirm_decision = decision;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.clear_progress_throttle(id);
        self.emit("confirmed", &task);
        Ok(task)
    }

    #[allow(dead_code)]
    /// Reads and clears the decision attached by the last
    /// `confirm_with_decision` call. Called by the waiting worker once it
    /// resumes, so a stale decision never leaks into a later phase.
    pub fn take_confirm_decision(&self, id: &str) -> Result<Option<Value>, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let decision = task.confirm_decision.take();
        if decision.is_none() {
            return Ok(None);
        }
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.emit("confirmed", &task);
        Ok(decision)
    }

    /// Reports a coarse-grained execution stage (e.g. "detecting",
    /// "retagging") without touching numeric progress. Changes are always
    /// persisted and broadcast immediately, so a phase change is visible on
    /// the frontend even while item counts stay identical.
    pub fn set_stage(&self, id: &str, stage: impl Into<String>) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        let stage = stage.into();
        if task.status != TaskStatus::Running || task.stage == stage {
            return Ok(task);
        }
        task.stage = stage;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.emit("stage", &task);
        Ok(task)
    }

    fn transition(
        &self,
        id: &str,
        allowed: &[TaskStatus],
        target: TaskStatus,
        event: &str,
    ) -> Result<TaskSnapshot, TaskManagerError> {
        let _mutation = self.mutations.lock().expect("task mutation lock poisoned");
        let mut task = self.load_task(id)?.ok_or(TaskManagerError::NotFound)?;
        if !allowed.contains(&task.status) {
            return Err(TaskManagerError::InvalidTransition {
                from: task.status,
                to: target,
            });
        }
        task.status = target;
        task.revision += 1;
        task.updated_at = unix_timestamp();
        self.persist_task(&task)?;
        self.emit(event, &task);
        Ok(task)
    }

    fn load_task(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskManagerError> {
        if let Some(task) = self
            .snapshots
            .lock()
            .expect("task snapshot cache lock poisoned")
            .get(id)
            .cloned()
        {
            return Ok(Some(task));
        }
        let task = self.store.get(id)?;
        if let Some(task) = task.as_ref() {
            self.cache_task(task);
        }
        Ok(task)
    }

    fn persist_task(&self, task: &TaskSnapshot) -> Result<(), TaskManagerError> {
        self.store.update(task.clone())?;
        self.cache_task(task);
        Ok(())
    }

    fn cache_task(&self, task: &TaskSnapshot) {
        self.snapshots
            .lock()
            .expect("task snapshot cache lock poisoned")
            .insert(task.id.clone(), task.clone());
    }

    fn should_persist_progress(&self, id: &str) -> bool {
        self.progress_persisted_at
            .lock()
            .expect("task progress throttle lock poisoned")
            .get(id)
            .is_none_or(|last| last.elapsed() >= TASK_PROGRESS_PERSIST_INTERVAL)
    }

    fn mark_progress_persisted(&self, id: &str) {
        self.progress_persisted_at
            .lock()
            .expect("task progress throttle lock poisoned")
            .insert(id.to_string(), Instant::now());
    }

    fn clear_progress_throttle(&self, id: &str) {
        self.progress_persisted_at
            .lock()
            .expect("task progress throttle lock poisoned")
            .remove(id);
    }

    fn emit(&self, event: &str, task: &TaskSnapshot) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let task_event = TaskEvent {
            sequence,
            task_id: task.id.clone(),
            revision: task.revision,
            event: event.to_string(),
            task: task.clone(),
        };
        {
            let mut replay = self.replay.lock().expect("task replay buffer poisoned");
            if replay.len() == 4_096 {
                replay.pop_front();
            }
            replay.push_back(task_event.clone());
        }
        let _ = self.events.send(task_event);
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Condvar;
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct FailingTaskStore;

    impl TaskStore for FailingTaskStore {
        fn insert(&self, _task: TaskSnapshot) -> Result<(), TaskStoreError> {
            Err(TaskStoreError::new("injected insert failure"))
        }

        fn get(&self, _id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError> {
            Ok(None)
        }

        fn update(&self, _task: TaskSnapshot) -> Result<(), TaskStoreError> {
            Err(TaskStoreError::new("injected update failure"))
        }

        fn delete(&self, _id: &str) -> Result<bool, TaskStoreError> {
            Err(TaskStoreError::new("injected delete failure"))
        }

        fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone, Default)]
    struct CountingTaskStore {
        inner: MemoryTaskStore,
        get_calls: Arc<AtomicUsize>,
    }

    impl CountingTaskStore {
        fn reset_get_calls(&self) {
            self.get_calls.store(0, Ordering::SeqCst);
        }

        fn get_calls(&self) -> usize {
            self.get_calls.load(Ordering::SeqCst)
        }
    }

    impl TaskStore for CountingTaskStore {
        fn insert(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
            self.inner.insert(task)
        }

        fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.get(id)
        }

        fn update(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
            self.inner.update(task)
        }

        fn delete(&self, id: &str) -> Result<bool, TaskStoreError> {
            self.inner.delete(id)
        }

        fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError> {
            self.inner.list()
        }
    }

    #[test]
    fn failed_insert_does_not_create_or_emit_a_task() {
        let manager = TaskManager::new(FailingTaskStore);
        let mut events = manager.subscribe();

        let result = manager.create("download", serde_json::json!({}));

        assert!(matches!(result, Err(TaskManagerError::Persistence { .. })));
        assert!(events.try_recv().is_err());
    }

    #[derive(Default)]
    struct RaceControl {
        armed: bool,
        get_arrivals: usize,
        paused_written: bool,
    }

    #[derive(Clone, Default)]
    struct RacingTaskStore {
        inner: MemoryTaskStore,
        control: Arc<(Mutex<RaceControl>, Condvar)>,
    }

    impl RacingTaskStore {
        fn arm(&self) {
            self.control.0.lock().unwrap().armed = true;
        }
    }

    impl TaskStore for RacingTaskStore {
        fn insert(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
            self.inner.insert(task)
        }

        fn get(&self, id: &str) -> Result<Option<TaskSnapshot>, TaskStoreError> {
            let snapshot = self.inner.get(id)?;
            let (lock, ready) = &*self.control;
            let mut control = lock.lock().unwrap();
            if control.armed {
                control.get_arrivals += 1;
                if control.get_arrivals < 2 {
                    control = ready
                        .wait_timeout(control, Duration::from_millis(50))
                        .unwrap()
                        .0;
                } else {
                    ready.notify_all();
                }
            }
            drop(control);
            Ok(snapshot)
        }

        fn update(&self, task: TaskSnapshot) -> Result<(), TaskStoreError> {
            let (lock, ready) = &*self.control;
            let mut control = lock.lock().unwrap();
            if control.armed && task.revision >= 3 {
                if task.status == TaskStatus::Paused {
                    self.inner.update(task)?;
                    control.paused_written = true;
                    ready.notify_all();
                    return Ok(());
                }
                if task.status == TaskStatus::Running && !control.paused_written {
                    control = ready
                        .wait_timeout(control, Duration::from_millis(50))
                        .unwrap()
                        .0;
                }
            }
            drop(control);
            self.inner.update(task)
        }

        fn delete(&self, id: &str) -> Result<bool, TaskStoreError> {
            self.inner.delete(id)
        }

        fn list(&self) -> Result<Vec<TaskSnapshot>, TaskStoreError> {
            self.inner.list()
        }
    }

    #[test]
    fn new_task_is_persisted_as_queued() {
        let store = MemoryTaskStore::default();
        let manager = TaskManager::new(store);

        let task = manager
            .create("download", serde_json::json!({"query": "cat"}))
            .unwrap();

        assert_eq!(task.status, TaskStatus::Queued);
        assert_eq!(task.revision, 1);
        assert_eq!(manager.get(&task.id).unwrap().unwrap(), task);
    }

    #[test]
    fn events_have_monotonic_global_sequence_and_task_revision() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let mut events = manager.subscribe();

        let first = manager.create("download", serde_json::json!({})).unwrap();
        let second = manager.create("index", serde_json::json!({})).unwrap();
        let first_event = events.try_recv().unwrap();
        let second_event = events.try_recv().unwrap();

        assert_eq!(
            (
                first_event.sequence,
                first_event.task_id,
                first_event.revision
            ),
            (1, first.id, 1)
        );
        assert_eq!(
            (
                second_event.sequence,
                second_event.task_id,
                second_event.revision
            ),
            (2, second.id, 1)
        );
    }

    #[test]
    fn running_task_can_be_paused_without_losing_snapshot() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager
            .create("download", serde_json::json!({"query": "cat"}))
            .unwrap();

        manager.start(&task.id).unwrap();
        let requested = manager.pause(&task.id).unwrap();

        assert_eq!(requested.status, TaskStatus::Pausing);
        assert_eq!(requested.revision, 3);
        assert_eq!(
            manager.get(&task.id).unwrap().unwrap().payload["query"],
            "cat"
        );
    }

    #[test]
    fn running_pause_request_waits_for_worker_acknowledgement() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();

        let requested = manager.pause(&task.id).unwrap();

        assert_eq!(serde_json::to_value(requested.status).unwrap(), "pausing");
        assert_ne!(requested.status, TaskStatus::Paused);
    }

    #[test]
    fn worker_acknowledgement_finishes_a_pause_request() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        manager.pause(&task.id).unwrap();

        let paused = manager.acknowledge_stop(&task.id).unwrap();

        assert_eq!(paused.status, TaskStatus::Paused);
        assert_eq!(paused.speed_bytes_per_sec, 0);
        assert_eq!(paused.eta_seconds, None);
    }

    #[test]
    fn concurrent_progress_cannot_overwrite_a_pause_transition() {
        let store = RacingTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        store.arm();

        let progress_manager = manager.clone();
        let progress_id = task.id.clone();
        let progress = std::thread::spawn(move || {
            let _ = progress_manager.progress(&progress_id, 1, 2, 10, 10);
        });
        let pause_manager = manager.clone();
        let pause_id = task.id.clone();
        let pause = std::thread::spawn(move || {
            let _ = pause_manager.pause(&pause_id);
        });
        progress.join().unwrap();
        pause.join().unwrap();

        assert_eq!(
            manager.get(&task.id).unwrap().unwrap().status,
            TaskStatus::Pausing
        );
    }

    #[test]
    fn paused_task_resumes_to_the_queue() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.pause(&task.id).unwrap();

        let resumed = manager.resume(&task.id).unwrap();

        assert_eq!(resumed.status, TaskStatus::Queued);
        assert_eq!(resumed.revision, 3);
    }

    #[test]
    fn running_cancel_waits_for_worker_ack_before_becoming_terminal() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();

        let requested = manager.cancel(&task.id).unwrap();

        assert_eq!(requested.status, TaskStatus::Cancelling);
        let cancelled = manager.acknowledge_stop(&task.id).unwrap();
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
        assert!(matches!(
            manager.resume(&task.id),
            Err(TaskManagerError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn progress_snapshot_contains_speed_bytes_and_eta() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();

        let progress = manager.progress(&task.id, 4, 10, 1_000, 200).unwrap();

        assert_eq!(progress.progress, 0.4);
        assert_eq!(progress.bytes_processed, 1_000);
        assert_eq!(progress.speed_bytes_per_sec, 200);
        assert_eq!(progress.eta_seconds, Some(8));
        assert_eq!(progress.revision, 3);
    }

    #[test]
    fn stream_progress_uses_the_cached_task_snapshot_between_persist_intervals() {
        let store = CountingTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        store.reset_get_calls();

        manager
            .stream_progress(&task.id, 128, 128, Some(10))
            .unwrap();
        manager
            .stream_progress(&task.id, 256, 256, Some(5))
            .unwrap();

        assert_eq!(store.get_calls(), 0);
    }

    #[test]
    fn rapid_progress_updates_do_not_flood_task_events() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let mut events = manager.subscribe();
        let task = manager
            .create("index_library", serde_json::json!({}))
            .unwrap();
        manager.start(&task.id).unwrap();
        while events.try_recv().is_ok() {}

        manager.progress(&task.id, 1, 100, 0, 0).unwrap();
        let first_progress = events.try_recv().expect("first progress event");
        manager.progress(&task.id, 2, 100, 0, 0).unwrap();

        assert_eq!(first_progress.task.completed_items, 1);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn completion_persists_a_terminal_result() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("index", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();

        let completed = manager
            .complete(&task.id, serde_json::json!({"indexed": 42}))
            .unwrap();

        assert_eq!(completed.status, TaskStatus::Completed);
        assert_eq!(completed.progress, 1.0);
        assert_eq!(completed.result.unwrap()["indexed"], 42);
    }

    #[test]
    fn retry_clears_retryable_failure_and_requeues_task() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        manager
            .fail(
                &task.id,
                TaskFailure {
                    code: "rate_limited".into(),
                    message: "later".into(),
                    retryable: true,
                },
            )
            .unwrap();

        let retried = manager.retry(&task.id).unwrap();

        assert_eq!(retried.status, TaskStatus::Queued);
        assert_eq!(retried.attempts, 1);
        assert_eq!(retried.error, None);
        assert_eq!(retried.progress, 0.0);
    }

    #[test]
    fn destructive_task_waits_for_confirmation_with_a_preview() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let task = manager
            .create("deduplicate", serde_json::json!({}))
            .unwrap();
        manager.start(&task.id).unwrap();

        let waiting = manager
            .await_confirmation(&task.id, serde_json::json!({"candidates": ["a.jpg"]}))
            .unwrap();

        assert_eq!(waiting.status, TaskStatus::AwaitingConfirmation);
        assert_eq!(waiting.preview.as_ref().unwrap()["candidates"][0], "a.jpg");
        assert_eq!(
            manager.confirm(&task.id).unwrap().status,
            TaskStatus::Queued
        );
    }

    #[test]
    fn snapshot_lists_all_tasks_for_late_subscribers() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let first = manager.create("download", serde_json::json!({})).unwrap();
        let second = manager.create("index", serde_json::json!({})).unwrap();

        let snapshot = manager.snapshot().unwrap();

        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|task| task.id == first.id));
        assert!(snapshot.iter().any(|task| task.id == second.id));
    }

    #[test]
    fn restart_pauses_downloads_and_training_but_fails_other_interrupted_tasks() {
        let store = MemoryTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let download = manager.create("download", serde_json::json!({})).unwrap();
        let training = manager.create("training", serde_json::json!({})).unwrap();
        let resize = manager.create("resize", serde_json::json!({})).unwrap();
        manager.start(&download.id).unwrap();
        manager.start(&training.id).unwrap();
        manager.start(&resize.id).unwrap();

        let restarted = TaskManager::new(store);
        restarted.recover_interrupted().unwrap();

        assert_eq!(
            restarted.get(&download.id).unwrap().unwrap().status,
            TaskStatus::Paused
        );
        assert_eq!(
            restarted.get(&training.id).unwrap().unwrap().status,
            TaskStatus::Paused
        );
        let failed_resize = restarted.get(&resize.id).unwrap().unwrap();
        assert_eq!(failed_resize.status, TaskStatus::Failed);
        assert_eq!(failed_resize.error.unwrap().code, "interrupted");
    }

    #[test]
    fn restart_acknowledges_a_pending_pause_after_the_worker_is_gone() {
        let store = MemoryTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let task = manager.create("resize", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        manager.pause(&task.id).unwrap();

        let restarted = TaskManager::new(store);
        restarted.recover_interrupted().unwrap();

        assert_eq!(
            restarted.get(&task.id).unwrap().unwrap().status,
            TaskStatus::Paused
        );
    }

    #[test]
    fn restart_acknowledges_a_pending_cancellation_after_the_worker_is_gone() {
        let store = MemoryTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let task = manager.create("resize", serde_json::json!({})).unwrap();
        manager.start(&task.id).unwrap();
        manager.cancel(&task.id).unwrap();

        let restarted = TaskManager::new(store);
        restarted.recover_interrupted().unwrap();

        assert_eq!(
            restarted.get(&task.id).unwrap().unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn recent_events_can_be_replayed_after_reconnect() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        manager
            .create("download", serde_json::json!({"n": 1}))
            .unwrap();
        manager
            .create("download", serde_json::json!({"n": 2}))
            .unwrap();
        manager
            .create("download", serde_json::json!({"n": 3}))
            .unwrap();

        let replay = manager.events_after(1);

        assert_eq!(manager.last_sequence(), 3);
        assert_eq!(
            replay
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn replay_window_reports_when_the_requested_sequence_has_fallen_out() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        for number in 0..4_100 {
            manager
                .create("download", serde_json::json!({"number": number}))
                .unwrap();
        }

        let replay = manager.replay_after(1);

        assert!(replay.requires_resync);
        assert_eq!(replay.latest_sequence, 4_100);
        assert_eq!(replay.events.len(), 4_096);
        assert_eq!(replay.events[0].sequence, 5);
    }

    #[test]
    fn event_after_an_atomic_snapshot_boundary_is_replayable() {
        let manager = TaskManager::new(MemoryTaskStore::default());
        let first = manager
            .create("download", serde_json::json!({"n": 1}))
            .unwrap();

        let (snapshot, sequence) = manager.snapshot_with_sequence().unwrap();
        let second = manager
            .create("download", serde_json::json!({"n": 2}))
            .unwrap();
        let replay = manager.events_after(sequence);

        assert_eq!(sequence, 1);
        assert_eq!(
            snapshot.iter().map(|task| &task.id).collect::<Vec<_>>(),
            vec![&first.id]
        );
        assert_eq!(
            replay
                .iter()
                .map(|event| &event.task_id)
                .collect::<Vec<_>>(),
            vec![&second.id]
        );
        assert_eq!(replay[0].sequence, sequence + 1);
    }

    #[test]
    fn sqlite_store_survives_a_manager_restart() {
        let directory = tempfile::tempdir().unwrap();
        let database =
            Arc::new(crate::database::Database::open(&directory.path().join("tasks.db")).unwrap());
        let store = SqliteTaskStore::new(database);
        let manager = TaskManager::new(store.clone());
        let task = manager
            .create("download", serde_json::json!({"query": "cat"}))
            .unwrap();
        manager.start(&task.id).unwrap();

        let restarted = TaskManager::new(store);

        assert_eq!(
            restarted.get(&task.id).unwrap().unwrap().status,
            TaskStatus::Running
        );
        assert_eq!(
            restarted.get(&task.id).unwrap().unwrap().payload["query"],
            "cat"
        );
    }

    #[test]
    fn manager_restart_keeps_event_sequences_above_persisted_history() {
        let store = MemoryTaskStore::default();
        let manager = TaskManager::new(store.clone());
        let first = manager.create("download", serde_json::json!({})).unwrap();
        manager.start(&first.id).unwrap();
        manager.create("download", serde_json::json!({})).unwrap();
        let previous_sequence = manager.last_sequence();
        drop(manager);

        let restarted = TaskManager::new(store);
        let mut events = restarted.subscribe();
        restarted.create("download", serde_json::json!({})).unwrap();
        let event = events.try_recv().unwrap();

        assert!(event.sequence > previous_sequence);
    }

    #[test]
    fn sqlite_task_store_does_not_drop_tasks_past_the_ui_page_size() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(
            crate::database::Database::open(&directory.path().join("many-tasks.db")).unwrap(),
        );
        for number in 0..1_005 {
            database
                .create_task(
                    &format!("task-{number:04}"),
                    "download",
                    &serde_json::json!({"number": number}),
                    "queued",
                )
                .unwrap();
        }
        let store = SqliteTaskStore::new(database);

        assert_eq!(store.list().unwrap().len(), 1_005);
    }
}
