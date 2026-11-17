-- The factory's Main Hold (crates/factory_records): the pull requests Runs
-- merged, and the base branches whose required checks failed after one did,
-- which no Run merges to until they pass again.
--
-- Both tables are new; the task a hold files references "Document", so
-- creating it takes a SHARE ROW EXCLUSIVE lock on "Document" until this
-- transaction ends. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- A pull request a Run merged, into the base branch it had then. It goes
-- with its Run. A red base names the merges made since it was last green.
CREATE TABLE factory_merge (
    run_id uuid PRIMARY KEY REFERENCES factory_run (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    base_branch text NOT NULL CHECK (octet_length(base_branch) BETWEEN 1 AND 255),
    pull_request_number bigint NOT NULL CHECK (pull_request_number > 0),
    pull_request_url text NOT NULL CHECK (octet_length(pull_request_url) <= 2048),
    merged_at timestamptz NOT NULL
);

-- A base's merges are read newest first, from when it was last green.
CREATE INDEX factory_merge_base_idx
    ON factory_merge (repo_url, base_branch, merged_at DESC);

-- What the factory last saw of a base branch it merged to: when its
-- required checks last passed; whether they could not be read ('unread')
-- or the branch requires none ('none'); and, while they fail, the hold:
-- since when, on which head, which checks, and the task filed with the
-- suspect merges. The task is claimed, then filed: one red period files one
-- task, and deleting the task document clears task_id but keeps
-- task_filed_at, so it is not filed again. A green base clears the hold.
-- Times are the reads' starts on the workers' clock, but task_filed_at.
CREATE TABLE factory_main_hold (
    repo_url text NOT NULL CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    base_branch text NOT NULL CHECK (octet_length(base_branch) BETWEEN 1 AND 255),
    last_green_at timestamptz,
    required_checks text CHECK (required_checks IN ('unread', 'none')),
    held_since timestamptz,
    head_sha text CHECK (head_sha ~ '^[0-9a-f]{40}$'),
    failed text CHECK (octet_length(failed) <= 65536),
    task_claimed_at timestamptz,
    task_filed_at timestamptz,
    task_id text REFERENCES "Document" (id) ON DELETE SET NULL,
    PRIMARY KEY (repo_url, base_branch),
    CONSTRAINT factory_main_hold_red CHECK (
        (held_since IS NULL) = (head_sha IS NULL)
        AND (held_since IS NULL) = (failed IS NULL)
    ),
    CONSTRAINT factory_main_hold_task CHECK (
        (task_claimed_at IS NULL OR held_since IS NOT NULL)
        AND (task_filed_at IS NULL OR task_claimed_at IS NOT NULL)
        AND (task_id IS NULL OR task_filed_at IS NOT NULL)
    )
);

-- A deleted task document clears its hold's task_id through this index.
CREATE INDEX factory_main_hold_task_idx
    ON factory_main_hold (task_id)
    WHERE task_id IS NOT NULL;
