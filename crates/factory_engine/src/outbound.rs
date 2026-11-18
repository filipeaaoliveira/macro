//! Outbound (driven) adapters of the factory's engine.

pub mod auth_user_directory;
pub mod document_hold_tasks;
pub mod document_task_directory;
pub mod document_task_notes;
pub mod document_task_thread;
pub mod fast_gate_model;
pub mod github_pull_requests;
pub mod ingress_gate_notifier;
pub mod local_agent;
pub mod model_run_judge;
pub mod routine_automation_triggers;
pub(crate) mod task_project;
pub mod temporal_main_watches;
pub mod temporal_run_workflows;
