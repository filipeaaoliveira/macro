//! Standing Rules: which Gates the factory answers on its own, with a Rule
//! Answer, instead of waiting for a person. A team's Orchestrator
//! Definition says, for each kind of Gate, whether its rule decides it; a
//! team may give one Project (an initiative) a definition of its own. A Run
//! keeps the definition it was created with. Its owner section says what
//! happens to the Gates no rule answers: with it on, they go to the Run's
//! owner (`owner_gates`). Its Queue limit says how many Gates and questions
//! may wait on a person before no new Run of theirs starts (`queue_limit`).

#[cfg(test)]
mod test;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A kind of Gate a Standing Rule may decide, by the entry that asks it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RuleGateKind {
    /// The plan Gate: approve the plan, or send it back.
    Plan,
    /// The Blockers Gate of a review panel: send a round's Blockers to be
    /// fixed and reviewed again, or stop for a person.
    Blockers,
    /// The merge Gate: merge the commit whose checks passed, or send it
    /// back.
    Merge,
}

impl RuleGateKind {
    /// The rule that decides a Gate of this kind.
    pub fn rule(self) -> StandingRule {
        match self {
            Self::Plan => StandingRule::ApprovePlan,
            Self::Blockers => StandingRule::FixBlockers,
            Self::Merge => StandingRule::MergeWhenGreen,
        }
    }
}

/// A Standing Rule: the answer the factory gives a Gate of one kind on its
/// own, when what the rule requires holds.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum StandingRule {
    /// Approve the plan Gate.
    ApprovePlan,
    /// Send a review round's Blockers to be fixed, for at most
    /// `MAX_RULE_FIX_ROUNDS` fix rounds in a row.
    FixBlockers,
    /// Merge once CI is green, when the Run is not Tampered and its base
    /// branch has no Main Hold.
    MergeWhenGreen,
}

/// The name the owner section is stored under, beside the Gate kinds.
pub const OWNER_GATES: &str = "owner_gates";

/// The highest Queue limit a definition may set.
pub const MAX_QUEUE_LIMIT: u16 = 1000;

/// A workspace's Orchestrator Definition: which kinds of Gate the factory
/// decides on its own, whether the Gates it leaves go to the Run's owner,
/// which the stand-in answers for the Run's owner when no rule does, and
/// the Queue limit above which no new Run starts. A kind left off waits for
/// a person, as every Gate did before Standing
/// Rules. Whatever it says, a Tampered Run's Gates and a merge through a
/// Main Hold stay a person's.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorDefinition {
    /// Approve the plan Gate (`approve_plan`).
    pub plan: bool,
    /// Send a review round's Blockers to be fixed (`fix_blockers`), for at
    /// most two fix rounds in a row; Blockers found after those go to a
    /// person.
    pub blockers: bool,
    /// Merge once CI is green, nothing is Tampered and the base branch has
    /// no Main Hold (`merge_when_green`).
    pub merge: bool,
    /// The owner section: a Gate no rule answers goes to the Run's owner.
    /// They are notified, and the Gate is listed as waiting on them; a
    /// review round whose Blockers are the round before's, unchanged after
    /// a rule sent those to a fix, waits for them rather than another fix;
    /// and what they comment on the Run's task once a Gate opened reaches
    /// the next agent Step. Off, the factory leaves those Gates waiting as
    /// before. A definition sent without it has it off.
    #[serde(default)]
    pub owner_gates: bool,
    /// The Queue limit: the most Gates and questions that may wait on a
    /// person (their Queue) for a new Run of theirs to start under this
    /// definition. Above it, no new Run starts; Runs already going go on.
    /// None, and a definition sent without it, sets no limit. At most
    /// [`MAX_QUEUE_LIMIT`]. A Run does not keep it: it decides only
    /// whether a Run starts, so a Run's rules read with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_limit: Option<u16>,
    /// The kinds of Gate the stand-in answers for the Run's owner, after
    /// the rules and before the person; off for every kind when left out.
    #[serde(default)]
    pub stand_in: StandInKinds,
    /// The kinds of Gate the Orchestrator, the owner's agent in Macro chat,
    /// may give the approving answer to for them (`approve`, `fix`); off
    /// for every kind when left out. It may always send work back, and
    /// never approves a merge.
    #[serde(default)]
    pub orchestrator: OrchestratorKinds,
}

/// The kinds of Gate the Orchestrator may give the approving answer to for
/// the Run's owner. It never approves a merge, so it has no setting for
/// one.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorKinds {
    /// Approve the plan Gate.
    pub plan: bool,
    /// Send a review round's Blockers to be fixed.
    pub blockers: bool,
}

/// The kinds of Gate the stand-in answers for the Run's owner: a small
/// model that answers as the owner would, which the owner may override for
/// a short while. It never answers a merge Gate, so it has no setting for
/// one.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StandInKinds {
    /// Answer the plan Gate.
    pub plan: bool,
    /// Answer a review round's Blockers Gate.
    pub blockers: bool,
}

/// How a definition keeps that the stand-in answers the plan Gate, beside
/// the kinds its rules decide.
const STAND_IN_PLAN: &str = "stand_in_plan";

/// How a definition keeps that the stand-in answers the Blockers Gate.
const STAND_IN_BLOCKERS: &str = "stand_in_blockers";

/// How a definition keeps that the Orchestrator may approve the plan Gate.
const ORCHESTRATOR_PLAN: &str = "orchestrator_plan";

/// How a definition keeps that the Orchestrator may send Blockers to a fix.
const ORCHESTRATOR_BLOCKERS: &str = "orchestrator_blockers";

impl OrchestratorDefinition {
    /// The definition that applies when a team set none: the review's
    /// Blockers are fixed on their own, as they were before Standing Rules,
    /// and the plan and the merge wait for a person.
    pub const DEFAULT: Self = Self {
        plan: false,
        blockers: true,
        merge: false,
        owner_gates: false,
        queue_limit: None,
        stand_in: StandInKinds {
            plan: false,
            blockers: false,
        },
        orchestrator: OrchestratorKinds {
            plan: false,
            blockers: false,
        },
    };

    /// Whether the rule of `kind` decides its Gates.
    pub fn decides(self, kind: RuleGateKind) -> bool {
        match kind {
            RuleGateKind::Plan => self.plan,
            RuleGateKind::Blockers => self.blockers,
            RuleGateKind::Merge => self.merge,
        }
    }

    /// Whether the stand-in answers Gates of `kind` for the Run's owner.
    /// It never answers a merge Gate.
    pub fn stands_in(self, kind: RuleGateKind) -> bool {
        match kind {
            RuleGateKind::Plan => self.stand_in.plan,
            RuleGateKind::Blockers => self.stand_in.blockers,
            RuleGateKind::Merge => false,
        }
    }

    /// Whether the Orchestrator may give a Gate of `kind` its approving
    /// answer for the Run's owner. It never approves a merge.
    pub fn orchestrator_approves(self, kind: RuleGateKind) -> bool {
        match kind {
            RuleGateKind::Plan => self.orchestrator.plan,
            RuleGateKind::Blockers => self.orchestrator.blockers,
            RuleGateKind::Merge => false,
        }
    }

    /// The kinds its rules decide, then [`OWNER_GATES`] when its owner
    /// section is on, then the kinds the stand-in answers
    /// (`stand_in_plan`, `stand_in_blockers`), then those the Orchestrator
    /// may approve (`orchestrator_plan`, `orchestrator_blockers`): what is
    /// stored of it beside
    /// its Queue limit, which has a column of its own.
    pub fn kinds(self) -> Vec<String> {
        let ruled = [
            RuleGateKind::Plan,
            RuleGateKind::Blockers,
            RuleGateKind::Merge,
        ]
        .into_iter()
        .filter(|kind| self.decides(*kind))
        .map(|kind| kind.to_string());
        let owner = self.owner_gates.then(|| OWNER_GATES.to_string());
        let stood_in = [
            (self.stand_in.plan, STAND_IN_PLAN),
            (self.stand_in.blockers, STAND_IN_BLOCKERS),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, kind)| kind.to_string());
        let orchestrated = [
            (self.orchestrator.plan, ORCHESTRATOR_PLAN),
            (self.orchestrator.blockers, ORCHESTRATOR_BLOCKERS),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, kind)| kind.to_string());
        ruled
            .chain(owner)
            .chain(stood_in)
            .chain(orchestrated)
            .collect()
    }

    /// The definition stored as `kinds` ([`Self::kinds`]), with no Queue
    /// limit; none when one is not a name this code knows.
    pub fn from_kinds(kinds: &[String]) -> Option<Self> {
        let mut definition = Self::default();
        for kind in kinds {
            match kind.as_str() {
                OWNER_GATES => definition.owner_gates = true,
                STAND_IN_PLAN => definition.stand_in.plan = true,
                STAND_IN_BLOCKERS => definition.stand_in.blockers = true,
                ORCHESTRATOR_PLAN => definition.orchestrator.plan = true,
                ORCHESTRATOR_BLOCKERS => definition.orchestrator.blockers = true,
                _ => match kind.parse::<RuleGateKind>().ok()? {
                    RuleGateKind::Plan => definition.plan = true,
                    RuleGateKind::Blockers => definition.blockers = true,
                    RuleGateKind::Merge => definition.merge = true,
                },
            }
        }
        Some(definition)
    }
}

/// Where a Run's Standing Rules came from.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RulesSource {
    /// The factory's default: the team set none, or its owner is on no
    /// team.
    Default,
    /// Its owner's team's.
    Team,
    /// Its owner's team's for the Project its task was in.
    Project,
}

/// The Standing Rules a Run keeps from when it was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRules {
    /// Which kinds of Gate its rules decide.
    pub definition: OrchestratorDefinition,
    /// Where they came from.
    pub source: RulesSource,
    /// The Project (an initiative) they were set for, when they are a
    /// Project's.
    pub project_id: Option<Uuid>,
}

impl RunRules {
    /// The factory's default rules, for a Run whose owner's team set none.
    pub const DEFAULT: Self = Self {
        definition: OrchestratorDefinition::DEFAULT,
        source: RulesSource::Default,
        project_id: None,
    };
}

/// A Project's own Standing Rules, which its team set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRules {
    /// The Project: an initiative's id.
    pub project_id: Uuid,
    /// Which kinds of Gate its rules decide.
    pub definition: OrchestratorDefinition,
}

/// Whether a Project's Standing Rules were kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRulesSet {
    /// They are kept, or were removed.
    Set,
    /// No initiative has the Project's id.
    NoSuchProject,
}

/// Where teams' Standing Rules are kept.
pub trait StandingRulesRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The Orchestrator Definition the team `team_id` set; none when it set
    /// none, so the factory's default applies.
    fn team_rules(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Option<OrchestratorDefinition>, Self::Err>> + Send;

    /// Set the team `team_id`'s Orchestrator Definition; none goes back to
    /// the factory's default. Its Control Files are left as they are.
    fn set_team_rules(
        &self,
        team_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Orchestrator Definition the team `team_id` set for the Project
    /// `project_id`; none when it set none.
    fn project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
    ) -> impl Future<Output = Result<Option<OrchestratorDefinition>, Self::Err>> + Send;

    /// Every Project the team `team_id` set rules for, by id.
    fn list_project_rules(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Vec<ProjectRules>, Self::Err>> + Send;

    /// Set the team `team_id`'s Orchestrator Definition for the Project
    /// `project_id`; none removes it, so the team's own applies.
    fn set_project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> impl Future<Output = Result<ProjectRulesSet, Self::Err>> + Send;
}
