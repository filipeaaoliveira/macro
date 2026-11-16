-- An agent Step of a factory Run works in a Macro agent session
-- (crates/factory_records), and the agent hands its result back from it: an
-- outcome the Step offered, and what it produced. The session is found again
-- from the code-mode request that hands back. factory_step is the factory's
-- own and small, so the new nullable columns and the index lock it briefly.
ALTER TABLE factory_step
    -- The session and its first prompt's action, both caller-made UUIDv7s,
    -- and the outcomes the agent may end the Step with. All null for a Step
    -- no agent does.
    ADD COLUMN session_id uuid UNIQUE,
    ADD COLUMN action_id uuid,
    ADD COLUMN offered text[],
    -- The agent's latest hand back; null until it hands one back.
    ADD COLUMN handed_outcome text,
    ADD COLUMN handed_output text CHECK (octet_length(handed_output) <= 65536),
    ADD COLUMN handed_at timestamptz,
    ADD CONSTRAINT factory_step_session CHECK (
        (session_id IS NULL) = (action_id IS NULL)
        AND (session_id IS NULL) = (offered IS NULL)
    ),
    ADD CONSTRAINT factory_step_hand_back CHECK (
        (handed_outcome IS NULL) = (handed_at IS NULL)
        AND (handed_output IS NULL) = (handed_at IS NULL)
        AND (handed_at IS NULL OR session_id IS NOT NULL)
        AND (handed_outcome IS NULL OR handed_outcome = ANY (offered))
    );
