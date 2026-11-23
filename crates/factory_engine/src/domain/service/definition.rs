//! The Run API's side of the Factory Definition: reading a team's at the
//! head of its repository's default branch, as the person who asks, and
//! the Flow a Run of it starts with, the text of one of its files, and
//! changes to it checked or proposed as pull requests (`change`).

mod change;

use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::standing_acts::StandingActRepository;
use std::collections::BTreeMap;

use factory_records::domain::models::{
    DefinitionRead, NewRunDefinition, RunFlow, RunId, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::self_improvement::SelfImprovementRepository;
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;

use super::{FactoryServiceImpl, internal};
use crate::domain::built_in_flows::{BuiltInFlow, walked_flow};
use crate::domain::definition::{
    DefinitionError, DefinitionFiles, DefinitionReadError, FactoryDefinition, is_definition_file,
    read_definition,
};
use crate::domain::flow::Flow;
use crate::domain::models::{
    CodeRepository, DEFINITION_PULL_REQUESTS_LISTED, DefinitionContents, DefinitionFile,
    DefinitionFlow, DefinitionPullRequest, DefinitionRole, DefinitionScorer, DefinitionSettings,
    FactoryError, RepositoryAccess,
};
use crate::domain::ports::{
    FileRefusal, PullRequests, RunWorkflows, StepExecutor, TaskDirectory, UserDirectory,
};

/// A Factory Definition read at one commit of its repository: what a Run
/// started under it keeps.
#[derive(Debug, Clone)]
pub(super) struct PinnedDefinition {
    /// The repository's `https://github.com/owner/name` URL.
    pub(super) repo_url: String,
    /// The default branch it was read from.
    pub(super) branch: String,
    /// The commit it was read at.
    pub(super) commit: String,
    /// What it holds.
    pub(super) definition: FactoryDefinition,
    /// Its Agents, as a Run keeps them.
    pub(super) agents: serde_json::Value,
    /// Its Scorers, Experiments and Benchmarks, as a Run keeps them.
    pub(super) evaluation: serde_json::Value,
}

impl PinnedDefinition {
    /// What a new Run keeps of it.
    pub(super) fn kept(&self) -> NewRunDefinition<'_> {
        NewRunDefinition {
            repo_url: &self.repo_url,
            commit: &self.commit,
            agents: &self.agents,
            evaluation: &self.evaluation,
        }
    }
}

/// The files of a Definition's repository at one commit, read on GitHub as
/// one person.
struct RepositoryFiles<'a, G> {
    github: &'a G,
    reader: &'a MacroUserIdStr<'static>,
    repository: &'a CodeRepository,
    commit: &'a str,
}

/// Why a file of a Definition's repository was not read.
#[derive(Debug, thiserror::Error)]
enum RepositoryFileError<E: std::error::Error + 'static> {
    /// GitHub could not be asked; a retry may read it.
    #[error(transparent)]
    Github(E),
    /// GitHub will not give the file, however often it is asked: the
    /// Definition's to fix.
    #[error("{path} was refused")]
    Refused {
        /// The file.
        path: String,
        /// Why.
        refusal: FileRefusal,
    },
}

impl<G: PullRequests> DefinitionFiles for RepositoryFiles<'_, G> {
    type Err = RepositoryFileError<G::Err>;

    async fn read(&self, path: &str) -> Result<Option<String>, Self::Err> {
        self.github
            .file_text(self.reader, self.repository, self.commit, path)
            .await
            .map_err(|error| match G::file_refusal(&error) {
                Some(refusal) => RepositoryFileError::Refused {
                    path: path.to_string(),
                    refusal,
                },
                None => RepositoryFileError::Github(error),
            })
    }
}

/// What a Definition's file GitHub will not give is, to the person who
/// fixes it: GitHub's own words name the request, not the file's problem.
fn refused_file(path: String, refusal: FileRefusal) -> DefinitionError {
    match refusal {
        FileRefusal::TooLarge => DefinitionError::TooLarge(path),
        FileRefusal::Refused => DefinitionError::Unreadable {
            path,
            error: "GitHub refused to give it".to_string(),
        },
    }
}

/// Where a Definition is now, as one person reaches it on GitHub.
struct DefinitionHead {
    /// Its repository.
    repository: CodeRepository,
    /// What the person may do with it.
    access: RepositoryAccess,
    /// The commit its default branch is at.
    commit: String,
}

/// What `definition`, read at `commit`, holds, as its setting shows it.
fn contents_of(commit: &str, definition: &FactoryDefinition) -> DefinitionContents {
    DefinitionContents {
        commit: commit.to_string(),
        description: definition.description.clone(),
        flows: definition
            .flows
            .iter()
            .map(|(name, flow)| DefinitionFlow {
                name: name.clone(),
                description: flow.description.clone(),
                entries: flow
                    .entries
                    .iter()
                    .map(|entry| entry.id.to_string())
                    .collect(),
            })
            .collect(),
        roles: definition
            .agents
            .roles()
            .map(|(role, agents)| DefinitionRole {
                role: role.to_string(),
                agents: agents.iter().map(ToString::to_string).collect(),
            })
            .collect(),
        standing_rules: definition.rules.standing_rules,
        scorers: definition
            .evaluation
            .scorers()
            .iter()
            .map(|scorer| DefinitionScorer {
                name: scorer.name.clone(),
                flows: scorer.flows.clone(),
                sample_percent: scorer.sample_percent,
            })
            .collect(),
    }
}

/// Whether `commit` is a full commit id, as GitHub names a branch's head.
fn is_commit_id(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + EvaluationRepository
        + StandingActRepository
        + SelfImprovementRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// The repository `team` names as its Factory Definition; none when it
    /// names none, or there is no team.
    pub(super) async fn named_definition(
        &self,
        team: Option<TeamMembership>,
    ) -> Result<Option<String>, FactoryError> {
        let Some(team) = team else {
            return Ok(None);
        };
        Ok(self
            .repo
            .team_definition(team.team_id)
            .await
            .map_err(internal)?)
    }

    /// The Factory Definition in `repo_url`, read and checked at the commit
    /// its default branch is at now, as `reader` reaches it on GitHub. A
    /// Run keeps that commit, so a change merged after it starts, or the
    /// Run's own branch, never changes what it runs under.
    pub(super) async fn definition_at_head(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Result<PinnedDefinition, FactoryError> {
        let DefinitionHead {
            repository,
            access,
            commit,
        } = self.definition_head(reader, repo_url).await?;
        let definition = self.definition_at(reader, &repository, &commit).await?;
        let agents = serde_json::to_value(&definition.agents)
            .map_err(|error| FactoryError::Internal(internal(error)))?;
        let evaluation = serde_json::to_value(&definition.evaluation)
            .map_err(|error| FactoryError::Internal(internal(error)))?;
        Ok(PinnedDefinition {
            repo_url: repository.url(),
            branch: access.default_branch,
            commit,
            definition,
            agents,
            evaluation,
        })
    }

    /// Where the Definition in `repo_url` is now, as `reader` reaches it on
    /// GitHub: its repository, what they may do with it, and the commit its
    /// default branch is at.
    async fn definition_head(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Result<DefinitionHead, FactoryError> {
        let repository = CodeRepository::parse(repo_url).ok_or(FactoryError::InvalidRepository)?;
        let access = self
            .github
            .repository_access(reader, &repository)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::UnreachableDefinition)?;
        // GitHub follows a renamed or transferred repository: the team
        // names the Definition where it is now.
        let asked = format!("{}/{}", repository.owner(), repository.name());
        if let Some(moved) = access
            .full_name
            .as_deref()
            .filter(|now| !now.eq_ignore_ascii_case(&asked))
        {
            return Err(FactoryError::InvalidDefinition(DefinitionError::Moved(
                moved.to_string(),
            )));
        }
        let commit = self
            .github
            .branch_head(reader, &repository, &access.default_branch)
            .await
            .map_err(internal)?
            .filter(|commit| is_commit_id(commit))
            .ok_or(FactoryError::UnreachableDefinition)?;
        Ok(DefinitionHead {
            repository,
            access,
            commit,
        })
    }

    /// The Factory Definition in `repository`, read and checked at
    /// `commit`, as `reader` reaches it on GitHub.
    async fn definition_at(
        &self,
        reader: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        commit: &str,
    ) -> Result<FactoryDefinition, FactoryError> {
        // The commit goes into GitHub's paths: only a full commit id does.
        if !is_commit_id(commit) {
            return Err(FactoryError::UnreachableDefinition);
        }
        let files = RepositoryFiles {
            github: &self.github,
            reader,
            repository,
            commit,
        };
        read_definition(&files, |step| self.executor.serves(step))
            .await
            .map_err(|error| match error {
                DefinitionReadError::Files(RepositoryFileError::Refused { path, refusal }) => {
                    FactoryError::InvalidDefinition(refused_file(path, refusal))
                }
                DefinitionReadError::Files(RepositoryFileError::Github(error)) => {
                    FactoryError::Internal(internal(error))
                }
                DefinitionReadError::Invalid(error) => FactoryError::InvalidDefinition(error),
            })
    }

    /// The Factory Definition `team` names, read at its default branch's
    /// head as `actor`; none when it names none, so the team's settings and
    /// the built-in Flows and Agents apply.
    pub(super) async fn team_definition(
        &self,
        actor: &MacroUserIdStr<'static>,
        team: Option<TeamMembership>,
    ) -> Result<Option<PinnedDefinition>, FactoryError> {
        match self.named_definition(team).await? {
            Some(repo_url) => Ok(Some(self.definition_at_head(actor, &repo_url).await?)),
            None => Ok(None),
        }
    }

    /// The Flow named `name` a Run starts with: `definition`'s when there
    /// is one, else the built-in Flow, as this service bounds it.
    pub(super) fn flow_named(
        &self,
        name: &str,
        definition: Option<&PinnedDefinition>,
    ) -> Result<Flow, FactoryError> {
        let mut flow = match definition {
            Some(pinned) => pinned
                .definition
                .flows
                .get(name)
                .cloned()
                .ok_or(FactoryError::UnknownFlow)?,
            None => BuiltInFlow::named(name)
                .ok_or(FactoryError::UnknownFlow)?
                .read(|step| self.executor.serves(step))?,
        };
        if let Some(rounds) = self.review_rounds {
            flow.bound_review_rounds(rounds);
        }
        flow.allow_questions(self.questions);
        Ok(flow)
    }

    /// The Flow the Run `run_id` walks: the one it kept when it started, or,
    /// for a Run kept before Runs kept theirs, the built-in Flow of its
    /// name, bound as this service bounds the Flows it starts. None when
    /// neither is there.
    pub(super) async fn run_flow(&self, run_id: RunId) -> Result<Option<Flow>, FactoryError> {
        let Some(kept) = self.repo.run_flow(run_id).await.map_err(internal)? else {
            return Ok(None);
        };
        Ok(self.bounded(run_id, kept))
    }

    /// The Flows the Runs `ids` walk, as [`Self::run_flow`] reads each, in
    /// one read: a Run whose Flow does not read is left out.
    pub(super) async fn runs_flows(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Flow>, FactoryError> {
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let kept = self.repo.runs_flows(ids).await.map_err(internal)?;
        Ok(kept
            .into_iter()
            .filter_map(|(id, kept)| Some((id, self.bounded(id, kept)?)))
            .collect())
    }

    /// The Flow `kept` says a Run walks, a built-in one bound as this
    /// service bounds the Flows it starts.
    fn bounded(&self, run_id: RunId, kept: RunFlow) -> Option<Flow> {
        let built_in = kept.definition.is_none();
        let mut flow = walked_flow(run_id, kept)?;
        if built_in && let Some(rounds) = self.review_rounds {
            flow.bound_review_rounds(rounds);
        }
        Some(flow)
    }

    /// The team's Factory Definition, as `actor` sees it: read at its
    /// head, saying why when it does not read.
    pub(super) async fn definition_settings(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<DefinitionSettings, FactoryError> {
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        let agent = self.repo.is_agent_account(actor).await.map_err(internal)?;
        let repo_url = self.named_definition(team).await?;
        let read = match &repo_url {
            Some(repo_url) => Some(self.definition_at_head(actor, repo_url).await),
            None => None,
        };
        self.settings_of(actor, team, !agent, repo_url, read).await
    }

    /// The settings of `team`'s Definition `repo_url`, from `read`, what
    /// reading it at its head came to; `editor` when the person is not an
    /// agent's account.
    async fn settings_of(
        &self,
        actor: &MacroUserIdStr<'static>,
        team: Option<TeamMembership>,
        editor: bool,
        repo_url: Option<String>,
        read: Option<Result<PinnedDefinition, FactoryError>>,
    ) -> Result<DefinitionSettings, FactoryError> {
        let (pinned, problem) = match read {
            Some(Ok(pinned)) => (Some(pinned), None),
            Some(Err(FactoryError::Internal(report))) => {
                return Err(FactoryError::Internal(report));
            }
            Some(Err(refused)) => (None, Some(refused.to_string())),
            None => (None, None),
        };
        // A change reaches the Definition by pull request only where its
        // default branch asks for one: say so when it does not. GitHub
        // failing to tell is not the settings' failure.
        let requires_pull_request = match &pinned {
            Some(pinned) => self.requires_pull_request(actor, pinned).await,
            None => None,
        };
        let (newest_run, contents, open_pull_requests) = match (&repo_url, team) {
            (Some(repo_url), Some(team)) => {
                let newest_run = self
                    .repo
                    .newest_definition_read(team.team_id, repo_url)
                    .await
                    .map_err(internal)?;
                let (contents, open) = tokio::join!(
                    self.shown_contents(actor, repo_url, newest_run.as_ref(), pinned.as_ref()),
                    self.definition_pull_requests(actor, repo_url),
                );
                (newest_run, contents, open)
            }
            _ => (None, None, None),
        };
        Ok(DefinitionSettings {
            repo_url,
            commit: pinned.map(|pinned| pinned.commit),
            problem,
            requires_pull_request,
            team_id: team.map(|team| team.team_id),
            can_edit: editor && team.is_some_and(|team| team.admin),
            newest_run,
            contents,
            open_pull_requests,
        })
    }

    /// What the setting shows the Definition `repo_url` holds: at its head
    /// (`head`), the commit the setting names, whenever it reads there, so
    /// the two never disagree once a change merged and no Run read it yet.
    /// When the head does not read, at the commit `newest_run` read, which
    /// the team's Runs under way still walk, if that commit reads as `actor`
    /// reaches it: GitHub down or limiting them, or the commit gone, is
    /// logged, never the settings' failure.
    async fn shown_contents(
        &self,
        actor: &MacroUserIdStr<'static>,
        repo_url: &str,
        newest_run: Option<&DefinitionRead>,
        head: Option<&PinnedDefinition>,
    ) -> Option<DefinitionContents> {
        if let Some(pinned) = head {
            return Some(contents_of(&pinned.commit, &pinned.definition));
        }
        let read = newest_run?;
        let repository = CodeRepository::parse(repo_url)?;
        match self.definition_at(actor, &repository, &read.commit).await {
            Ok(definition) => Some(contents_of(&read.commit, &definition)),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "the definition at the newest run's commit could not be read"
                );
                None
            }
        }
    }

    /// The file `path` of the Factory Definition `actor`'s team names, as
    /// it is at its default branch's head, read as `actor` reaches it on
    /// GitHub: what a change to it starts from. A file not there yet has
    /// no text.
    pub(super) async fn read_definition_file(
        &self,
        actor: &MacroUserIdStr<'static>,
        path: String,
    ) -> Result<DefinitionFile, FactoryError> {
        if !is_definition_file(&path) {
            return Err(FactoryError::NotADefinitionFile(path));
        }
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        let repo_url = self
            .named_definition(team)
            .await?
            .ok_or(FactoryError::NoDefinition)?;
        let DefinitionHead {
            repository, commit, ..
        } = self.definition_head(actor, &repo_url).await?;
        let files = RepositoryFiles {
            github: &self.github,
            reader: actor,
            repository: &repository,
            commit: &commit,
        };
        let text = files.read(&path).await.map_err(|error| match error {
            RepositoryFileError::Refused { path, refusal } => {
                FactoryError::InvalidDefinition(refused_file(path, refusal))
            }
            RepositoryFileError::Github(error) => FactoryError::Internal(internal(error)),
        })?;
        Ok(DefinitionFile { path, commit, text })
    }

    /// The open pull requests of the Definition `repo_url`, as `reader`
    /// reaches them; none when GitHub fails to list them, which is not the
    /// settings' failure.
    async fn definition_pull_requests(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Option<Vec<DefinitionPullRequest>> {
        let repository = CodeRepository::parse(repo_url)?;
        match self
            .github
            .open_pull_requests(reader, &repository, DEFINITION_PULL_REQUESTS_LISTED)
            .await
        {
            Ok(open) => Some(open),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "the definition's open pull requests could not be listed"
                );
                None
            }
        }
    }

    /// Whether `pinned`'s default branch takes changes only through a pull
    /// request, as `reader` can tell; none when they cannot, or GitHub
    /// fails to say.
    async fn requires_pull_request(
        &self,
        reader: &MacroUserIdStr<'static>,
        pinned: &PinnedDefinition,
    ) -> Option<bool> {
        let repository = CodeRepository::parse(&pinned.repo_url)?;
        match self
            .github
            .requires_pull_request(reader, &repository, &pinned.branch)
            .await
        {
            Ok(requires) => requires,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "whether the definition's branch requires a pull request could not be read"
                );
                None
            }
        }
    }

    /// Name `repo_url` as `actor`'s team's Factory Definition once it reads
    /// as they reach it, or name none.
    pub(super) async fn name_definition(
        &self,
        actor: MacroUserIdStr<'static>,
        repo_url: Option<String>,
    ) -> Result<DefinitionSettings, FactoryError> {
        let team = self.administered_team(&actor).await?;
        // The Definition is read once: to check it, and for the settings
        // that answer.
        let pinned = match repo_url {
            Some(repo_url) => Some(self.definition_at_head(&actor, &repo_url).await?),
            None => None,
        };
        let repo_url = pinned.as_ref().map(|pinned| pinned.repo_url.clone());
        self.repo
            .set_team_definition(team.team_id, repo_url.as_deref(), &actor)
            .await
            .map_err(internal)?;
        // The same write settles a self-improvement Run open on a
        // Definition the team no longer names.
        // An administered team's admin is never an agent's account.
        self.settings_of(&actor, Some(team), true, repo_url, pinned.map(Ok))
            .await
    }

    /// The team `actor` administers, to change a setting a Factory
    /// Definition holds: refused while the team names one, whose
    /// repository's pull requests change it.
    pub(super) async fn settings_team(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<TeamMembership, FactoryError> {
        let team = self.administered_team(actor).await?;
        if self.named_definition(Some(team)).await?.is_some() {
            return Err(FactoryError::DefinitionNamed);
        }
        Ok(team)
    }
}
