-- A Run that works on code works on one GitHub repository, from the branch
-- it had as default when the Run started. When it asks a person to merge its
-- pull request, it keeps the commit it showed them, so only that commit
-- merges once they agree. All three are null for a Run that works on no
-- repository.
--
-- Adding nullable columns with no default rewrites nothing; the lock is
-- brief, but give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN repo_url text CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    ADD COLUMN base_branch text CHECK (octet_length(base_branch) BETWEEN 1 AND 255),
    ADD COLUMN merge_head_sha text CHECK (merge_head_sha ~ '^[0-9a-f]{40}$'),
    ADD CONSTRAINT factory_run_repository CHECK (
        (repo_url IS NULL) = (base_branch IS NULL)
        AND (merge_head_sha IS NULL OR repo_url IS NOT NULL)
    );
