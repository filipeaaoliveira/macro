-- Each agent Step fills a Role of its Flow with one of the Agents the
-- factory's agent definition lists for it, in order. The Step keeps the Role,
-- the Agent whose session it works in, and the Agents passed over before it
-- because they could not start for lack of quota, each with the reason's
-- code. All are set when the Step claims its session; Steps that claimed one
-- before this have none.
--
-- Adding nullable columns, and ones whose default is a constant, rewrites no
-- rows; the checks scan factory_step under its ACCESS EXCLUSIVE lock, which
-- a small table holds briefly. Give up rather than queue behind a long
-- transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN role text CHECK (role ~ '^[a-z0-9_-]{1,64}$'),
    ADD COLUMN agent text CHECK (agent ~ '^[a-z0-9_-]{1,64}$'),
    ADD COLUMN passed_over text[] NOT NULL DEFAULT '{}',
    ADD COLUMN pass_reasons text[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT factory_step_agent CHECK (
        (role IS NULL) = (agent IS NULL)
        AND (agent IS NULL OR session_id IS NOT NULL)
        AND cardinality(passed_over) = cardinality(pass_reasons)
        AND (cardinality(passed_over) = 0 OR agent IS NOT NULL)
    );
