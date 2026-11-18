//! [`AutomationTriggers`] over Macro's routines: a schedule's, a
//! repository event's and a Task event's Automation is fired by a routine
//! of its creator's, made through the scheduler's API with the internal
//! key and the creator as the acting user, as the routine tools do
//! (`crates/routines`).
//!
//! The routine's trigger is the scheduler's own: a cron trigger, a GitHub
//! event filter on the Automation's repository, or a task property change
//! filter. Its one code step fires the Automation through the SDK
//! (`macro.factory.fireAutomation`), keyed by the routine run, and skips a
//! dry run.

#[cfg(test)]
mod test;

use std::time::Duration;

use factory_records::domain::automations::{Automation, AutomationTrigger};
use macro_authorization::{INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER};
use macro_user_id::user_id::MacroUserIdStr;
use reqwest::{Client, Method, StatusCode, Url, header::HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::models::CodeRepository;
use crate::domain::ports::{AutomationTriggerError, AutomationTriggers};

/// The code step that fires the Automation: the routine run keys the
/// firing, a Task event names its task and a repository event its GitHub
/// delivery, with the branch it is about, so a delivery about a factory
/// Run's own branch starts nothing. A dry run starts nothing.
const FIRE_STEP: &str = r#"export async function main({ macro, run, trigger }: StepContext) {
  if (run.dryRun) {
    return { started: false, skipped: null };
  }
  const item = trigger.kind === 'event' ? trigger.item : undefined;
  const github = item?.github;
  const payload = github?.payload;
  const pushed =
    typeof payload?.ref === 'string'
      ? payload.ref.replace(/^refs\/heads\//, '')
      : undefined;
  const branch =
    payload?.pull_request?.head?.ref ??
    payload?.workflow_run?.head_branch ??
    payload?.check_suite?.head_branch ??
    payload?.check_run?.check_suite?.head_branch ??
    pushed;
  const fired = await macro.factory.fireAutomation({
    run,
    task:
      trigger.kind === 'event' && trigger.event.startsWith('task.') && item
        ? macro.tasks.byId(item.entity_id)
        : undefined,
    github: github
      ? {
          event: github.event,
          action: github.action ?? undefined,
          branch: typeof branch === 'string' ? branch : undefined,
        }
      : undefined,
  });
  return { started: fired.started, skipped: fired.skipped ?? null };
}
"#;

/// The most bytes of the scheduler's refusal kept as its reason.
const MAX_REASON_BYTES: usize = 300;

/// Makes and changes the routines that fire Automations.
#[derive(Debug, Clone)]
pub struct RoutineAutomationTriggers {
    client: Client,
    base: Url,
    key: HeaderValue,
}

impl RoutineAutomationTriggers {
    /// A client of the scheduler at `base`, which takes the internal key
    /// `key`.
    pub fn new(base: &str, key: &str) -> Result<Self, rootcause::Report> {
        let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))
            .map_err(|error| rootcause::report!("invalid scheduler URL: {error}"))?;
        let mut key = HeaderValue::from_str(key)
            .map_err(|error| rootcause::report!("invalid internal key: {error}"))?;
        key.set_sensitive(true);
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| rootcause::report!("could not build the scheduler client: {error}"))?;
        Ok(Self { client, base, key })
    }

    /// Send a request as `owner`, and return the scheduler's answer,
    /// whatever its status.
    async fn request(
        &self,
        owner: &MacroUserIdStr<'static>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response, AutomationTriggerError> {
        let url = self
            .base
            .join(path)
            .map_err(|error| AutomationTriggerError::Unavailable(rootcause::report!("{error}")))?;
        let mut request = self
            .client
            .request(method, url)
            .header(INTERNAL_API_KEY_HEADER, self.key.clone())
            .header(INTERNAL_MACRO_USER_ID_HEADER, owner.as_ref());
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .map_err(|error| AutomationTriggerError::Unavailable(rootcause::report!("{error}")))
    }

    /// Send a request as `owner`, and return a successful answer.
    async fn send(
        &self,
        owner: &MacroUserIdStr<'static>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response, AutomationTriggerError> {
        let response = self.request(owner, method, path, body).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        // A refusal says why: an invalid schedule, event routines the
        // creator may not have, or no linked GitHub account. Anything
        // else is the scheduler's failure.
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            return Err(AutomationTriggerError::Refused(reason(response).await));
        }
        Err(AutomationTriggerError::Unavailable(rootcause::report!(
            "the scheduler answered {status}"
        )))
    }
}

/// The scheduler's reason for a refusal, cut short.
async fn reason(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let mut end = body.len().min(MAX_REASON_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    match body[..end].trim() {
        "" => format!("the scheduler answered {status}"),
        reason => reason.to_string(),
    }
}

/// The scheduler's trigger for `automation`.
fn trigger(automation: &Automation) -> Result<Value, AutomationTriggerError> {
    let trigger = match &automation.trigger {
        AutomationTrigger::Schedule { cron, timezone } => json!({
            "type": "cron",
            "schedule": cron,
            "timezone": timezone,
        }),
        AutomationTrigger::RepositoryEvent { events, actions } => {
            let repository = automation
                .repo_url
                .as_deref()
                .and_then(CodeRepository::parse)
                .ok_or_else(|| {
                    AutomationTriggerError::Refused(
                        "a repository event needs the automation's repository".to_string(),
                    )
                })?;
            json!({
                "type": "events",
                "filters": [{
                    "events": ["github"],
                    "github": {
                        "events": events,
                        "actions": actions,
                        "repositories": [format!("{}/{}", repository.owner(), repository.name())],
                    },
                }],
            })
        }
        AutomationTrigger::TaskEvent {
            property_id,
            values,
        } => json!({
            "type": "events",
            "filters": [{
                "events": ["task.property_changed"],
                "property": { "definitionId": property_id, "values": values },
            }],
        }),
        AutomationTrigger::Webhook => {
            return Err(AutomationTriggerError::Refused(
                "a webhook is fired by its calls, not a routine".to_string(),
            ));
        }
    };
    Ok(trigger)
}

/// The routine that fires `automation`.
fn routine(automation: &Automation) -> Result<Value, AutomationTriggerError> {
    Ok(json!({
        "name": format!("Factory automation: {}", automation.name),
        "description": format!(
            "Managed by the factory: starts a run of the {} flow for the automation \"{}\" \
             each time it runs. Change or delete the automation in team settings, under \
             Automations; changes made to this routine are not read back.",
            automation.flow, automation.name,
        ),
        "kind": "Agent",
        "trigger": trigger(automation)?,
        "task": {
            "steps": [{
                "type": "code",
                "title": "Start the factory run",
                "summary": "Starts a run of the automation's flow, as its creator.",
                "source": FIRE_STEP,
            }],
        },
        "enabled": true,
    }))
}

/// The part of a saved routine read back.
#[derive(Deserialize)]
struct SavedRoutine {
    id: Uuid,
}

impl AutomationTriggers for RoutineAutomationTriggers {
    #[tracing::instrument(skip_all, fields(automation_id = %automation.id))]
    async fn create(&self, automation: &Automation) -> Result<Uuid, AutomationTriggerError> {
        let saved: SavedRoutine = self
            .send(
                &automation.created_by,
                Method::POST,
                "scheduled-actions",
                Some(routine(automation)?),
            )
            .await?
            .json()
            .await
            .map_err(|error| AutomationTriggerError::Unavailable(rootcause::report!("{error}")))?;
        Ok(saved.id)
    }

    #[tracing::instrument(skip(self, owner))]
    async fn set_enabled(
        &self,
        owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        enabled: bool,
    ) -> Result<(), AutomationTriggerError> {
        self.send(
            owner,
            Method::PUT,
            &format!("scheduled-actions/{routine_id}/enabled"),
            Some(json!({ "enabled": enabled })),
        )
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, owner))]
    async fn delete(
        &self,
        owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
    ) -> Result<(), AutomationTriggerError> {
        let response = self
            .request(
                owner,
                Method::DELETE,
                &format!("scheduled-actions/{routine_id}"),
                None,
            )
            .await?;
        let status = response.status();
        // A routine already gone, deleted by its owner or before a retry,
        // is deleted.
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(AutomationTriggerError::Unavailable(rootcause::report!(
            "the scheduler answered {status}"
        )))
    }
}
