//! What each update of a Run says on its task's thread.

use chrono::{DateTime, Utc};
use factory_records::domain::models::{EntryId, GateAnswer, GateKind, Outcome, Role, Run, Step};
use factory_records::domain::review_rounds::ReviewRound;
use model_notifications::FactoryGateWaitingMetadata;

use super::{RunUpdate, ThreadComment, comment_id};
use crate::domain::built_in_flows::BUILT_IN_FLOWS;
use crate::domain::flow::StepUse;
use crate::domain::gate_owner::factory_signed;
use crate::domain::review_rounds::escape;

/// The Steps whose output starts by naming the Run's pull request.
const PULL_REQUEST_STEPS: [&str; 2] = ["publish", "merge"];

/// What an update's comment reads beside the Run's records.
#[derive(Debug, Clone, Copy)]
pub struct UpdateContext<'a> {
    /// The link that opens the Run.
    pub run_link: &'a str,
    /// The Run's review rounds.
    pub rounds: &'a [ReviewRound],
    /// For an answer a person gave, the name they show others.
    pub answerer_name: Option<&'a str>,
}

/// The thread's first comment: the Run started, with the link that opens
/// it, by which the thread is found again.
pub fn first_comment(run: &Run, run_link: &str) -> ThreadComment {
    ThreadComment {
        id: comment_id(run.id, "start", run.created_at),
        thread_id: None,
        text: factory_signed(&format!(
            "**A factory run started.** [Open the run]({run_link}); its updates follow in this thread."
        )),
    }
}

/// The comment that posts `update` of `run`, whose Steps are `steps`: none
/// when what it tells is not recorded, as a Step that has not ended, or is
/// told another way, as a review panel's Step is with its round.
pub fn update_comment(
    run: &Run,
    steps: &[Step],
    context: &UpdateContext<'_>,
    update: &RunUpdate,
) -> Option<ThreadComment> {
    let step_at = |seq: u16| steps.iter().find(|step| step.seq == seq);
    let reply = |event: String, at: DateTime<Utc>, text: String| ThreadComment {
        id: comment_id(run.id, &event, at),
        thread_id: Some(first_comment(run, context.run_link).id),
        text: factory_signed(&text),
    };

    match update {
        RunUpdate::Started => Some(first_comment(run, context.run_link)),
        RunUpdate::StepEnded { seq } => {
            let step = step_at(*seq)?;
            let end = step.end.as_ref()?;
            if on_review_panel(steps, context.rounds, &step.entry) {
                return None;
            }
            let mut text = format!(
                "Step {seq}, `{}`{}, ended `{}`.",
                step.entry,
                worked_by(step),
                end.outcome
            );
            // A pull request Step's output starts by naming its pull
            // request; any other Step's output stays on the Run.
            if runs_pull_request_step(&step.entry)
                && let Some(named) = end.output.lines().next()
            {
                text.push_str(&format!("\n\n{named}"));
            }
            Some(reply(format!("step:{seq}"), end.ended_at, text))
        }
        RunUpdate::RoundClosed { seq } => {
            let round = context.rounds.iter().find(|round| round.seq == *seq)?;
            let end = round.end.as_ref()?;
            let outcome = serde_json::to_value(end.outcome)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))?;
            let blockers = round.blockers().count();
            let found = match blockers {
                0 => String::new(),
                1 => " with 1 blocker".to_string(),
                many => format!(" with {many} blockers"),
            };
            let results = reviewer_results(steps, round);
            let reviewers = if results.is_empty() {
                String::new()
            } else {
                format!(": {}", results.join(", "))
            };
            let text = format!(
                "Review round {} ended `{outcome}`{found}{reviewers}.",
                round.round
            );
            Some(reply(format!("round:{seq}"), end.ended_at, text))
        }
        RunUpdate::GateWaiting { seq } => {
            let step = step_at(*seq)?;
            let gate = step.gate.as_ref()?;
            // An agent's question is the agent's words: the thread says
            // only that it asks.
            // So is the last message of an agent that stopped without a
            // result, which its Gate shows the owner.
            let stopped = step
                .end
                .as_ref()
                .is_some_and(|end| end.outcome == Outcome::no_result());
            let text = match gate.kind {
                GateKind::Question => format!(
                    "Step {seq}'s {} asks the run's owner a question.",
                    role_name(step.role.as_ref())
                ),
                GateKind::Flow if stopped => format!(
                    "Step {seq}'s {} stopped without handing back a result, and waits for the \
                     run's owner to answer `retry` or `stop`.",
                    role_name(step.role.as_ref())
                ),
                GateKind::Flow => {
                    let answers: Vec<String> = gate
                        .answers
                        .iter()
                        .map(|answer| format!("`{answer}`"))
                        .collect();
                    format!(
                        "Step {seq} waits for the run's owner to answer {}.\n\n> {}",
                        answers.join(" or "),
                        quoted(&gate.question)
                    )
                }
            };
            Some(reply(format!("waiting:{seq}"), gate.opened_at, text))
        }
        RunUpdate::GateAnswered { seq } => {
            let gate = step_at(*seq)?.gate.as_ref()?;
            let answer = gate.answer.as_ref()?;
            let text = format!(
                "Step {seq}'s gate was answered `{}` {}.",
                answer.answer,
                answered_by(answer, context.answerer_name)
            );
            Some(reply(format!("answer:{seq}"), answer.answered_at, text))
        }
        RunUpdate::GateTold { seq, question } => {
            let end = step_at(*seq)?.end.as_ref()?;
            let text = format!(
                "Step {seq} tells the run's owner, and the run goes on.\n\n> {}",
                quoted(question)
            );
            Some(reply(format!("notice:{seq}"), end.ended_at, text))
        }
        RunUpdate::PauseRunningOut { seq, ends_by } => {
            let step = step_at(*seq)?;
            let paused_at = step.terminal.paused_at.filter(|_| step.end.is_none())?;
            let text = format!(
                "Step {seq} is paused. Its time runs out at {} UTC: unless the run's owner resumes it from the run, it ends failed then.",
                ends_by.format("%Y-%m-%d %H:%M")
            );
            Some(reply(format!("pause:{seq}"), paused_at, text))
        }
        RunUpdate::Ended => {
            let end = run.end.as_ref()?;
            let text = format!("**The factory run ended** `{}`.", end.outcome);
            Some(reply("end".to_string(), end.ended_at, text))
        }
    }
}

/// Whether the entry `entry` is a review panel of the Run: the entry of a
/// round's first Step, among `steps`.
fn on_review_panel(steps: &[Step], rounds: &[ReviewRound], entry: &EntryId) -> bool {
    rounds.iter().any(|round| {
        steps
            .iter()
            .any(|step| step.seq == round.seq && step.entry == *entry)
    })
}

/// Whether the entry `entry` runs a pull request Step in a built-in Flow,
/// whose output names the Run's pull request.
fn runs_pull_request_step(entry: &EntryId) -> bool {
    BUILT_IN_FLOWS
        .into_iter()
        .filter_map(|flow| flow.read(|_| Some(StepUse::Work)).ok())
        .flat_map(|flow| flow.entries)
        .any(|built_in| {
            built_in.id == *entry && PULL_REQUEST_STEPS.contains(&built_in.step.as_str())
        })
}

/// Each reviewer of `round` and how they ended, in the order they first
/// appear: its Steps are its first Step's and the ones of the same entry
/// right after it, and a reviewer that went on from a question ended as
/// its last Step did.
fn reviewer_results(steps: &[Step], round: &ReviewRound) -> Vec<String> {
    let Some(first) = steps.iter().find(|step| step.seq == round.seq) else {
        return Vec::new();
    };
    let mut results: Vec<(String, String)> = Vec::new();
    let panel = steps
        .iter()
        .filter(|step| step.seq >= round.seq)
        .take_while(|step| step.entry == first.entry);
    for step in panel {
        let Some(end) = &step.end else { continue };
        let role = role_name(step.role.as_ref());
        let outcome = end.outcome.to_string();
        match results.iter_mut().find(|(named, _)| *named == role) {
            Some(result) => result.1 = outcome,
            None => results.push((role, outcome)),
        }
    }
    results
        .into_iter()
        .map(|(role, outcome)| format!("{role} `{outcome}`"))
        .collect()
}

/// The name a comment gives a Step's Role.
fn role_name(role: Option<&Role>) -> String {
    role.map_or_else(|| "agent".to_string(), ToString::to_string)
}

/// The start of `text`, on one line, as a comment quotes it: it opens no
/// tag, mention or code block, and carries no link or image.
fn quoted(text: &str) -> String {
    escape(&FactoryGateWaitingMetadata::excerpt(&without_links(text)))
}

/// `text` with no link or image: a Markdown link or image keeps its words,
/// and a bare web address is left out.
pub(super) fn without_links(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let image = rest.starts_with("![");
        let label_start = if image { 2 } else { 1 };
        if (image || rest.starts_with('['))
            && let Some(label_end) = rest[label_start..].find("](")
            && let Some(target_end) = rest[label_start + label_end + 2..].find(')')
        {
            kept.push_str(&rest[label_start..label_start + label_end]);
            rest = &rest[label_start + label_end + 2 + target_end + 1..];
            continue;
        }
        let lower = rest.get(..8).unwrap_or(rest).to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            kept.push_str("(link left out)");
            rest = &rest[end..];
            continue;
        }
        let Some(next) = rest.chars().next() else {
            break;
        };
        kept.push(next);
        rest = &rest[next.len_utf8()..];
    }
    kept
}

/// Who did `step`'s work, as a comment names them after its entry: its
/// Role and Agent, when it had them.
fn worked_by(step: &Step) -> String {
    match (&step.role, &step.agent) {
        (Some(role), Some(agent)) => format!(" ({role}, {agent})"),
        (Some(role), None) => format!(" ({role})"),
        _ => String::new(),
    }
}

/// Who or what gave `answer`, as a comment names them: a person by the
/// name they show others (`name`), else "the run's owner", since only
/// they answer, and the Orchestrator, an agent in Macro chat answering for
/// them, as itself.
fn answered_by(answer: &GateAnswer, name: Option<&str>) -> String {
    if let Some(rule) = answer.rule {
        return format!("by the `{rule}` Standing Rule");
    }
    if answer.by_stand_in {
        return "by the stand-in, for the run's owner".to_string();
    }
    let person = name.map_or_else(|| "the run's owner".to_string(), escape);
    if answer.agent_session_id.is_some() {
        return format!("by the Orchestrator, for {person}");
    }
    match answer.reply_confidence {
        Some(_) => format!("by {person}, read from their words"),
        None => format!("by {person}"),
    }
}
