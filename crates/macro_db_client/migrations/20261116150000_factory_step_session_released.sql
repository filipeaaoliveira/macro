-- When a factory Step let go of its agent session: stopped it for good, or
-- deleted it once the Step's end was recorded. An attempt of the Step still
-- running after that, such as one Temporal timed out, must not open the
-- session again under its id. Null while the Step holds its session.
--
-- factory_step is new and small; the column has no default, so adding it
-- rewrites nothing.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN session_released_at timestamptz,
    ADD CONSTRAINT factory_step_session_released CHECK (
        session_released_at IS NULL OR session_id IS NOT NULL
    );
