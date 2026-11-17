//! Insights: how a person's Runs went over a period, summed up for them.
//!
//! The records give the person's Runs, newest first and capped, and their
//! Steps summed up by entry; agent usage comes per Step session. This module
//! counts Runs by day, outcome and Flow, measures how long they took, and
//! lays the entries out in their Flows' order.

#[cfg(test)]
pub(crate) mod fakes;
#[cfg(test)]
mod test;

use std::collections::{BTreeMap, HashMap};

use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use chrono::{DateTime, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use factory_records::domain::insights::{
    DurationSpread, EntryCountKind, InsightsRecords, InsightsScope, RunInsightsRepository,
    RunSummary,
};
use factory_records::domain::models::{RunId, RunOutcome, checked_name};
use macro_user_id::user_id::MacroUserIdStr;
use serde::Serialize;

use crate::domain::built_in_flows::{BUILT_IN_FLOWS, BuiltInFlow};
use crate::domain::flow::StepUse;

/// The most Runs insights read for one period: the newest in it.
pub const INSIGHTS_RUN_CAP: u16 = 1000;

/// The longest period insights read, in days.
pub const MAX_INSIGHTS_DAYS: i64 = 366;

/// The most agent sessions one usage read sums up.
const SESSIONS_PER_READ: usize = 1000;

/// How many Runs [`UsageInsight::heaviest`] names.
const HEAVIEST_RUNS: usize = 5;

/// Errors of the insights use case.
#[derive(Debug, thiserror::Error)]
pub enum InsightsError {
    /// The period ends before it starts, or is too long.
    #[error("the period must end on or after its start and last at most {MAX_INSIGHTS_DAYS} days")]
    InvalidPeriod,
    /// The Flow filter is not a Flow name.
    #[error(transparent)]
    InvalidFlow(#[from] factory_records::domain::models::InvalidName),
    /// Anything else.
    #[error("insights failed: {0:?}")]
    Internal(rootcause::Report),
}

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> InsightsError {
    InsightsError::Internal(rootcause::Report::new(error).into_dynamic())
}

/// The period, and the Flow, insights are asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct InsightsQuery {
    from: NaiveDate,
    to: NaiveDate,
    time_zone: Tz,
    flow: Option<String>,
}

impl InsightsQuery {
    /// The days `from` to `to`, both included, in the IANA time zone
    /// `time_zone`, of the Flow named `flow` or of every Flow. A time zone
    /// the time zone database does not name, as a browser may report one,
    /// reads as UTC; the insights say which time zone they used.
    pub fn new(
        from: NaiveDate,
        to: NaiveDate,
        time_zone: &str,
        flow: Option<String>,
    ) -> Result<Self, InsightsError> {
        let days = to.signed_duration_since(from).num_days();
        if !(0..MAX_INSIGHTS_DAYS).contains(&days) {
            return Err(InsightsError::InvalidPeriod);
        }
        let time_zone = time_zone.parse::<Tz>().unwrap_or(Tz::UTC);
        let flow = flow.map(|flow| checked_name("flow", flow)).transpose()?;
        Ok(Self {
            from,
            to,
            time_zone,
            flow,
        })
    }

    /// Every day of the period, in order.
    fn days(&self) -> impl Iterator<Item = NaiveDate> {
        self.from.iter_days().take_while(|day| *day <= self.to)
    }
}

/// When `day` starts in `zone`: its midnight, or the first hour it has when a
/// clock change skips midnight.
fn start_of_day(zone: Tz, day: NaiveDate) -> Option<DateTime<Utc>> {
    (0..=3).find_map(|hour| {
        let local = day.and_hms_opt(hour, 0, 0)?;
        let start = zone.from_local_datetime(&local).earliest()?;
        Some(start.with_timezone(&Utc))
    })
}

/// How many Runs went each way: still going, or ended with each outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeCounts {
    /// Runs still going.
    pub running: u32,
    /// Runs whose Flow reached its successful end.
    pub success: u32,
    /// Runs that ended for a person to take over.
    pub manual: u32,
    /// Runs that ran out of visits, or of Steps, with nowhere to go.
    pub blocked: u32,
    /// Runs that ended failed.
    pub failed: u32,
    /// Runs a person cancelled.
    pub cancelled: u32,
}

impl OutcomeCounts {
    fn add(&mut self, outcome: Option<RunOutcome>) {
        let count = match outcome {
            None => &mut self.running,
            Some(RunOutcome::Success) => &mut self.success,
            Some(RunOutcome::Manual) => &mut self.manual,
            Some(RunOutcome::Blocked) => &mut self.blocked,
            Some(RunOutcome::Failed) => &mut self.failed,
            Some(RunOutcome::Cancelled) => &mut self.cancelled,
        };
        *count += 1;
    }
}

/// The Runs created on one day of the period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InsightDay {
    /// The day, in the period's time zone.
    pub date: NaiveDate,
    /// How its Runs went.
    pub counts: OutcomeCounts,
}

/// The Runs of one Flow.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowInsight {
    /// The Flow's name; none for Runs recorded before the Flow was kept.
    pub flow: Option<String>,
    /// How its Runs went.
    pub counts: OutcomeCounts,
}

/// A spread of durations: how many, their median and 90th percentile.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DurationStats {
    /// How many durations were measured.
    pub count: u32,
    /// The median, in seconds.
    pub median_seconds: f64,
    /// The 90th percentile, in seconds.
    pub p90_seconds: f64,
}

impl From<DurationSpread> for DurationStats {
    fn from(spread: DurationSpread) -> Self {
        Self {
            count: spread.count,
            median_seconds: spread.median_seconds,
            p90_seconds: spread.p90_seconds,
        }
    }
}

/// A name a Flow gives (an outcome, a Gate's answer) and how often it came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamedCount {
    /// The name, as the Flow writes it.
    pub name: String,
    /// How many times.
    pub count: u32,
}

/// One of the period's Runs, to link to.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunLink {
    /// The Run.
    pub id: RunId,
    /// The first line of its request.
    pub title: String,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// How it ended; none while it goes.
    pub outcome: Option<RunOutcome>,
}

impl From<&RunSummary> for RunLink {
    fn from(run: &RunSummary) -> Self {
        Self {
            id: run.id,
            title: run.title.clone(),
            created_at: run.created_at,
            outcome: run.end.map(|(outcome, _)| outcome),
        }
    }
}

/// A Run where an entry failed or was visited more than once.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TroubleRun {
    /// The Run.
    pub run: RunLink,
    /// Whether one of the entry's Steps in it failed.
    pub failed: bool,
    /// How many times it visited the entry.
    pub visits: u32,
}

/// Tokens agent model calls used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    /// Input tokens, besides those read from or written to the prompt cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
}

impl TokenCounts {
    fn add(&mut self, tokens: &SessionTokens) {
        self.input += tokens.input;
        self.output += tokens.output;
        self.cache_read += tokens.cache_read;
        self.cache_write += tokens.cache_write;
    }

    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// A Gate's questions and their answers.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GateInsight {
    /// Gates opened.
    pub opened: u32,
    /// Gates a person answered.
    pub answered: u32,
    /// How long people took to answer.
    pub wait: Option<DurationStats>,
    /// How often each answer was given, most given first.
    pub answers: Vec<NamedCount>,
}

/// One of a Flow's entries, across the period's Runs.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntryInsight {
    /// The entry's id, as its Flow names it.
    pub entry: String,
    /// Its Steps.
    pub steps: u32,
    /// The Runs that visited it.
    pub runs: u32,
    /// The Runs that visited it more than once: sent back to it, or started
    /// over.
    pub revisited_runs: u32,
    /// The Runs in which one of its Steps failed.
    pub failed_runs: u32,
    /// The Runs in which one of its Steps failed or that visited it more
    /// than once, each counted once.
    pub troubled_runs: u32,
    /// How long its Steps took, a cancelled Run's left out.
    pub duration: Option<DurationStats>,
    /// How its Steps ended, most often first.
    pub outcomes: Vec<NamedCount>,
    /// Its Gate, if its Steps opened one.
    pub gate: Option<GateInsight>,
    /// The tokens its agent Steps used, if any recorded usage.
    pub tokens: Option<TokenCounts>,
    /// The newest Runs where it failed or was visited more than once.
    pub trouble: Vec<TroubleRun>,
}

/// One of the period's Runs and the tokens its agent Steps used.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunTokens {
    /// The Run.
    pub run: RunLink,
    /// What its agents used.
    pub tokens: TokenCounts,
}

/// What the period's agent Steps used.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageInsight {
    /// Steps an agent worked on, in a session.
    pub agent_steps: u32,
    /// Those whose session has usage recorded. An agent that runs on a
    /// person's own computer records none.
    pub measured_steps: u32,
    /// Runs with any usage recorded.
    pub measured_runs: u32,
    /// What they used, summed.
    pub tokens: TokenCounts,
    /// The Runs that used the most tokens, most first.
    pub heaviest: Vec<RunTokens>,
}

/// How a person's Runs went over a period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunInsights {
    /// The period's first day.
    pub from: NaiveDate,
    /// Its last day.
    pub to: NaiveDate,
    /// The time zone its days are in.
    pub time_zone: String,
    /// The Flow read, or none for every Flow.
    pub flow: Option<String>,
    /// The most Runs read: the newest in the period.
    pub run_cap: u16,
    /// Whether the period had more Runs than that, so the oldest are left
    /// out.
    pub capped: bool,
    /// How the Runs went.
    pub totals: OutcomeCounts,
    /// The Runs created each day of the period, every day listed.
    pub days: Vec<InsightDay>,
    /// The Runs of each Flow, most Runs first.
    pub flows: Vec<FlowInsight>,
    /// How long Runs took from creation to their end, cancelled Runs and
    /// Runs still going left out.
    pub run_duration: Option<DurationStats>,
    /// The Flows' entries the Runs visited, in their Flows' order.
    pub entries: Vec<EntryInsight>,
    /// What the agent Steps used.
    pub usage: UsageInsight,
}

/// The insights use case.
pub trait InsightsService: Send + Sync + 'static {
    /// How `actor`'s own Runs went over the period of `query`.
    fn insights(
        &self,
        actor: &MacroUserIdStr<'static>,
        query: InsightsQuery,
    ) -> impl Future<Output = Result<RunInsights, InsightsError>> + Send;
}

/// Insights over the Run records and the agent usage log.
pub struct InsightsServiceImpl<R, U> {
    repo: R,
    usage: U,
}

impl<R, U> InsightsServiceImpl<R, U> {
    /// Create the service.
    pub fn new(repo: R, usage: U) -> Self {
        Self { repo, usage }
    }
}

impl<R, U> InsightsService for InsightsServiceImpl<R, U>
where
    R: RunInsightsRepository,
    U: SessionUsageReader,
{
    #[tracing::instrument(skip(self, actor))]
    async fn insights(
        &self,
        actor: &MacroUserIdStr<'static>,
        query: InsightsQuery,
    ) -> Result<RunInsights, InsightsError> {
        let next_day = query
            .to
            .checked_add_days(Days::new(1))
            .ok_or(InsightsError::InvalidPeriod)?;
        let from = start_of_day(query.time_zone, query.from).ok_or(InsightsError::InvalidPeriod)?;
        let to = start_of_day(query.time_zone, next_day).ok_or(InsightsError::InvalidPeriod)?;
        let scope = InsightsScope {
            user_id: actor.clone(),
            from,
            to,
            flow: query.flow.clone(),
            cap: INSIGHTS_RUN_CAP,
        };
        let records = self.repo.read_insights(&scope).await.map_err(internal)?;

        let session_ids: Vec<_> = records
            .sessions
            .iter()
            .map(|session| session.session_id)
            .collect();
        let mut tokens = Vec::new();
        for sessions in session_ids.chunks(SESSIONS_PER_READ) {
            let read = self
                .usage
                .session_tokens(actor, sessions)
                .await
                .map_err(internal)?;
            tokens.extend(read);
        }

        Ok(summarize(&query, records, &tokens))
    }
}

/// The entries of the Flow `flow`, or of every built-in Flow, in their
/// Flows' order: where an entry first appears.
fn entry_order(flow: Option<&str>) -> Vec<String> {
    let flows: Vec<BuiltInFlow> = match flow {
        Some(name) => BuiltInFlow::named(name).into_iter().collect(),
        None => BUILT_IN_FLOWS.into_iter().collect(),
    };
    let mut order: Vec<String> = Vec::new();
    for built_in in flows {
        let Ok(read) = built_in.read(|_| Some(StepUse::Work)) else {
            continue;
        };
        for entry in read.entries {
            let id = entry.id.as_str().to_string();
            if !order.contains(&id) {
                order.push(id);
            }
        }
    }
    order
}

/// The `p`th percentile of `sorted`, between its two nearest values, as
/// Postgres's `percentile_cont` computes it.
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let place = p * last as f64;
    let below = place.floor() as usize;
    let above = place.ceil() as usize;
    let low = *sorted.get(below)?;
    let high = *sorted.get(above)?;
    let value = low + (place - below as f64) * (high - low);
    // To the millisecond, as the records keep Step durations.
    Some((value * 1000.0).round() / 1000.0)
}

fn duration_stats(mut seconds: Vec<f64>) -> Option<DurationStats> {
    seconds.sort_by(f64::total_cmp);
    Some(DurationStats {
        count: u32::try_from(seconds.len()).ok()?,
        median_seconds: percentile(&seconds, 0.5)?,
        p90_seconds: percentile(&seconds, 0.9)?,
    })
}

fn by_count_then_name(counts: BTreeMap<String, u32>) -> Vec<NamedCount> {
    let mut counts: Vec<NamedCount> = counts
        .into_iter()
        .map(|(name, count)| NamedCount { name, count })
        .collect();
    counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    counts
}

/// Sum up `records` for `query`, with `tokens` the usage of their sessions.
fn summarize(
    query: &InsightsQuery,
    records: InsightsRecords,
    tokens: &[SessionTokens],
) -> RunInsights {
    let runs: HashMap<RunId, &RunSummary> = records.runs.iter().map(|run| (run.id, run)).collect();

    // Runs by day, outcome and Flow.
    let mut totals = OutcomeCounts::default();
    let mut days: BTreeMap<NaiveDate, OutcomeCounts> = query
        .days()
        .map(|day| (day, OutcomeCounts::default()))
        .collect();
    let mut flows: BTreeMap<Option<String>, OutcomeCounts> = BTreeMap::new();
    let mut run_seconds = Vec::new();
    for run in &records.runs {
        let outcome = run.end.map(|(outcome, _)| outcome);
        totals.add(outcome);
        let day = run.created_at.with_timezone(&query.time_zone).date_naive();
        if let Some(counts) = days.get_mut(&day) {
            counts.add(outcome);
        }
        flows.entry(run.flow.clone()).or_default().add(outcome);
        if let Some((outcome, ended_at)) = run.end
            && outcome != RunOutcome::Cancelled
        {
            run_seconds.push((ended_at - run.created_at).num_milliseconds() as f64 / 1000.0);
        }
    }
    let mut flows: Vec<FlowInsight> = flows
        .into_iter()
        .map(|(flow, counts)| FlowInsight { flow, counts })
        .collect();
    flows.sort_by_key(|flow| std::cmp::Reverse(total(&flow.counts)));

    // Usage, by entry and by Run.
    let tokens_by_session: HashMap<_, _> = tokens
        .iter()
        .map(|tokens| (tokens.session_id, tokens))
        .collect();
    let mut entry_tokens: HashMap<&str, TokenCounts> = HashMap::new();
    let mut run_tokens: HashMap<RunId, TokenCounts> = HashMap::new();
    let mut usage_total = TokenCounts::default();
    let mut measured_steps = 0;
    for session in &records.sessions {
        let Some(used) = tokens_by_session.get(&session.session_id) else {
            continue;
        };
        measured_steps += 1;
        usage_total.add(used);
        entry_tokens
            .entry(session.entry.as_str())
            .or_default()
            .add(used);
        run_tokens.entry(session.run_id).or_default().add(used);
    }
    let mut heaviest: Vec<RunTokens> = run_tokens
        .iter()
        .filter_map(|(run_id, tokens)| {
            let run = runs.get(run_id)?;
            Some(RunTokens {
                run: RunLink::from(*run),
                tokens: *tokens,
            })
        })
        .collect();
    heaviest.sort_by(|a, b| {
        b.tokens
            .total()
            .cmp(&a.tokens.total())
            .then_with(|| b.run.created_at.cmp(&a.run.created_at))
    });
    heaviest.truncate(HEAVIEST_RUNS);

    // Entries, with their outcomes, answers and troubled Runs.
    let mut outcomes: HashMap<&str, BTreeMap<String, u32>> = HashMap::new();
    let mut answers: HashMap<&str, BTreeMap<String, u32>> = HashMap::new();
    for count in &records.counts {
        let into = match count.kind {
            EntryCountKind::Outcome => &mut outcomes,
            EntryCountKind::Answer => &mut answers,
        };
        into.entry(count.entry.as_str())
            .or_default()
            .insert(count.value.clone(), count.count);
    }
    let mut trouble: HashMap<&str, Vec<TroubleRun>> = HashMap::new();
    for row in &records.trouble {
        let Some(run) = runs.get(&row.run_id) else {
            continue;
        };
        trouble
            .entry(row.entry.as_str())
            .or_default()
            .push(TroubleRun {
                run: RunLink::from(*run),
                failed: row.failed,
                visits: row.visits,
            });
    }

    let order = entry_order(query.flow.as_deref());
    let place = |entry: &str| {
        order
            .iter()
            .position(|id| id == entry)
            .unwrap_or(order.len())
    };
    let mut entries: Vec<EntryInsight> = records
        .entries
        .iter()
        .map(|summary| {
            let entry = summary.entry.as_str();
            let gate = (summary.gates_opened > 0).then(|| GateInsight {
                opened: summary.gates_opened,
                answered: summary.gates_answered,
                wait: summary.gate_wait.map(DurationStats::from),
                answers: by_count_then_name(answers.remove(entry).unwrap_or_default()),
            });
            EntryInsight {
                entry: entry.to_string(),
                steps: summary.steps,
                runs: summary.runs,
                revisited_runs: summary.revisited_runs,
                failed_runs: summary.failed_runs,
                troubled_runs: summary.troubled_runs,
                duration: summary.duration.map(DurationStats::from),
                outcomes: by_count_then_name(outcomes.remove(entry).unwrap_or_default()),
                gate,
                tokens: entry_tokens.get(entry).copied(),
                trouble: trouble.remove(entry).unwrap_or_default(),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        place(&a.entry)
            .cmp(&place(&b.entry))
            .then_with(|| a.entry.cmp(&b.entry))
    });

    RunInsights {
        from: query.from,
        to: query.to,
        time_zone: query.time_zone.name().to_string(),
        flow: query.flow.clone(),
        run_cap: INSIGHTS_RUN_CAP,
        capped: records.capped,
        totals,
        days: days
            .into_iter()
            .map(|(date, counts)| InsightDay { date, counts })
            .collect(),
        flows,
        run_duration: duration_stats(run_seconds),
        entries,
        usage: UsageInsight {
            agent_steps: u32::try_from(records.sessions.len()).unwrap_or(u32::MAX),
            measured_steps,
            measured_runs: u32::try_from(run_tokens.len()).unwrap_or(u32::MAX),
            tokens: usage_total,
            heaviest,
        },
    }
}

fn total(counts: &OutcomeCounts) -> u32 {
    counts.running
        + counts.success
        + counts.manual
        + counts.blocked
        + counts.failed
        + counts.cancelled
}
