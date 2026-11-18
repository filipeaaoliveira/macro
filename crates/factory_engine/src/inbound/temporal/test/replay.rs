//! Runs in flight keep replaying: histories of `code` Runs recorded by
//! earlier workers replay on this workflow without a nondeterminism error.
//! They need no Temporal server, so they run in CI.
//!
//! The histories were recorded from the ignored tests in `test.rs` on a
//! Temporal dev server, then read with the client's `fetch_history` and `to_json`, with null
//! fields the history reads as their defaults left out:
//! - `code_merged`: a person approves the plan and the merge;
//! - `code_revised_then_cancelled`: a person sends the merge back, and
//!   cancels the Run at the next merge Gate;
//! - `code_review_unresolved`: review rounds keep finding Blockers, and the
//!   Run ends for a person;
//! - `code_waiting_at_merge_gate`: `code_merged` cut where the Run waits on
//!   its merge Gate, as a Run in flight is when a new worker takes it;
//! - `code_reply_answered`: the owner's words answer the plan Gate
//!   (`gate_replies.rs`);
//! - `code_reply_asked_back`: the words are asked back, and a button
//!   answers the Gate;
//! - `code_stand_in_answered`: the stand-in's answer to the plan Gate stands
//!   once its window passes;
//! - `code_stand_in_overridden`: the owner overrides it in the window.
//!
//! Before changing what the workflow decides, record the histories it
//! makes now beside these, so the change is checked against both.

use temporalio_client::WorkflowHistory;
use temporalio_sdk::workflow_replayer::{WorkflowReplayer, WorkflowReplayerOptions};

use crate::inbound::temporal::workflow::FlowWorkflow;

/// Replay `history` on this worker's workflow, and fail on any
/// nondeterminism.
async fn replays(name: &str, history: &[u8]) {
    let options = WorkflowReplayerOptions::new()
        .register_workflow::<FlowWorkflow>()
        .expect("register")
        .build();
    let replayer = WorkflowReplayer::new(options).expect("replayer");
    let history = WorkflowHistory::from_json(history).expect("the history reads");
    replayer
        .replay_workflow(history)
        .await
        .unwrap_or_else(|error| panic!("{name} does not replay: {error}"));
}

#[tokio::test]
async fn a_run_a_person_merged_replays() {
    replays("code_merged", include_bytes!("histories/code_merged.json")).await;
}

#[tokio::test]
async fn a_run_sent_back_and_cancelled_at_its_merge_gate_replays() {
    replays(
        "code_revised_then_cancelled",
        include_bytes!("histories/code_revised_then_cancelled.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_whose_review_stayed_unresolved_replays() {
    replays(
        "code_review_unresolved",
        include_bytes!("histories/code_review_unresolved.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_waiting_on_its_merge_gate_replays() {
    replays(
        "code_waiting_at_merge_gate",
        include_bytes!("histories/code_waiting_at_merge_gate.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_whose_plan_gate_a_reply_answered_replays() {
    replays(
        "code_reply_answered",
        include_bytes!("histories/code_reply_answered.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_whose_reply_was_asked_back_replays() {
    replays(
        "code_reply_asked_back",
        include_bytes!("histories/code_reply_asked_back.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_whose_plan_gate_the_stand_in_answered_replays() {
    replays(
        "code_stand_in_answered",
        include_bytes!("histories/code_stand_in_answered.json"),
    )
    .await;
}

#[tokio::test]
async fn a_run_whose_owner_overrode_the_stand_in_replays() {
    replays(
        "code_stand_in_overridden",
        include_bytes!("histories/code_stand_in_overridden.json"),
    )
    .await;
}
