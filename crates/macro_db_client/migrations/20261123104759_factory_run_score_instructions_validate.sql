-- Validate the check on factory_run_score.instructions_sha256 added NOT
-- VALID in factory_scorer_label. Every existing row is null, so it holds;
-- validating takes only a SHARE UPDATE EXCLUSIVE lock, so writes go on.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run_score VALIDATE CONSTRAINT factory_run_score_instructions_sha256;
