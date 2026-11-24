-- The child tasks a `split` Step filed (crates/factory_records,
-- `record_split_task`): when a Run's plan finds its task too big for one
-- Run, the Step files each child task as the Run's owner, and keeps its id
-- and name here, so the Run's read lists them without asking document
-- storage. `place` is the child's place in the plan, from 0, which with the
-- Run and the Step fixes the task's id: a retried Step keeps each child
-- once. A child document storage refused is not kept. The children go with
-- their Step.
--
-- The new table references factory_step, which takes a SHARE ROW EXCLUSIVE
-- lock on it until this transaction ends. Give up rather than queue behind
-- a long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_split_task (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    place integer NOT NULL CHECK (place BETWEEN 0 AND 99),
    task_id uuid NOT NULL,
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 200),
    PRIMARY KEY (run_id, seq, place),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);
