//! What crosses the Tauri IPC boundary besides plain data: the error type every command returns,
//! and the task manager that runs long work off the main thread with progress and cancellation.
//!
//! Rules for commands (see `commands.rs`):
//! - every command returns `Result<T, AppError>`;
//! - commands name a registered project by id, never a raw path, except the flows that add a
//!   project (add existing, clone, create), which take the folder the user picked;
//! - there is no command that runs arbitrary git arguments or reads arbitrary files;
//! - anything that can take more than a moment runs as a task and returns its id at once.

mod error;
mod tasks;

pub use error::{AppError, ErrorKind};
pub use tasks::{EventSink, TaskCtx, TaskEvent, TaskId, TaskInfo, TaskManager, TaskOutcome};

#[cfg(test)]
mod tests;
