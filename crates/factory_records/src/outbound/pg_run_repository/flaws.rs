//! The Postgres Run repository's flawed Runs: the flaw columns of
//! `factory_run`.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::flaws::{FlawMarking, FlawRepository};
use crate::domain::models::{FlawMarker, RunEnd, RunFlaw, RunId, RunOutcome};

/// The flaw a Run's row keeps, if it was marked flawed.
pub(super) fn run_flaw(
    flawed_at: Option<DateTime<Utc>>,
    by_user_id: Option<String>,
    by_scorer: Option<String>,
    reason: Option<String>,
) -> Result<Option<RunFlaw>, PgRunRepositoryErr> {
    let by = match (by_user_id, by_scorer) {
        (Some(user_id), None) => Some(FlawMarker::Person {
            user_id: MacroUserIdStr::try_from(user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(user_id))?,
        }),
        (None, Some(name)) => Some(FlawMarker::Scorer { name }),
        (None, None) => None,
        (Some(_), Some(_)) => return Err(PgRunRepositoryErr::PartialFlaw),
    };
    match (flawed_at, by, reason) {
        (Some(marked_at), Some(by), Some(reason)) => Ok(Some(RunFlaw {
            by,
            reason,
            marked_at,
        })),
        (None, None, None) => Ok(None),
        _ => Err(PgRunRepositoryErr::PartialFlaw),
    }
}

impl FlawRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, reason))]
    async fn mark_flawed(
        &self,
        id: RunId,
        by: &FlawMarker,
        reason: &str,
    ) -> Result<FlawMarking, Self::Err> {
        let (by_user_id, by_scorer) = match by {
            FlawMarker::Person { user_id } => (Some(user_id.as_ref()), None),
            FlawMarker::Scorer { name } => (None, Some(name.as_str())),
        };

        // Only a Run that ended `success` turns `flawed`; one already
        // flawed keeps its first mark. Either way the Run is read back
        // after, so a repeated mark answers as the first did.
        sqlx::query!(
            r#"
            UPDATE factory_run
            SET outcome = 'flawed', flawed_at = now(), flawed_by_user_id = $2,
                flawed_by_scorer = $3, flaw_reason = $4
            WHERE id = $1 AND outcome = 'success'
            "#,
            id.as_uuid(),
            by_user_id,
            by_scorer,
            reason,
        )
        .execute(&self.pool)
        .await?;

        let row = sqlx::query!(
            r#"
            SELECT outcome, ended_at, flawed_at, flawed_by_user_id, flawed_by_scorer, flaw_reason
            FROM factory_run
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(FlawMarking::NoSuchRun);
        };
        let outcome = row
            .outcome
            .map(|outcome| {
                outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))
            })
            .transpose()?;
        let flaw = run_flaw(
            row.flawed_at,
            row.flawed_by_user_id,
            row.flawed_by_scorer,
            row.flaw_reason,
        )?;
        match (outcome, row.ended_at) {
            (Some(RunOutcome::Flawed), Some(ended_at)) => Ok(FlawMarking::Flawed(RunEnd {
                outcome: RunOutcome::Flawed,
                ended_at,
                flaw,
            })),
            (outcome, _) => Ok(FlawMarking::NotSucceeded(outcome)),
        }
    }
}
