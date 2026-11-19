//! The in-memory stand-in for agent sessions' bots and Runs' standing acts.

use std::collections::BTreeMap;

use chrono::Utc;
use factory_records::domain::models::RunId;
use factory_records::domain::standing_acts::{
    StandingAct, StandingActRecord, StandingActRepository, StandingActor,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeError, FakeRepo};

impl FakeRepo {
    /// Make `session_id` a session of `owner` run by `bot_id`.
    pub(crate) fn add_session_bot(&self, session_id: Uuid, owner: &str, bot_id: Uuid) {
        self.session_bots
            .lock()
            .expect("session bots")
            .insert(session_id, (owner.to_string(), bot_id));
    }
}

impl StandingActRepository for FakeRepo {
    type Err = FakeError;

    async fn session_bot(
        &self,
        session_id: Uuid,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<Uuid>, FakeError> {
        let bots = self.session_bots.lock().expect("session bots");
        Ok(bots
            .get(&session_id)
            .filter(|(session_owner, _)| session_owner == owner.as_ref())
            .map(|(_, bot)| *bot))
    }

    async fn record_standing_act(
        &self,
        run_id: RunId,
        act: StandingAct,
        seq: Option<u16>,
        actor: &StandingActor,
    ) -> Result<(), FakeError> {
        if self.fails("record_standing_act") {
            return Err(FakeError);
        }
        let mut acts = self.standing_acts.lock().expect("standing acts");
        // As the records keep it: one start, cancel and answer per Gate.
        let kept = acts
            .iter()
            .any(|(run, record)| *run == run_id && record.act == act && record.seq == seq);
        if kept && act != StandingAct::Link {
            return Ok(());
        }
        acts.push((
            run_id,
            StandingActRecord {
                act,
                seq,
                bot_id: actor.bot_id,
                agent: actor.agent.clone(),
                standing_role: actor.standing_role.clone(),
                session_id: actor.session_id,
                at: Utc::now(),
            },
        ));
        Ok(())
    }

    async fn runs_standing_acts(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<StandingActRecord>>, FakeError> {
        let acts = self.standing_acts.lock().expect("standing acts");
        let mut read: BTreeMap<RunId, Vec<StandingActRecord>> = BTreeMap::new();
        for (run, record) in acts.iter().filter(|(run, _)| ids.contains(run)) {
            read.entry(*run).or_default().push(record.clone());
        }
        Ok(read)
    }
}
