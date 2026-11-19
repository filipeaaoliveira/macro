-- The factory's merge queue (crates/factory_records) orders a base's Runs by
-- a serial taken as each first reached its merge, so two that reached it in
-- the same instant keep the order they entered in, and keys a base by its
-- repository URL lower-cased, trimmed and without a `.git` ending, as GitHub
-- names one repository whatever the case.
--
-- factory_merge_queue is new and holds a handful of rows at most: adding the
-- identity column rewrites it, and its index is rebuilt, both at once.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_merge_queue
    ADD COLUMN place bigint GENERATED ALWAYS AS IDENTITY;

UPDATE factory_merge_queue
SET repo_url = lower(regexp_replace(btrim(repo_url), '\.git$', ''))
WHERE repo_url <> lower(regexp_replace(btrim(repo_url), '\.git$', ''));

DROP INDEX factory_merge_queue_base_idx;

-- A base's queue is read in order.
CREATE INDEX factory_merge_queue_base_idx
    ON factory_merge_queue (repo_url, base_branch, place);
