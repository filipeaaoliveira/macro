//! What a person sees on GitHub, for the factory merge guard: read through
//! their GitHub link, as the mergeability route reads it.

use std::sync::Arc;

use github::domain::models::{GithubError, GithubPullRequestNumber};
use github::domain::ports::GithubLinkService;
use macro_user_id::{lowercased::Lowercase, user_id::MacroUserId};

use crate::domain::factory_merge_guard::PullRequestSight;

/// A person's sight of pull requests, through their GitHub link.
pub struct GithubLinkSight<G>(pub Arc<G>);

impl<G: GithubLinkService> PullRequestSight for GithubLinkSight<G> {
    async fn sees(
        &self,
        user: &MacroUserId<Lowercase<'static>>,
        pull_request: &GithubPullRequestNumber,
    ) -> Result<bool, GithubError> {
        // GitHub leaves out a pull request the person cannot see.
        let read = self
            .0
            .get_pull_request_mergeability(user, vec![pull_request.clone()])
            .await?;
        Ok(!read.is_empty())
    }
}
