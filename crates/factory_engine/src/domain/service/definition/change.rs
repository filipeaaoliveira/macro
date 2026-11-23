//! A change to a team's Factory Definition, proposed from the factory's
//! settings: checked as the Definition would read with it, committed on a
//! branch of its own, and opened as a pull request a person merges.
//!
//! Editing the Definition changes the rules every Run of the team runs
//! under, so only an admin of the team proposes, in their own signed-in
//! session (never an agent's account), with their own GitHub link. Nothing
//! is pushed to the default branch: the change is committed on a new
//! branch, which GitHub only creates, and reaches the team's Runs once a
//! person merges its pull request, as any change to the Definition does.

use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::self_improvement::SelfImprovementRepository;
use factory_records::domain::standing_acts::StandingActRepository;
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{DefinitionHead, RepositoryFileError, RepositoryFiles, refused_file};
use crate::domain::definition::{
    DefinitionChange, DefinitionChangeError, DefinitionChangeReadError, read_changed_definition,
};
use crate::domain::models::{
    DefinitionPullRequest, FactoryError, NewBranch, OpenPullRequest, Opening,
};
use crate::domain::ports::{
    PullRequests, RunWorkflows, StepExecutor, TaskDirectory, UserDirectory,
};
use crate::domain::service::{FactoryServiceImpl, internal};

/// The branch a change to a Definition is committed on: one per change, so
/// sending the same change again finds its branch and pull request.
fn change_branch(change_id: Uuid) -> String {
    format!("definition-change/{change_id}")
}

/// The description of a change's pull request: the person's, and where it
/// came from.
fn change_body(change: &DefinitionChange, commit: &str) -> String {
    let made = format!(
        "Proposed from the factory's settings, and checked against the definition at {commit}."
    );
    if change.description.is_empty() {
        return made;
    }
    format!("{}\n\n{made}", change.description)
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
    /// Propose `change` to the Factory Definition `actor`'s team names, as
    /// the pull request of the change `change_id`: once the Definition
    /// reads with it at its default branch's head, commit it on the
    /// change's own branch and open its pull request, as `actor` on GitHub.
    /// Sending the same change again answers the pull request it opened.
    pub(in crate::domain::service) async fn propose_change(
        &self,
        actor: MacroUserIdStr<'static>,
        change_id: Uuid,
        change: DefinitionChange,
    ) -> Result<DefinitionPullRequest, FactoryError> {
        let team = self.administered_team(&actor).await?;
        let repo_url = self
            .named_definition(Some(team))
            .await?
            .ok_or(FactoryError::NoDefinition)?;
        let DefinitionHead {
            repository,
            access,
            commit,
        } = self.definition_head(&actor, &repo_url).await?;
        if !access.can_push {
            return Err(FactoryError::NoPushAccess);
        }

        // The change is checked as the Definition would read with it at
        // the commit it goes on top of: the same checks a Run's start makes.
        let files = RepositoryFiles {
            github: &self.github,
            reader: &actor,
            repository: &repository,
            commit: &commit,
        };
        read_changed_definition(&files, &change, |step| self.executor.serves(step))
            .await
            .map_err(|error| match error {
                DefinitionChangeReadError::Files(RepositoryFileError::Refused {
                    path,
                    refusal,
                }) => DefinitionChangeError::Invalid(refused_file(path, refusal)).into(),
                DefinitionChangeReadError::Files(RepositoryFileError::Github(error)) => {
                    FactoryError::Internal(internal(error))
                }
                DefinitionChangeReadError::Refused(error) => error.into(),
            })?;

        // The branch is only ever created, never moved: a change sent again
        // finds its own branch there and leaves it as it is.
        let branch = change_branch(change_id);
        let body = change_body(&change, &commit);
        let DefinitionChange { title, files, .. } = change;
        self.github
            .create_branch(
                &actor,
                &repository,
                &NewBranch {
                    name: branch.clone(),
                    base_commit: commit,
                    message: title.clone(),
                    files,
                },
            )
            .await
            .map_err(internal)?;
        let opening = self
            .github
            .open(
                &actor,
                &repository,
                &OpenPullRequest {
                    branch: branch.clone(),
                    base: access.default_branch,
                    title: title.clone(),
                    body,
                },
            )
            .await
            .map_err(internal)?;

        // GitHub refuses a second pull request from the branch: the first
        // one, opened by the change sent before, is the answer.
        let found = self
            .github
            .find(&actor, &repository, &branch)
            .await
            .map_err(internal)?;
        match (found, opening) {
            (Some(pull_request), _) => Ok(DefinitionPullRequest {
                number: pull_request.number,
                title,
                url: pull_request.url,
                draft: false,
            }),
            (None, Opening::Refused(why)) => Err(FactoryError::DefinitionChangeNotOpened(why)),
            (None, Opening::Opened) => Err(FactoryError::Internal(rootcause::report!(
                "the definition change's pull request was opened but is not found"
            ))),
        }
    }
}
