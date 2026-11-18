//! Standing Roles on the acts an agent does for a Run's owner from its
//! code: which bot may start, cancel or link a Run, or answer its plan or
//! Blockers Gate, by the Standing Roles of the Run's Factory Definition.

use bot_id::BotId;
use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::RunId;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::standing_acts::{StandingAct, StandingActRepository, StandingActor};
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FactoryServiceImpl, internal};
use crate::domain::agents::{AgentDefinition, StandingCheck};
use crate::domain::models::FactoryError;
use crate::domain::ports::{
    PullRequests, RunWorkflows, StepExecutor, TaskDirectory, UserDirectory,
};

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + StandingActRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// The Agents of the Factory Definition the Run `run_id` started
    /// under, as it kept them, with their Standing Roles; none when it
    /// started under none.
    pub(super) async fn kept_agents(
        &self,
        run_id: RunId,
    ) -> Result<Option<AgentDefinition>, FactoryError> {
        let Some(kept) = self
            .repo
            .run_definition_agents(run_id)
            .await
            .map_err(internal)?
        else {
            return Ok(None);
        };
        let agents = serde_json::from_value(kept).map_err(internal)?;
        Ok(Some(agents))
    }

    /// Who does `act` for `actor` from the agent session `session`, when a
    /// Standing Role of `agents` lets its bot: none when the caller is no
    /// agent session, or the Definition has no Standing Roles, so the act
    /// goes on as before them. Fails when no Standing Role the session's
    /// bot fills allows the act, naming those that do; a session that is
    /// not `actor`'s fills none.
    pub(super) async fn standing_actor(
        &self,
        actor: &MacroUserIdStr<'static>,
        agents: Option<&AgentDefinition>,
        act: StandingAct,
        session: Option<Uuid>,
    ) -> Result<Option<StandingActor>, FactoryError> {
        let (Some(session_id), Some(agents)) = (session, agents) else {
            return Ok(None);
        };
        if !agents.has_standing_roles() {
            return Ok(None);
        }

        let bot = self
            .repo
            .session_bot(session_id, actor)
            .await
            .map_err(internal)?;
        match agents.standing_check(act, bot.map(BotId::new_from_uuid)) {
            StandingCheck::Open => Ok(None),
            StandingCheck::Filled { role, agent } => Ok(bot.map(|bot_id| StandingActor {
                session_id,
                bot_id,
                agent,
                standing_role: role,
            })),
            StandingCheck::Refused { allowing } => {
                Err(FactoryError::NotInStandingRole { act, allowing })
            }
        }
    }
}
