-- Flaky CI reruns of the software factory (crates/factory_records): when
-- every job that turned a Run's pull request red is a known flake, the
-- `ci` Step reruns those jobs once on GitHub before any CI fix, under the
-- Standing Rule `rerun_flaky_ci`. Each rerun is kept with how it ended, and
-- a job whose reruns passed often enough in a repository is quarantined:
-- a task is filed for it, and Insights lists it.
--
-- The Standing Rules' checks are swapped to allow `rerun_flaky_ci`; each
-- swap scans its small table under its ACCESS EXCLUSIVE lock, briefly.
-- The Runs' new column is nullable, so no row is rewritten. The new tables
-- reference factory_step and factory_run, which takes a SHARE ROW
-- EXCLUSIVE lock on them until the transaction ends. Give up rather than
-- queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project', 'definition')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates',
            'rerun_flaky_ci'
        ]
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates',
            'rerun_flaky_ci'
        ]
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates',
            'rerun_flaky_ci'
        ]
    );

-- The CI jobs the Run's Factory Definition names as known flakes
-- (`knownFlakes` in its `rules.json`), as it read them when the Run
-- started. Null for a Run whose Definition names none, or that started
-- under none.
ALTER TABLE factory_run
    ADD COLUMN known_ci_flakes text[] CHECK (
        cardinality(known_ci_flakes) BETWEEN 1 AND 100
    );

-- One failed CI job a `ci` Step reran, once per Step and job. `job_id` is
-- the GitHub Actions job that failed, so a read can tell its rerun from
-- it. `matched` says why it was taken for flaky: the Definition names it
-- (`known`), it is quarantined (`quarantined`), or the same failure
-- happened on other pull requests (`record`). `result` is null until the
-- rerun ends: `passed`, `failed`, or `refused` when GitHub would not
-- rerun it. A rerun goes with its Step.
CREATE TABLE factory_ci_retry (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 1024),
    head_sha text NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}([0-9a-f]{24})?$'),
    job_id bigint NOT NULL CHECK (job_id > 0),
    failed_step text CHECK (octet_length(failed_step) BETWEEN 1 AND 1024),
    failure text CHECK (octet_length(failure) BETWEEN 1 AND 1024),
    matched text NOT NULL CHECK (matched IN ('known', 'quarantined', 'record')),
    rule text NOT NULL CHECK (rule = 'rerun_flaky_ci'),
    result text CHECK (result IN ('passed', 'failed', 'refused')),
    requested_at timestamptz NOT NULL DEFAULT now(),
    settled_at timestamptz,
    CONSTRAINT factory_ci_retry_settled CHECK ((result IS NULL) = (settled_at IS NULL)),
    PRIMARY KEY (run_id, seq, name),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);

-- A CI job quarantined in a repository (its URL in lower case): its
-- reruns passed often enough that it counts as flaky from then on, and a
-- `ci` Step reruns it as a known flake. The Run whose rerun crossed the
-- threshold files a task for it as its owner, with the id `task_id`, kept
-- before the task is made so filing it again makes no second one; `filed`
-- once it is made. One quarantine per job and repository.
CREATE TABLE factory_ci_quarantine (
    repo_url text NOT NULL CHECK (
        length(repo_url) <= 300
        AND repo_url ~ '^https://github\.com/[a-z0-9_.-]+/[a-z0-9_.-]+$'
    ),
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 1024),
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    passed_retries integer NOT NULL CHECK (passed_retries > 0),
    task_id uuid NOT NULL UNIQUE,
    filed boolean NOT NULL DEFAULT false,
    quarantined_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_url, name)
);

CREATE INDEX factory_ci_quarantine_run_idx ON factory_ci_quarantine (run_id);
