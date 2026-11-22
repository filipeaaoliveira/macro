//! OpenAI Codex through the `@agentclientprotocol/codex-acp` adapter
//! (`agentInfo.name = "@agentclientprotocol/codex-acp"`; the deprecated Rust
//! adapter announced `codex-acp` and emitted nothing for subagents).
//!
//! Writes its own keys under `_meta.codex` ([`CodexMeta`]), and two flat flags:
//!
//! - `_meta.is_mcp_tool_call: true` with the title `mcp.<server>.<tool>` -
//!   how it names an MCP tool, where Claude Code uses `mcp__<server>__<tool>`.
//! - `_meta.codex.collaboration = { tool, senderThreadId, receiverThreadIds }`
//!   on the collaboration tools. `tool: "spawnAgent"` is the delegation; the
//!   others (`sendInput`, `wait`, `closeAgent`, …) steer an existing one.
//!
//! Its `request_user_input` asks through `elicitation/create` (adapter ≥
//! 1.8): per question an `<id>` select and an `<id>__other` free-text
//! companion marked `_meta.codex = { questionId, isOtherAnswer: true }`. No
//! tool call goes over the wire for it, so nothing reports the answer back.
//!
//! A `spawnAgent` call is `kind: other`, titled `spawnAgent` for its whole
//! life, and carries its state in `rawInput` ([`SpawnAgentInput`]) - there is
//! no `rawOutput`. The child's own activity is not streamed into the parent
//! session by this (legacy) mode; the adapter's native-subagent mode uses
//! `sessionUpdate` variants ACP has not standardized, which this fold does
//! not read yet.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::Meta;
use lazy_regex::{regex_captures, regex_is_match};
use serde::Deserialize;
use serde_json::Value;

use super::{HarnessReader, SubagentInput, ToolFrame, generic, has_namespace, namespaced, raw};
use crate::domain::model::{FailureNotice, FailureNoticeKind, SubagentResult, ToolName};

/// The `_meta` namespace codex-acp writes under.
pub const NAMESPACE: &str = "codex";

/// Reader for codex-acp's conventions.
pub struct Codex;

/// The error classes Codex gives a turn its account's usage limit ended:
/// codex-acp's (`usageLimitExceeded`) and the deprecated Rust adapter's
/// (`usage_limit_exceeded`).
const USAGE_LIMIT_EXCEEDED: [&str; 2] = ["usageLimitExceeded", "usage_limit_exceeded"];

/// What codex-acp puts in the `data` of the error it answers a prompt with
/// when Codex ended the turn in error: Codex's own message and the class of
/// the error, beside JSON-RPC's bare `Internal error`. The deprecated Rust
/// adapter wrote the class as `codex_error_info`.
#[derive(Debug, Deserialize)]
struct TurnErrorData {
    #[serde(default)]
    message: Option<String>,
    #[serde(rename = "codexErrorInfo", alias = "codex_error_info")]
    codex_error_info: Value,
}

/// A turn Codex ended in error, as its prompt error's `data` tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexTurnFailure {
    /// Codex's own message, which the error's message (JSON-RPC's bare
    /// `Internal error`) leaves out; none when `data` has none.
    pub message: Option<String>,
    /// The failure in the person's terms, when it is one they can act on:
    /// the usage limit of the Codex account the runtime is signed in with.
    pub notice: Option<FailureNotice>,
}

/// The turn failure a prompt error's `data` describes, when it is codex-acp's
/// (`{ message, codexErrorInfo }`): none for any other `data`, which another
/// runtime's notice or debugging payload may be.
pub fn turn_failure(data: Option<&Value>) -> Option<CodexTurnFailure> {
    let data: TurnErrorData = serde_json::from_value(data?.clone()).ok()?;
    let message = data
        .message
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty());
    let usage_limit = data
        .codex_error_info
        .as_str()
        .is_some_and(|class| USAGE_LIMIT_EXCEEDED.contains(&class));
    let notice = usage_limit.then(|| FailureNotice {
        kind: FailureNoticeKind::ProviderUsageLimit,
        title: "Codex usage limit reached".to_owned(),
        body: format!(
            "{}The limit is on the Codex account signed in where this agent runs. Send your \
             message again once it resets, or ask another agent.",
            message
                .as_deref()
                .map_or_else(String::new, |message| format!("{message}\n\n"))
        ),
        link: None,
    });
    Some(CodexTurnFailure { message, notice })
}

/// `_meta.codex`, as far as the fold reads it.
#[derive(Debug, Default, Deserialize)]
struct CodexMeta {
    collaboration: Option<Collaboration>,
}

#[derive(Debug, Deserialize)]
struct Collaboration {
    tool: String,
}

/// `_meta.codex` on an elicitation form's "Other" property.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtherAnswerMarker {
    question_id: String,
    #[serde(default)]
    is_other_answer: bool,
}

/// What the adapter appends to a question's id for its "Other" companion.
const OTHER_ANSWER_SUFFIX: &str = "__other";

/// `_meta` flags codex-acp writes at the top level.
#[derive(Debug, Default, Deserialize)]
struct CodexFlags {
    #[serde(default)]
    is_mcp_tool_call: bool,
}

/// `spawnAgent`'s `rawInput`, which is also where its result lives.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnAgentInput {
    prompt: Option<String>,
    #[serde(default)]
    receiver_thread_ids: Vec<String>,
    #[serde(default)]
    agents_states: BTreeMap<String, AgentState>,
    model: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentState {
    message: Option<String>,
}

fn flags_of(meta: Option<&Meta>) -> CodexFlags {
    meta.and_then(|meta| serde_json::from_value(Value::Object(meta.clone())).ok())
        .unwrap_or_default()
}

impl HarnessReader for Codex {
    fn announces(&self, name: &str) -> bool {
        regex_is_match!(r"(?i)codex", name)
    }

    /// Either of its `_meta` signals: the namespace, or the flat MCP flag it
    /// writes on MCP calls whether or not the namespace is present.
    fn wrote(&self, frame: &ToolFrame<'_>) -> bool {
        has_namespace(frame.meta, NAMESPACE) || flags_of(frame.meta).is_mcp_tool_call
    }

    fn reported_tool_name(&self, frame: &ToolFrame<'_>) -> Option<ToolName> {
        if !flags_of(frame.meta).is_mcp_tool_call {
            return None;
        }
        regex_captures!(r"^mcp\.([^.]+)\.(.+)$", frame.title?).map(|(_, server, tool)| {
            ToolName::Mcp {
                server: server.to_owned(),
                tool: tool.to_owned(),
            }
        })
    }

    fn is_subagent(&self, _name: &ToolName, frame: &ToolFrame<'_>) -> bool {
        namespaced::<CodexMeta>(frame.meta, NAMESPACE)
            .and_then(|meta| meta.collaboration)
            .is_some_and(|collaboration| collaboration.tool == "spawnAgent")
    }

    fn subagent_input(&self, frame: &ToolFrame<'_>) -> SubagentInput {
        SubagentInput {
            prompt: raw::<SpawnAgentInput>(frame.raw_input).and_then(|input| input.prompt),
            ..SubagentInput::default()
        }
    }

    fn subagent_result(&self, frame: &ToolFrame<'_>) -> Option<SubagentResult> {
        let Some(input) = raw::<SpawnAgentInput>(frame.raw_input) else {
            return generic::subagent_result(frame);
        };
        let child = input.receiver_thread_ids.first().cloned();
        let message = child
            .as_ref()
            .and_then(|child| input.agents_states.get(child))
            .and_then(|state| state.message.clone());
        let failed = input.status.as_deref() == Some("failed");
        let result = SubagentResult {
            text: if failed { None } else { message.clone() },
            error: failed.then(|| message.unwrap_or_else(|| "subagent failed".to_owned())),
            agent_id: child,
            model: input.model.filter(|model| !model.is_empty()),
            ..SubagentResult::default()
        };
        (!result.is_empty()).then_some(result)
    }

    fn custom_answer_for(&self, name: &str, property: &Value) -> Option<String> {
        let own_marker = || {
            let marker: OtherAnswerMarker =
                serde_json::from_value(property.get("_meta")?.get(NAMESPACE)?.clone()).ok()?;
            marker.is_other_answer.then_some(marker.question_id)
        };
        own_marker()
            .or_else(|| generic::custom_answer_for(name, property))
            .or_else(|| generic::custom_answer_by_suffix(name, property, OTHER_ANSWER_SUFFIX))
    }
}
