//! Outbound (driven) adapters of the factory's engine.

pub mod auth_user_directory;
pub mod document_hold_tasks;
pub mod document_task_directory;
pub mod github_pull_requests;
pub mod local_agent;
pub(crate) mod task_project;
pub mod temporal_main_watches;
pub mod temporal_run_workflows;
