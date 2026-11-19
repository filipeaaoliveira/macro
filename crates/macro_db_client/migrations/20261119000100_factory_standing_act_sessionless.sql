-- A standing act may come from a bot's code that runs in no agent session
-- (a code-mode call or a routine's run signed for the bot): it keeps the
-- bot, with no session. Dropping NOT NULL only changes the catalog: no row
-- is read or rewritten.
ALTER TABLE factory_standing_act ALTER COLUMN session_id DROP NOT NULL;
