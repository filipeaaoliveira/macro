-- Whether the owner commented on a follow-up task that a finding of a later
-- review round joined (crates/factory_records): the comment says the problem
-- was reported again. It is set once the comment is made, so a retried close
-- of the round makes a missing comment and never a second one.
--
-- A column with a constant default rewrites no rows. Adding it takes
-- factory_review_finding's ACCESS EXCLUSIVE lock for a moment: give up
-- rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_review_finding
    ADD COLUMN follow_up_commented boolean NOT NULL DEFAULT false;
