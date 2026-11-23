//! Which agent turns are running, read off the ACP wire.
//!
//! A turn is a `session/prompt` request the agent has not answered yet. The
//! self-updater restarts macrod only while none is outstanding.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use agent_client_protocol::LineDirection;

#[cfg(test)]
mod test;

/// The ids of prompt requests sent to the ACP agent and not yet answered,
/// and where each session works.
#[derive(Clone, Default)]
pub struct ActiveTurns {
    prompt_ids: Arc<Mutex<HashSet<String>>>,
    workplaces: Arc<Mutex<Workplaces>>,
}

/// Which directory each session works in, read off `session/new`,
/// `session/load` and `session/resume`, and which of them have a turn
/// running or had one start or end lately: what keeps a branch's worktree
/// (`crate::branch_worktrees`) from being removed under its session.
#[derive(Default)]
struct Workplaces {
    /// `session/new` requests not answered yet, by id: their `cwd`.
    opening: HashMap<String, String>,
    /// Each session's `cwd`, by the agent's session id.
    cwd: HashMap<String, String>,
    /// Prompts not answered yet, by request id: their session.
    prompts: HashMap<String, String>,
    /// When a session last opened, or a turn last started or ended, in
    /// each `cwd`.
    active: HashMap<String, SystemTime>,
}

impl Workplaces {
    fn touch_session(&mut self, session: &str) {
        if let Some(cwd) = self.cwd.get(session) {
            self.active.insert(cwd.clone(), SystemTime::now());
        }
    }

    fn opened(&mut self, session: String, cwd: String) {
        self.active.insert(cwd.clone(), SystemTime::now());
        self.cwd.insert(session, cwd);
    }
}

/// The JSON-RPC fields that say whether a line starts or ends a turn.
#[derive(serde::Deserialize)]
struct RpcLine {
    id: Option<serde_json::Value>,
    method: Option<String>,
    #[serde(default)]
    params: Option<RpcParams>,
    #[serde(default)]
    result: Option<RpcParams>,
}

/// The parameters (or result) fields that say where a session works.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcParams {
    session_id: Option<String>,
    cwd: Option<String>,
}

impl ActiveTurns {
    /// Record a line crossing the agent's stdio: a prompt request starts a
    /// turn, the response with the same id ends it. A session's opening
    /// says where it works.
    pub fn observe(&self, line: &str, direction: LineDirection) {
        match direction {
            LineDirection::Stdin => {
                if !line.contains("session/") {
                    return;
                }
                let Ok(RpcLine {
                    id: Some(id),
                    method: Some(method),
                    params,
                    ..
                }) = serde_json::from_str(line)
                else {
                    return;
                };
                let id = id.to_string();
                let mut workplaces = self.workplaces();
                match (method.as_str(), params) {
                    ("session/prompt", params) => {
                        if let Some(session) = params.and_then(|params| params.session_id) {
                            workplaces.touch_session(&session);
                            workplaces.prompts.insert(id.clone(), session);
                        }
                        drop(workplaces);
                        self.prompt_ids
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(id);
                    }
                    ("session/new", Some(RpcParams { cwd: Some(cwd), .. })) => {
                        workplaces.opening.insert(id, cwd);
                    }
                    (
                        "session/load" | "session/resume",
                        Some(RpcParams {
                            session_id: Some(session),
                            cwd: Some(cwd),
                        }),
                    ) => workplaces.opened(session, cwd),
                    _ => {}
                }
            }
            LineDirection::Stdout => {
                let waiting = !self.is_idle() || {
                    let workplaces = self.workplaces();
                    !workplaces.opening.is_empty() || !workplaces.prompts.is_empty()
                };
                if !waiting {
                    return;
                }
                let Ok(RpcLine {
                    id: Some(id),
                    method: None,
                    result,
                    ..
                }) = serde_json::from_str(line)
                else {
                    return;
                };
                let id = id.to_string();
                self.prompt_ids
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                let mut workplaces = self.workplaces();
                if let Some(cwd) = workplaces.opening.remove(&id)
                    && let Some(session) = result.and_then(|result| result.session_id)
                {
                    workplaces.opened(session, cwd);
                }
                if let Some(session) = workplaces.prompts.remove(&id) {
                    workplaces.touch_session(&session);
                }
            }
            LineDirection::Stderr => {}
        }
    }

    fn workplaces(&self) -> std::sync::MutexGuard<'_, Workplaces> {
        self.workplaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a session working in `cwd` has a turn running, and when a
    /// session there last opened, or a turn there last started or ended.
    pub fn workplace(&self, cwd: &str) -> (bool, Option<SystemTime>) {
        let workplaces = self.workplaces();
        let running = workplaces
            .prompts
            .values()
            .any(|session| workplaces.cwd.get(session).is_some_and(|at| at == cwd));
        (running, workplaces.active.get(cwd).copied())
    }

    /// The agent process ended: none of its turns can still be running.
    pub fn clear(&self) {
        self.prompt_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let mut workplaces = self.workplaces();
        workplaces.opening.clear();
        workplaces.prompts.clear();
    }

    /// The prompt ids still running, to hand to the next process image.
    pub fn snapshot(&self) -> Vec<String> {
        self.prompt_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Continue tracking turns an earlier process image saw start.
    pub fn restore(&self, prompt_ids: impl IntoIterator<Item = String>) {
        self.prompt_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(prompt_ids);
    }

    /// Whether no turn is running.
    pub fn is_idle(&self) -> bool {
        self.prompt_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}
