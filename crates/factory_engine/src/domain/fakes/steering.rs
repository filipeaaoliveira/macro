//! The Step Terminal's records in memory: each Step's steering, by the
//! same rules as the Postgres repository.

use chrono::{DateTime, Utc};
use factory_records::domain::models::{
    NewSteer, Steer, SteerRefusal, Steered, StepKey, StepSteering, StepTerminal,
};
use uuid::Uuid;

use super::{FakeError, FakeRepo};

/// One Step's steering.
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeSteering {
    pub(crate) paused_at: Option<DateTime<Utc>>,
    pub(crate) stop_asked_at: Option<DateTime<Utc>>,
    pub(crate) prompt: Option<Uuid>,
    pub(crate) closed: bool,
    pub(crate) pause_ran_out_at: Option<DateTime<Utc>>,
    /// What the owner did, in order, with the prompt each sent.
    pub(crate) steers: Vec<(Steer, Option<Uuid>)>,
}

impl FakeRepo {
    fn with_steering<T>(&self, key: StepKey, change: impl FnOnce(&mut FakeSteering) -> T) -> T {
        let mut steering = self.steering.lock().expect("steering");
        change(steering.entry(key).or_default())
    }

    /// The Step `key`'s terminal as the Run view reads it.
    pub(crate) fn terminal_of(&self, key: StepKey, ended: bool) -> StepTerminal {
        let session_id = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .find(|claimed| claimed.key == key && !claimed.released && !ended)
            .map(|claimed| claimed.session.session_id);
        self.with_steering(key, |steering| StepTerminal {
            session_id,
            paused_at: steering.paused_at,
            stop_asked_at: steering.stop_asked_at,
            between_turns: steering.closed && !ended,
            ends_by: None,
            pause_ran_out_at: steering.pause_ran_out_at,
            steers: steering
                .steers
                .iter()
                .map(|(steer, _)| steer.clone())
                .collect(),
        })
    }

    /// A later prompt of the factory's: the owner may steer the Step again,
    /// and it follows this prompt.
    pub(crate) fn open_steering(&self, key: StepKey) {
        self.with_steering(key, |steering| {
            steering.prompt = None;
            steering.closed = false;
        });
    }

    pub(crate) fn record_steer(
        &self,
        key: StepKey,
        steer: &NewSteer,
    ) -> Result<Result<Steered, SteerRefusal>, FakeError> {
        let run_ended = self
            .runs
            .lock()
            .expect("runs")
            .get(&key.run_id)
            .is_none_or(|run| run.end.is_some());
        let Some((ended, role)) =
            self.with_step(key, |step| (step.end.is_some(), step.role.clone()))
        else {
            return Ok(Err(SteerRefusal::Ended));
        };
        let claimed = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .find(|claimed| claimed.key == key)
            .map(|claimed| (claimed.session.clone(), claimed.released));
        if ended || run_ended || claimed.as_ref().is_some_and(|(_, released)| *released) {
            return Ok(Err(SteerRefusal::Ended));
        }
        let (Some((session, _)), Some(role)) = (claimed, role) else {
            return Ok(Err(SteerRefusal::NoSession));
        };
        Ok(self.with_steering(key, |steering| {
            let paused = steering.paused_at.is_some();
            let refusal = if steering.stop_asked_at.is_some() {
                Some(SteerRefusal::Stopping)
            } else {
                match steer {
                    NewSteer::Note { .. } | NewSteer::Pause if paused => Some(SteerRefusal::Paused),
                    NewSteer::Resume { .. } if !paused => Some(SteerRefusal::NotPaused),
                    _ if steering.closed => Some(SteerRefusal::NotWorking),
                    _ => None,
                }
            };
            if let Some(refusal) = refusal {
                return Err(refusal);
            }
            let previous_prompt = steering.prompt;
            let now = Utc::now();
            let note = match steer {
                NewSteer::Note { note, .. } => Some(note.clone()),
                NewSteer::Resume { note, .. } => note.clone(),
                NewSteer::Pause => {
                    steering.paused_at = Some(now);
                    None
                }
                NewSteer::Stop => {
                    steering.stop_asked_at = Some(now);
                    None
                }
            };
            if matches!(steer, NewSteer::Resume { .. }) {
                steering.paused_at = None;
            }
            if let Some(prompt) = steer.prompt_id() {
                steering.prompt = Some(prompt);
            }
            steering.steers.push((
                Steer {
                    action: steer.action(),
                    note,
                    undelivered: false,
                    at: now,
                },
                steer.prompt_id(),
            ));
            Ok(Steered {
                session,
                role,
                previous_prompt,
            })
        }))
    }

    pub(crate) fn undo_steer(&self, key: StepKey, prompt_id: Uuid, previous: Option<Uuid>) {
        let ended = self.step_ended(key);
        self.with_steering(key, |steering| {
            let mut resumed = false;
            for (steer, prompt) in &mut steering.steers {
                if *prompt == Some(prompt_id) {
                    steer.undelivered = true;
                    resumed = steer.action == factory_records::domain::models::SteerAction::Resume;
                }
            }
            if steering.prompt == Some(prompt_id) && !ended {
                steering.prompt = previous;
                if resumed && steering.paused_at.is_none() {
                    steering.paused_at = Some(Utc::now());
                }
            }
        });
    }

    pub(crate) fn read_steering(&self, key: StepKey) -> StepSteering {
        let started_at = self.with_step(key, |step| step.started_at);
        self.with_steering(key, |steering| StepSteering {
            paused: steering.paused_at.is_some(),
            stop_asked: steering.stop_asked_at.is_some(),
            prompt: steering.prompt,
            started_at,
            pause_ran_out: steering.pause_ran_out_at.is_some(),
        })
    }

    pub(crate) fn record_pause_ran_out(&self, key: StepKey) {
        self.with_steering(key, |steering| {
            if steering.paused_at.is_some() && steering.pause_ran_out_at.is_none() {
                steering.pause_ran_out_at = Some(Utc::now());
            }
        });
    }

    pub(crate) fn stop_steering(&self, key: StepKey, turn: Uuid, followed: Option<Uuid>) -> bool {
        // The wait on a prompt the Step no longer works on closes nothing.
        let current = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .find(|claimed| claimed.key == key)
            .map(|claimed| claimed.turn.unwrap_or(claimed.session.action_id));
        if current.is_some_and(|current| current != turn) {
            return true;
        }
        self.with_steering(key, |steering| {
            let closes = steering.prompt == followed
                && steering.paused_at.is_none()
                && steering.stop_asked_at.is_none();
            if closes {
                steering.closed = true;
            }
            closes
        })
    }
}
