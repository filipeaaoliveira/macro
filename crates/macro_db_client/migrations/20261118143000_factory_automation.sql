-- Automations of the software factory (crates/factory_records): a team's
-- standing orders to start a Run of a Flow when something happens, on a
-- schedule, on a repository's GitHub event, on a signed webhook call or on
-- a Task event. An Automation acts as the person who created it, and a Run
-- it starts keeps which Automation it was.
--
-- The new table references team and "User", and the new columns and index
-- change factory_run, so this takes locks on them until the transaction
-- ends. Adding nullable columns rewrites no rows; building the index scans
-- factory_run under a SHARE lock, which this small table holds briefly.
-- Give up rather than queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

-- An Automation belongs to its team and goes with it, and with the account
-- of the person it acts as. Its trigger is the kind's own settings, as
-- `factory_records`' `AutomationTrigger` keeps them. A schedule, a
-- repository event and a Task event are a Macro routine of its creator's,
-- which fires it (`routine_id`, one routine for one Automation); a webhook
-- is fired by a call signed with its own secret.
CREATE TABLE factory_automation (
    id uuid PRIMARY KEY,
    team_id uuid NOT NULL REFERENCES team (id) ON DELETE CASCADE,
    created_by text NOT NULL REFERENCES "User" (id) ON DELETE CASCADE,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    trigger_kind text NOT NULL
        CHECK (trigger_kind IN ('schedule', 'repository_event', 'webhook', 'task_event')),
    trigger jsonb NOT NULL,
    flow text NOT NULL CHECK (flow ~ '^[a-z0-9_-]{1,64}$'),
    repo_url text CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    request_template text NOT NULL CHECK (octet_length(request_template) BETWEEN 1 AND 65536),
    enabled boolean NOT NULL DEFAULT true,
    routine_id uuid UNIQUE,
    webhook_secret text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT factory_automation_webhook CHECK (
        (trigger_kind = 'webhook') = (webhook_secret IS NOT NULL)
        AND (trigger_kind <> 'webhook' OR routine_id IS NULL)
    )
);

-- A team's Automations are listed newest first.
CREATE INDEX factory_automation_team_idx
    ON factory_automation (team_id, created_at DESC, id DESC);

-- The creator's column is indexed so deleting an account finds their
-- Automations without a full scan.
CREATE INDEX factory_automation_created_by_idx ON factory_automation (created_by);

-- Each time an Automation fires, keyed by what fired it: the routine run's
-- id, or the SHA-256 of a webhook call's signed timestamp and body.
-- A second firing with the same key is a retry or a replay and starts
-- nothing. It keeps the Run it started, or why it started none.
CREATE TABLE factory_automation_firing (
    automation_id uuid NOT NULL REFERENCES factory_automation (id) ON DELETE CASCADE,
    firing_key text NOT NULL CHECK (octet_length(firing_key) BETWEEN 1 AND 200),
    fired_at timestamptz NOT NULL DEFAULT now(),
    run_id uuid REFERENCES factory_run (id) ON DELETE SET NULL,
    skipped text CHECK (
        skipped IN (
            'disabled', 'task_busy', 'queue_full', 'refused', 'own_branch', 'creator_left_team',
            'agent_account'
        )
    ),
    PRIMARY KEY (automation_id, firing_key)
);

CREATE INDEX factory_automation_firing_run_idx
    ON factory_automation_firing (run_id)
    WHERE run_id IS NOT NULL;

-- The Automation that started the Run, with its name and trigger kind as
-- they were then, and the key of the firing that started it: a record, not
-- a reference, so it outlives the Automation. All null for a Run a person
-- started.
ALTER TABLE factory_run
    ADD COLUMN automation_id uuid,
    ADD COLUMN automation_name text,
    ADD COLUMN automation_trigger text,
    ADD COLUMN automation_firing_key text;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_automation CHECK (
        (automation_id IS NULL) = (automation_name IS NULL)
        AND (automation_id IS NULL) = (automation_trigger IS NULL)
        AND (automation_id IS NULL) = (automation_firing_key IS NULL)
        AND automation_trigger IN ('schedule', 'repository_event', 'webhook', 'task_event')
    );

-- An Automation never starts a second Run on a task while one it started
-- there runs: two firings that race both check for a running Run, and this
-- index refuses the second insert.
CREATE UNIQUE INDEX factory_run_automation_task_open_idx
    ON factory_run (task_id)
    WHERE automation_id IS NOT NULL AND ended_at IS NULL AND task_id IS NOT NULL;

-- A firing starts at most one Run: one taken over after its worker stopped
-- finds the Run the first started, and a second insert is refused.
CREATE UNIQUE INDEX factory_run_automation_firing_idx
    ON factory_run (automation_id, automation_firing_key)
    WHERE automation_id IS NOT NULL;
