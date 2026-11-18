//! Gate replies and the stand-in: a person's answer read from their words,
//! the factory asking back when it cannot read them, and the stand-in's
//! answer for the Run's owner, which the owner may override for a while.
//! The owner's past answers, which the stand-in reads, and the answered
//! Gates a replay measures the model on, come from the same records.

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::models::{
    EntryId, GateAnswer, GateKind, GateModelReading, Outcome, RunId, StandInAnswer, StepKey,
};

/// A person's past answer to a Gate, as the stand-in reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastAnswer {
    /// The Gate's question.
    pub question: String,
    /// The answer they gave.
    pub answer: Outcome,
    /// What they wrote with it.
    pub note: Option<String>,
}

/// A Gate a person answered, with what it asked about: what a replay of
/// the model reads, and the answer it is measured against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredGate {
    /// The Run.
    pub run_id: RunId,
    /// The Step that asked, among the Run's Steps.
    pub seq: u16,
    /// The entry whose Step asked.
    pub entry: EntryId,
    /// The Run's owner.
    pub owner: MacroUserIdStr<'static>,
    /// The built-in Flow the Run walked; none for a Run recorded before
    /// Runs kept it.
    pub flow: Option<String>,
    /// The Run's request.
    pub request: String,
    /// Who asked: the Flow, or the Step's agent.
    pub kind: GateKind,
    /// The question.
    pub question: String,
    /// The answers it offered.
    pub answers: Vec<Outcome>,
    /// What the Step that asked produced, which the Gate is about.
    pub output: String,
    /// The Run's plan: what the latest planner's Step before the Gate
    /// produced, if one did.
    pub plan: Option<String>,
    /// The person's answer.
    pub answer: Outcome,
    /// What they wrote with it.
    pub note: Option<String>,
    /// When they answered.
    pub answered_at: DateTime<Utc>,
}

/// Where Gate replies and the stand-in's answers are kept.
///
/// Each write keeps a Gate's first answer, so a retried activity never
/// answers a Gate twice, nor over a person's answer.
pub trait GateReplyRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Record that the factory asked back for `reply`, the person's words
    /// on the Gate of `key`, which the Gate model read as `reading`, and
    /// whether because they approved a Tampered Run (`refused_tampered`),
    /// while the Gate has no answer; a later reply replaces them. Whether
    /// it was recorded.
    fn ask_back(
        &self,
        key: StepKey,
        reply: &str,
        reading: &GateModelReading,
        refused_tampered: bool,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Answer the Gate of `key` with `answer`, read from `reply`, the words
    /// of `user_id`, which it keeps as its note, how sure the factory was
    /// (`confidence`, in percent; none for a question Gate, whose answer is
    /// the words themselves). The Gate keeps its first answer: what it
    /// keeps is returned, none when it is not recorded.
    fn answer_gate_from_reply(
        &self,
        key: StepKey,
        answer: &Outcome,
        reply: &str,
        user_id: &MacroUserIdStr<'static>,
        confidence: Option<u8>,
    ) -> impl Future<Output = Result<Option<GateAnswer>, Self::Err>> + Send;

    /// Keep the stand-in's `answer` on the Gate of `key`, while the Gate has
    /// no answer and no stand-in answer. What the Gate keeps of the
    /// stand-in is returned: the first answer, or none when the Gate was
    /// answered first or is not recorded.
    fn stand_in(
        &self,
        key: StepKey,
        answer: &StandInAnswer,
    ) -> impl Future<Output = Result<Option<StandInAnswer>, Self::Err>> + Send;

    /// Record that the stand-in, asked about the Gate of `key`, left it to
    /// the owner, and how the Gate model read it (`reading`), while the
    /// Gate has no answer and no stand-in answer; a retry replaces it.
    fn decline_stand_in(
        &self,
        key: StepKey,
        reading: &GateModelReading,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Answer the Gate of `key` with the stand-in's answer it keeps, once
    /// the owner let it stand, while the Gate has no answer. What the Gate
    /// keeps is returned, whichever answer that is; none when it is not
    /// recorded or the stand-in gave none.
    fn answer_gate_by_stand_in(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<GateAnswer>, Self::Err>> + Send;

    /// Take back the stand-in's answer on the Gate of `key`, while the
    /// Gate has no answer, so the Gate is the person's again.
    fn withdraw_stand_in(&self, key: StepKey)
    -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// `owner`'s own answers to the Gates of `entry` in their newest other
    /// Runs of the Flow the Run `of` walks, newest first, at most `limit`,
    /// leaving out the agents' questions asked at the entry.
    fn past_answers(
        &self,
        owner: &MacroUserIdStr<'static>,
        entry: &EntryId,
        of: RunId,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<PastAnswer>, Self::Err>> + Send;

    /// The Gates people answered themselves, with a button, oldest first,
    /// at most `limit`: no Rule Answer, no stand-in's, and none read from a
    /// person's words, which the model itself decided.
    fn answered_gates(
        &self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AnsweredGate>, Self::Err>> + Send;
}
