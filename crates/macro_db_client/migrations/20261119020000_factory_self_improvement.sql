-- Self-improvement of the software factory (crates/factory_records): the
-- failures a team's Runs met under its Factory Definition (a Scorer's
-- judgment below the pass score, or a Run marked flawed), each kept until a
-- self-improvement Run's pull request against the Definition that groups it
-- merges; and those self-improvement Runs, at most one open per team.
--
-- Who named a team's Definition is kept too: the self-improvement Run is
-- theirs. A Definition named before this keeps none, so its team starts no
-- such Run until an admin names it again.
--
-- Adding a nullable column rewrites no row. The new tables reference team,
-- factory_run and "User", so this takes a SHARE ROW EXCLUSIVE lock on them
-- until the transaction ends. Give up rather than queue behind a long
-- transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_team_definition
    ADD COLUMN named_by_user_id text REFERENCES "User" (id) ON DELETE SET NULL;

-- A self-improvement Run of a team, on its Definition's repository:
-- `starting` while its Run is recorded and started, `open` once it is
-- (and while its pull request is open after the Run ended), then `merged`
-- when the pull request merged, `closed` when it closed unmerged or the
-- Run ended without one, or `refused` when its Run could not start, with
-- why. The Run goes with the factory_run row; the record stays.
CREATE TABLE factory_improvement (
    id uuid PRIMARY KEY,
    team_id uuid NOT NULL REFERENCES team (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (length(repo_url) <= 300),
    run_id uuid UNIQUE REFERENCES factory_run (id) ON DELETE SET NULL,
    state text NOT NULL CHECK (state IN ('starting', 'open', 'merged', 'closed', 'refused')),
    refusal text CHECK (octet_length(refusal) <= 4096),
    -- How many failures it took when it began, kept after they go back.
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    settled_at timestamptz,
    CONSTRAINT factory_improvement_settled CHECK (
        (state IN ('starting', 'open')) = (settled_at IS NULL)
        AND (refusal IS NOT NULL) = (state = 'refused')
    )
);

-- At most one self-improvement Run of a team is open at a time.
CREATE UNIQUE INDEX factory_improvement_open_idx
    ON factory_improvement (team_id)
    WHERE state IN ('starting', 'open');

-- A team's latest self-improvement Run is read first.
CREATE INDEX factory_improvement_team_created_idx
    ON factory_improvement (team_id, created_at DESC, id DESC);

-- One failure of a Run under its team's Definition: a Scorer's judgment
-- below the pass score (`scorer`, named), or the Run marked flawed
-- (`flawed`, scorer ''). Pending while no self-improvement Run holds it;
-- held by one while it is open; addressed once its pull request merged. A
-- Run that closed unmerged lets its failures go back to pending.
CREATE TABLE factory_improvement_failure (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('scorer', 'flawed')),
    scorer text NOT NULL DEFAULT '' CHECK (scorer = '' OR scorer ~ '^[a-z0-9_-]{1,64}$'),
    team_id uuid NOT NULL REFERENCES team (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (length(repo_url) <= 300),
    score double precision CHECK (score BETWEEN 0 AND 1),
    reason text NOT NULL CHECK (octet_length(reason) <= 4096),
    failed_at timestamptz NOT NULL DEFAULT now(),
    improvement_id uuid REFERENCES factory_improvement (id) ON DELETE CASCADE,
    addressed_at timestamptz,
    PRIMARY KEY (run_id, kind, scorer),
    CONSTRAINT factory_improvement_failure_source CHECK (
        (kind = 'scorer') = (scorer <> '')
        AND (kind = 'scorer') = (score IS NOT NULL)
        AND (addressed_at IS NULL OR improvement_id IS NOT NULL)
    )
);

-- A team's pending failures are counted, oldest first.
CREATE INDEX factory_improvement_failure_pending_idx
    ON factory_improvement_failure (team_id, failed_at)
    WHERE improvement_id IS NULL;

-- A self-improvement Run's failures are read and settled together.
CREATE INDEX factory_improvement_failure_improvement_idx
    ON factory_improvement_failure (improvement_id)
    WHERE improvement_id IS NOT NULL;
