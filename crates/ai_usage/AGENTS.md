# ai_usage

## What it is

Part of billing: a robust log of AI usage with cost computed from model id
and token counts (or audio duration, or provider credits a vendor such as
Tavily bills), feeding `ai_billing`'s admission and ledger decisions, plus a
flexible admin query API.

## Start here

- `src/lib.rs` doc comment: the layering (`domain`/`outbound`/`inbound`)
  and the split between best-effort analytics tracking and the awaited,
  fail-closed `FinancialUsage` capability for billable traffic.
- `src/domain/financial.rs` + `src/domain/financial_service.rs`: immutable
  provider cost evidence and exact customer-money calculation.
- `src/domain/tracking.rs`: `UsageTracking` — the always-recorded log, used
  for analytics regardless of billing status.
- `src/domain/counting.rs`: `is_billable_feature` and the
  `NON_BILLABLE_AI_FEATURES` allowlist.
- `src/domain/session_usage.rs` + `src/outbound/pg_session_usage.rs`:
  `SessionUsageReader`, the tokens recorded for given agent sessions of one
  user (factory insights). Usage recorded with a session only as its
  `entity`, as the harness's model gateway records it, is not counted.
  Its read starts from the session ids and looks each up through
  `ai_usage_agent_session_id_idx`, the user check fenced out of the lookup
  (`OFFSET 0`). Measured on PostgreSQL 18 with 5.96M `ai_usage` rows (361,000
  of them one user's) and 3,000 sessions of 6 calls each, every plan uses
  only that index for 6 rows per session: fresh statistics, custom plan 33.8
  and 32.8 ms, generic 32.1 and 32.3 ms; stale statistics (the table looks
  empty), custom 33.4 and 33.7 ms, generic 34.8 and 37.9 ms. Without the
  fence, stale statistics add the user's index to every lookup: 39.4 s
  generic, and the custom plan reads all of the user's rows.
- `src/outbound/pg_usage_repo.rs`, `pg_tracking_repo.rs`,
  `pg_financial_usage_repo.rs`: the Postgres adapters.

## Scope

- Owns: pricing a model invocation and logging usage/cost.
- Not here: plan allowances, credits, and settlement → `ai_billing` (reads
  this crate's financial usage, never recomputes cost itself).

## Boundaries

- Depends on: none notable outside the workspace-hack set.
- Used by: `agent`, `agent_session`, `agent_harness`, `agent_trigger`,
  `ai_billing`, `ai_tools`, `anthropic`, `call`,
  `documents`, `import`, `image_generation`, `memory`, `channel_bots`,
  `agent_inmem`, `dictation`, `task_dedup`, `jev`, `factory_engine`.

## Recipes

- Price a new model: `src/domain/financial.rs` (rate entry) →
  `src/config.rs` (if a new Doppler-sourced rate) → test in
  `src/config/test.rs`.
- Mark a feature non-billable: `src/domain/counting.rs`'s
  `NON_BILLABLE_AI_FEATURES`.

## Checks

- `cargo test -p ai_usage` (`SQLX_OFFLINE` unset).

## Gotchas

- `UsageRecorder` (analytics) is best-effort and never fails the
  originating call; `FinancialUsage` (billable) is awaited and fail-closed
  — don't conflate the two when adding a new recording path.
- Read prices from the `ai_effective_pricing` view, never `ai_pricing`
  directly: it applies `ai_pricing_discount`, a per-model input and output
  percent off (input's also covers cache, audio and provider-credit rates). Gemini 3.8 Flash
  is discounted to DeepSeek V4.1 Flash's $0.30 / $1.20.
- `UsageContext::with_agent_api_key` marks usage an agent's own API key paid
  for: it is stored with `agent_api_key_id` and never counted. Set it only
  on calls that actually authenticate with that key.
- `UsageContext::with_agent_session` stores the agent session a call is part
  of in `agent_session_id`. A session's tools get a recorder wrapped in
  `for_agent_session`, which stamps the session on any event that names
  none. A service that keeps the recorder it was built with (image
  generation) gets no stamp and must carry the session in its request.

## Related

- Rules: `.agents/rules/config-and-service-urls.md`
