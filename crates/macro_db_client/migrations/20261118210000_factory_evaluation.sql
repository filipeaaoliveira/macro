-- How the software factory's Runs are judged and compared
-- (crates/factory_records): Scorers' judgments of a sample of finished
-- Runs, the arm of each Experiment a Run was assigned, and Benchmark suites,
-- which replay frozen tasks as trial Runs on one configuration and record
-- each trial's score and tokens.
--
-- The new tables reference factory_run and "User", so creating them takes a
-- SHARE ROW EXCLUSIVE lock on both until this transaction ends. No existing
-- row is rewritten. Give up rather than queue behind a long transaction on
-- either.
SET LOCAL lock_timeout = '5s';

-- A Scorer's judgment of one finished Run, on its one dimension: a score
-- from 0 to 1, why, and the model that judged. A Run is judged at most once
-- per Scorer, so a retried judge records nothing new. Benchmark trials are
-- never judged here (factory_benchmark_trial_score keeps theirs), so these
-- rows are the live Scorers' numbers alone.
CREATE TABLE factory_run_score (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    scorer text NOT NULL CHECK (scorer ~ '^[a-z0-9_-]{1,64}$'),
    score double precision NOT NULL CHECK (score BETWEEN 0 AND 1),
    reason text NOT NULL CHECK (octet_length(reason) <= 4096),
    model text NOT NULL CHECK (octet_length(model) BETWEEN 1 AND 200),
    judged_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, scorer)
);

-- The arm of an Experiment a Run was assigned when it was created, and the
-- Agents the arm gives the Run's Roles (a JSON object of Role to Agent;
-- empty for the arm that keeps the agent definition's). The Run's Steps
-- read the Agents from here, so a change to the Experiment does not change
-- a Run under way.
CREATE TABLE factory_run_arm (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    experiment text NOT NULL CHECK (experiment ~ '^[a-z0-9_-]{1,64}$'),
    arm text NOT NULL CHECK (arm ~ '^[a-z0-9_-]{1,64}$'),
    agents jsonb NOT NULL CHECK (jsonb_typeof(agents) = 'object'),
    PRIMARY KEY (run_id, experiment)
);

-- A Benchmark suite: one replay of a Benchmark's frozen tasks, started by a
-- person, on one configuration (the Agents it gives Roles, as for an arm).
-- It goes with its starter's account.
CREATE TABLE factory_benchmark_suite (
    id uuid PRIMARY KEY,
    user_id text NOT NULL REFERENCES "User" (id) ON DELETE CASCADE,
    benchmark text NOT NULL CHECK (benchmark ~ '^[a-z0-9_-]{1,64}$'),
    agents jsonb NOT NULL CHECK (jsonb_typeof(agents) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now()
);

-- A person's suites are listed newest first.
CREATE INDEX factory_benchmark_suite_user_created_idx
    ON factory_benchmark_suite (user_id, created_at DESC, id DESC);

-- One trial of a suite: one frozen task replayed as one Run. Once its Run
-- ends, the trial keeps its score (1 when the write Step ended done with its
-- checks green, 0.5 when they stayed red, 0 otherwise) and the tokens its
-- agents used, all set together. Its Run goes with it.
CREATE TABLE factory_benchmark_trial (
    suite_id uuid NOT NULL REFERENCES factory_benchmark_suite (id) ON DELETE CASCADE,
    task text NOT NULL CHECK (task ~ '^[a-z0-9_-]{1,64}$'),
    run_id uuid NOT NULL UNIQUE REFERENCES factory_run (id) ON DELETE CASCADE,
    score double precision CHECK (score BETWEEN 0 AND 1),
    input_tokens bigint CHECK (input_tokens >= 0),
    output_tokens bigint CHECK (output_tokens >= 0),
    cache_read_tokens bigint CHECK (cache_read_tokens >= 0),
    cache_write_tokens bigint CHECK (cache_write_tokens >= 0),
    scored_at timestamptz,
    CONSTRAINT factory_benchmark_trial_result CHECK (
        (score IS NULL) = (scored_at IS NULL)
        AND (input_tokens IS NULL) = (scored_at IS NULL)
        AND (output_tokens IS NULL) = (scored_at IS NULL)
        AND (cache_read_tokens IS NULL) = (scored_at IS NULL)
        AND (cache_write_tokens IS NULL) = (scored_at IS NULL)
    ),
    PRIMARY KEY (suite_id, task)
);

-- A Scorer's judgment of one trial, kept with the trial, apart from the live
-- Scorers' judgments (factory_run_score), so a benchmark never moves them.
CREATE TABLE factory_benchmark_trial_score (
    suite_id uuid NOT NULL,
    task text NOT NULL,
    scorer text NOT NULL CHECK (scorer ~ '^[a-z0-9_-]{1,64}$'),
    score double precision NOT NULL CHECK (score BETWEEN 0 AND 1),
    reason text NOT NULL CHECK (octet_length(reason) <= 4096),
    model text NOT NULL CHECK (octet_length(model) BETWEEN 1 AND 200),
    judged_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (suite_id, task, scorer),
    FOREIGN KEY (suite_id, task)
        REFERENCES factory_benchmark_trial (suite_id, task) ON DELETE CASCADE
);
