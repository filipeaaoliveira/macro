use std::hash::Hash;
use std::time::{Duration, Instant};

use agent_session::domain::model::AgentSessionId;
use dashmap::DashMap;

#[cfg(test)]
mod test;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContainerState {
    Pending,
    Active { last_activity: Instant },
    Stopping { last_activity: Instant },
}

pub(crate) struct ManagedContainers<Id> {
    entries: DashMap<Id, ContainerState>,
    /// The session each container id was registered for, so a reaper -
    /// which only ever sees `Id` - can ask whether the harness has a
    /// command in flight for it.
    sessions: DashMap<Id, AgentSessionId>,
    /// How many commands run in each container now, beside its agent: a
    /// reaper leaves a container with one alone, however long it has been
    /// since its agent's last activity.
    commands: DashMap<Id, usize>,
    /// Containers tracked here only for the commands running in them: this
    /// replica holds no session in them, so it stops tracking them once the
    /// commands end, and never reaps a container another replica may hold.
    command_only: DashMap<Id, ()>,
}

impl<Id> ManagedContainers<Id>
where
    Id: Clone + Eq + Hash,
{
    pub(crate) fn new() -> Self {
        Self {
            entries: DashMap::new(),
            sessions: DashMap::new(),
            commands: DashMap::new(),
            command_only: DashMap::new(),
        }
    }

    pub(crate) fn register(&self, id: Id, session: AgentSessionId) {
        self.command_only.remove(&id);
        self.entries.insert(id.clone(), ContainerState::Pending);
        self.sessions.insert(id, session);
    }

    /// The session `id` was registered for, if it is still tracked.
    pub(crate) fn session_of(&self, id: &Id) -> Option<AgentSessionId> {
        self.sessions.get(id).map(|entry| *entry)
    }

    pub(crate) fn activate(&self, id: &Id, now: Instant) -> bool {
        let Some(mut entry) = self.entries.get_mut(id) else {
            return false;
        };
        *entry = ContainerState::Active { last_activity: now };
        true
    }

    pub(crate) fn record_activity(&self, id: &Id, now: Instant) {
        if let Some(mut entry) = self.entries.get_mut(id) {
            match &mut *entry {
                ContainerState::Pending => {}
                ContainerState::Active { last_activity }
                | ContainerState::Stopping { last_activity } => *last_activity = now,
            }
        }
    }

    /// Count a command that starts running in `id`, the container of
    /// `session`, now. One this manager does not track (its session is
    /// attached to no transport here) is tracked only while its commands
    /// run.
    pub(crate) fn begin_command(&self, id: &Id, session: AgentSessionId, now: Instant) {
        *self.commands.entry(id.clone()).or_insert(0) += 1;
        if let dashmap::mapref::entry::Entry::Vacant(vacant) = self.entries.entry(id.clone()) {
            vacant.insert(ContainerState::Active { last_activity: now });
            self.command_only.insert(id.clone(), ());
            self.sessions.entry(id.clone()).or_insert(session);
        }
    }

    /// Count a command in `id` as ended now: the container's idle time
    /// starts again from its end, and one tracked only for its commands is
    /// no longer tracked once the last ends.
    pub(crate) fn end_command(&self, id: &Id, now: Instant) {
        let ended = self
            .commands
            .remove_if_mut(id, |_, running| {
                *running = running.saturating_sub(1);
                *running == 0
            })
            .is_some();
        if ended && self.command_only.remove(id).is_some() {
            self.entries.remove(id);
            self.sessions.remove(id);
            return;
        }
        self.record_activity(id, now);
    }

    fn runs_command(&self, id: &Id) -> bool {
        self.commands.contains_key(id)
    }

    pub(crate) fn remove(&self, id: &Id) -> bool {
        self.sessions.remove(id);
        self.entries.remove(id).is_some()
    }

    /// Stop-candidate ids idle at least `max_idle`, excluding any `skip`
    /// answers true for.
    ///
    /// `skip` is checked before a candidate is claimed as stale: a session
    /// with a command already admitted but not yet turn-active - the harness
    /// marks this at admission, before the runtime visibly starts a turn -
    /// looks idle by activity timestamp alone, and reaping it now would only
    /// race that command's delivery.
    pub(crate) fn reap_stale(
        &self,
        now: Instant,
        max_idle: Duration,
        skip: impl Fn(&Id) -> bool,
    ) -> Vec<Id> {
        let candidates = self
            .entries
            .iter()
            .filter_map(|entry| match entry.value() {
                ContainerState::Active { last_activity }
                    if now.saturating_duration_since(*last_activity) >= max_idle
                        && !self.runs_command(entry.key())
                        && !skip(entry.key()) =>
                {
                    Some(entry.key().clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut stale = Vec::new();
        for id in candidates {
            let Some(mut state) = self.entries.get_mut(&id) else {
                continue;
            };
            if let ContainerState::Active { last_activity } = *state
                && now.saturating_duration_since(last_activity) >= max_idle
                && !self.runs_command(&id)
                && !skip(&id)
            {
                *state = ContainerState::Stopping { last_activity };
                stale.push(id);
            }
        }
        stale
    }

    pub(crate) fn restore_failed_stop(&self, id: Id, now: Instant, max_idle: Duration) {
        let last_activity = now.checked_sub(max_idle).unwrap_or(now);
        self.entries
            .entry(id)
            .or_insert(ContainerState::Active { last_activity });
    }

    pub(crate) fn finish_stop(&self, id: &Id, stopped: bool) {
        if stopped {
            self.entries.remove_if(id, |_, state| {
                matches!(state, ContainerState::Stopping { .. })
            });
        } else if let Some(mut state) = self.entries.get_mut(id)
            && let ContainerState::Stopping { last_activity } = *state
        {
            *state = ContainerState::Active { last_activity };
        }
    }

    pub(crate) fn drain(&self) -> Vec<Id> {
        let ids = self
            .entries
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.entries.remove(&id).map(|(id, _)| id))
            .collect()
    }
}
