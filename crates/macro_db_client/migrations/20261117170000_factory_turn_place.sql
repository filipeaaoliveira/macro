-- The order of the later prompts of a factory Step's session, and the rounds
-- of checks the agent's quota ended (crates/factory_records).
--
-- A Step's later prompts each have a place the Step gives them: a round's
-- checks come before its fix, and its fix before the next round's checks.
-- The place, not the prompt's id, which the worker that asks makes on its
-- own clock, tells a later prompt from an earlier one.
--
-- The new columns change factory_step and factory_check_round, so this
-- takes locks on them until the transaction ends. Give up rather than queue
-- behind a long transaction on either.
SET LOCAL lock_timeout = '5s';

-- The place of the later prompt the Step works on now; null while its first
-- prompt's turn is the latest.
ALTER TABLE factory_step
    ADD COLUMN turn_place integer CHECK (turn_place >= 0);

-- A Step already on a later prompt takes that prompt's place from its round:
-- a round's checks are at twice its number, its fix just after. A prompt no
-- round names is placed first.
UPDATE factory_step s
SET turn_place = COALESCE(
    (
        SELECT c.round * 2
        FROM factory_check_round c
        WHERE c.run_id = s.run_id AND c.seq = s.seq AND c.check_action_id = s.turn_action_id
    ),
    (
        SELECT c.round * 2 + 1
        FROM factory_check_round c
        WHERE c.run_id = s.run_id AND c.seq = s.seq AND c.fix_action_id = s.turn_action_id
    ),
    0
)
WHERE s.turn_action_id IS NOT NULL;

ALTER TABLE factory_step
    ADD CONSTRAINT factory_step_turn_place CHECK ((turn_place IS NULL) = (turn_action_id IS NULL));

-- A round that failed because the agent's quota ran out, during its checks
-- or the fix of their failure: no fix or round follows it, and the Step ends
-- there. Rounds before this have none.
ALTER TABLE factory_check_round
    ADD COLUMN quota_ran_out boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT factory_check_round_quota CHECK (NOT quota_ran_out OR outcome = 'failed');
