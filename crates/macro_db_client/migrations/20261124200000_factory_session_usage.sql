-- What the runtime of a software factory Step's agent session reported its
-- model calls used (crates/factory_records, `record_session_usage`): the
-- tokens each turn's prompt response counted, the cost the runtime's own
-- usage updates gave, in US dollars (null when it gave none), and the model
-- that used the most tokens. A runtime that calls its model directly, as
-- Claude Code does through macrod, records nothing in ai_usage, so this is
-- where Insights read its usage from. The session's log goes with the
-- session once its Step is done with it, so the factory keeps this as each
-- turn ends and before it deletes the session. `user_id` is the session's
-- owner, the Run's, as ai_usage keeps who a call ran as; `bot_id` the bot
-- that ran it.
--
-- `bot_id` on factory_step_used_session: the bot that ran each session a
-- Step worked in. A Step that goes on in a new session records the new
-- Agent's bot on its own row, so a session it left kept no bot and its
-- spend read as unknown. Filled for the sessions a Step still holds; a
-- session left before this keeps none.
--
-- The new table locks nothing else. The nullable column rewrites no row, and
-- the backfill reads factory_step once under factory_step_used_session's
-- ACCESS EXCLUSIVE lock, which a small table holds briefly. Give up rather
-- than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_session_usage (
    session_id uuid PRIMARY KEY,
    user_id text NOT NULL,
    bot_id uuid NOT NULL,
    input_tokens bigint NOT NULL CHECK (input_tokens >= 0),
    output_tokens bigint NOT NULL CHECK (output_tokens >= 0),
    cache_read_tokens bigint NOT NULL CHECK (cache_read_tokens >= 0),
    cache_write_tokens bigint NOT NULL CHECK (cache_write_tokens >= 0),
    cost_usd double precision CHECK (cost_usd >= 0),
    model text CHECK (octet_length(model) BETWEEN 1 AND 1024),
    recorded_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE factory_step_used_session ADD COLUMN bot_id uuid;

UPDATE factory_step_used_session u
SET bot_id = s.bot_id
FROM factory_step s
WHERE s.run_id = u.run_id AND s.seq = u.seq AND s.session_id = u.session_id
    AND s.bot_id IS NOT NULL;
