-- The review rounds of the factory's review panel (crates/factory_records):
-- several reviewers review the run's change at once, each in a Step of its
-- own, and hand back the Blockers they found. A round records the commit it
-- reviews and the diff under review: the run's change in its first round,
-- and the changes the fix made since the round before (the fix diff) in a
-- later one.
--
-- The new tables reference factory_run and factory_step, so this takes locks
-- on them until the transaction ends. Give up rather than queue behind a
-- long transaction on either.
SET LOCAL lock_timeout = '5s';

-- One review round, keyed by the place of its first Step: the round is
-- recorded before its Steps start, and its Steps take the places that
-- follow, one per reviewer. A round goes with its Run.
CREATE TABLE factory_review_round (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    seq integer NOT NULL CHECK (seq >= 1),
    -- Which round of the panel's entry, from 1; a reset counts from 1 again.
    round integer NOT NULL CHECK (round >= 1),
    reviewers integer NOT NULL CHECK (reviewers >= 1),
    -- The commit of the run's branch the round reviews.
    head_sha text NOT NULL CHECK (octet_length(head_sha) BETWEEN 1 AND 64),
    -- Where the diff under review starts: the run's base branch, or the
    -- commit the round before reviewed when the round reviews a fix diff.
    diff_base text NOT NULL CHECK (octet_length(diff_base) BETWEEN 1 AND 255),
    fix_diff boolean NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now(),
    -- pass: no Blocker in the diff under review; changes: one or more;
    -- failed: a reviewer's work failed. Both are null while it reviews.
    outcome text CHECK (outcome IN ('pass', 'changes', 'failed')),
    ended_at timestamptz,
    CONSTRAINT factory_review_round_end CHECK ((outcome IS NULL) = (ended_at IS NULL)),
    PRIMARY KEY (run_id, seq)
);

-- What one reviewer found, at its place in what it handed back: the file,
-- the lines, what is wrong and why. Once its round ends, a finding in the
-- diff under review is a Blocker, and one outside it a follow-up, which
-- becomes a task of its own (`follow_up_task_id`) and does not hold the
-- pull request. A finding goes with its Step and its round.
CREATE TABLE factory_review_finding (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    place integer NOT NULL CHECK (place >= 1),
    round_seq integer NOT NULL,
    path text NOT NULL CHECK (octet_length(path) BETWEEN 1 AND 1024),
    start_line integer CHECK (start_line >= 1),
    end_line integer,
    problem text NOT NULL CHECK (octet_length(problem) BETWEEN 1 AND 4096),
    why text NOT NULL CHECK (octet_length(why) <= 4096),
    kind text CHECK (kind IN ('blocker', 'follow_up')),
    follow_up_task_id text REFERENCES "Document" (id) ON DELETE SET NULL,
    CONSTRAINT factory_review_finding_lines CHECK (
        (start_line IS NULL) = (end_line IS NULL) AND (end_line IS NULL OR end_line >= start_line)
    ),
    CONSTRAINT factory_review_finding_task CHECK (follow_up_task_id IS NULL OR kind = 'follow_up'),
    PRIMARY KEY (run_id, seq, place),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE,
    FOREIGN KEY (run_id, round_seq) REFERENCES factory_review_round (run_id, seq) ON DELETE CASCADE
);

-- A round's findings are read by the round.
CREATE INDEX factory_review_finding_round_idx ON factory_review_finding (run_id, round_seq);
