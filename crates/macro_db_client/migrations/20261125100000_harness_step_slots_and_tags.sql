-- What a Machine offers the software factory (crates/harnesses,
-- `patch_harness`): how many factory Steps may run on it at once
-- (`step_slots`, null for no limit) and the tags it has (`tags`, such as
-- `rust-build`, `gpu` or `arm64`), which a Factory Definition's Agent may
-- ask of the Machine its Steps run on. Its owner sets both in Settings,
-- Machines. Tags are lower case, unique and checked by the harnesses
-- service; the table bounds only how many a Machine has.
--
-- A nullable column and a column with a constant default rewrite no row; the
-- checks scan harnesses under its ACCESS EXCLUSIVE lock, which a small table
-- holds briefly. Give up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE harnesses
    ADD COLUMN step_slots integer CHECK (step_slots BETWEEN 1 AND 64),
    ADD COLUMN tags text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 16);
