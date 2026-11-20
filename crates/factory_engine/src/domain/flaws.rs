//! Marking a Run flawed: a Run that ended `success` whose result a Scorer
//! or a person found wrong afterwards. [`mark_flawed`] is the one call
//! both make; the Run API makes it for the Run's owner in person.

#[cfg(test)]
mod test;

use factory_records::domain::flaws::{FlawMarking, FlawRepository, MAX_FLAW_REASON_BYTES};
use factory_records::domain::models::{FlawMarker, RunEnd, RunId, checked_name};

use crate::domain::models::FactoryError;

/// Mark the Run `id` flawed by `by`, for `reason`, and return its end with
/// the flaw. The reason is trimmed, and must hold 1 to
/// [`MAX_FLAW_REASON_BYTES`] bytes; a Scorer is named as a Flow names
/// things. Only a Run that ended `success` turns flawed; marking a flawed
/// Run again keeps its first mark, so a caller may repeat the call. A
/// person's mark must name the Run's owner: the caller checks who may.
pub async fn mark_flawed<R: FlawRepository>(
    repo: &R,
    id: RunId,
    by: FlawMarker,
    reason: &str,
) -> Result<RunEnd, FactoryError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > MAX_FLAW_REASON_BYTES {
        return Err(FactoryError::FlawReasonSize {
            max: MAX_FLAW_REASON_BYTES,
        });
    }
    let by = match by {
        FlawMarker::Scorer { name } => FlawMarker::Scorer {
            name: checked_name("scorer", name).map_err(FactoryError::InvalidScorer)?,
        },
        person => person,
    };

    let marking = repo
        .mark_flawed(id, &by, reason)
        .await
        .map_err(|error| FactoryError::Internal(rootcause::Report::new(error).into_dynamic()))?;
    match marking {
        FlawMarking::Flawed(end) => Ok(end),
        FlawMarking::NotSucceeded(_) => Err(FactoryError::NotSucceeded),
        FlawMarking::NoSuchRun => Err(FactoryError::NotFound),
    }
}
