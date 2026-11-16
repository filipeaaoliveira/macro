-- Every factory Run belongs to the user who started it (crates/factory_records):
-- its agent Steps run as them, and the tasks it works on are checked against
-- what they can see. A Run goes with its owner's account.
--
-- Runs from before they had owners were local trials of a feature no
-- environment runs yet, so they are removed rather than kept ownerless.
DELETE FROM factory_run;

ALTER TABLE factory_run
    ADD COLUMN user_id text NOT NULL REFERENCES "User" (id) ON DELETE CASCADE;

-- Deleting a user finds their Runs without scanning every Run.
CREATE INDEX factory_run_user_id_idx ON factory_run (user_id);
