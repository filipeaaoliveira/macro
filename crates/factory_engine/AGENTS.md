# factory_engine

## What it is

The software factory's engine: runs a Flow, a JSON list of entries whose Step
outcomes say where a Run goes next, as one durable Temporal workflow per Run.

## How it works

`POST /runs` reads its Flow, records the Run (`factory_records`), and starts
`FlowWorkflow` with the Run's id as its workflow id. For each Step the
interpreter names, the workflow records its start (`factory_start_step`), runs
its work (`factory_run_step`), records its end (`factory_end_step`), and feeds
the outcome back; then it records the Run's end (`factory_end_run`). A Step is
keyed by its place among the Run's Steps (`seq`), which a reset never reuses.
Work that fails for good, or still fails after two hours of retries (not a count
of attempts, which a worker shutting down uses up), ends the Step `failed`, with
a fixed output (why is in the worker's logs). The work heartbeats every 2 s
against a 30 s timeout, and the worker holds a heartbeat back at most 4 s, so a
cancel reaches it within about 4 s. A cancelled activity only drops its work: it
can't tell the Run's cancellation from a worker shutting down or a timed-out
attempt, and a retry goes on in the same agent session. Once the Step's or the
Run's end is recorded, the workflow stops the agent (`factory_stop_step`, on a
token the cancellation does not reach) when the Run was cancelled, or the work
failed for good: the stop is sent again for a minute while the session still
works (a prompt on its way can restart it), then the session is deleted. A
failed delete is retried, unless no attempt saw the session open and its last
prepare (recorded at the database's time before each) is over five minutes old:
then the session is not there. A delete that answers a missing session as
deleted is retried too while such a prepare may still open it. A Step that ended
has its session deleted (`factory_release_step`). Either way the Step first
records that it let go of the session. An attempt checks that record before it
prepares and before it prompts, so none starts either after it; one that passed
a check just before can still prepare or prompt, and the stop's watch and
retried delete cover it. A prepare refused outright, or never shown open, is no
longer in flight. A shutting-down worker gives running Steps a few seconds, then
cancels them, so Temporal retries them on another worker at once. A cancelled
Run's open Steps end `cancelled` with it. A Run takes at most `MAX_STEPS` Steps,
then ends `blocked`. Workflow inputs carry ids, never the request's text.

An entry with a `gate` asks a person a question once its Step ends `done`: the
workflow records the Gate (`factory_open_gate`), waits for the
`factory_answer_gate` update that `POST /runs/{id}/steps/{seq}/answer` sends,
records the answer (`factory_answer_gate` activity), and routes the answer as
the entry's outcome, unless a Standing Rule answers it first (below). An
answer the recorded Gate does not offer is refused
before it is sent (400); the update's validator (`domain/gate.rs`) refuses an
answer to a Gate the Run does not wait on and a second answer (409). Only the
Run's owner answers its Gates, in their own signed-in session: an internal
caller, a user API key or Macro API token, and an agent's program acting for
them are refused (403).

A Gate the Flow marks `notify_only` (`GateDefinition::notify_only`) only
tells the person: it offers no answers, has no kind and is not
`person_only` (`FlowError::NotifyOnlyGateAnswers`), and once its Step (or
its review round) ends with the outcome that opens it, the workflow calls
`factory_tell_gate` (behind the `factory-notify-only-gates` patch marker;
tried for at most five minutes, its failure ignored) and routes that
outcome, so the entry's `on` routes `done` (or the round's outcome) as if
there were no Gate. No Gate is recorded: the Run never waits on it, is not
listed as waiting, and insights count none. The worker tells the owner
with the waiting Gate's notification kind (`factory_gate_waiting`,
flagged `notifyOnly`, so its title and the inbox say the Run goes on
rather than waits on an answer), lighter:
not pushed to their phone, one per entry and Run however often its Gate
tells (`gate_owner::told_id`), and marked done in `factory_end_run`
(no record says which told, so each visited entry whose Gate the Run's
Flow makes notify-only: `gate_owner::told_entries`). It is about the Run's
task, or, for a Run started without one, filed under the owner's own User
as a waiting Gate's is (see below). It is sent whatever the definition
says, since the Gate exists only to tell the owner
(`gate_owner::gate_told`); the worker also posts the notice to the task
thread when that section is on. No built-in
Flow has one.

An agent may ask the Run's owner a question mid-Step (`src/domain/questions.rs`)
when its Flow says how many (`questions`, which the service writes into the
Flow a Run starts with: `FACTORY_QUESTIONS`, two by default; zero offers
none). Its Step is offered `question` and told how to ask, and how many
questions it has left (`questions/ask.md`); handing it back,
with the question as its text (1 to 4096 bytes, else 400), ends the Step
`question`, which keeps its session, and the workflow opens a question Gate
(`GateKind::Question`, read from the Step's outcome: its one answer is
`answer`, and the owner's words are its note, which must not be blank, else
400). The answer is never routed: the same Role goes on in a Step of its own,
at the next place and the same visit (`Interpreter::resume`, counted against
`MAX_STEPS`), which claims the asking Step's session under a prompt of its
own and prompts it with the answer and how many questions are left
(`StepRequest::resumes`, `questions_left`, `SessionAgent::go_on`); no other
Agent takes it over. The asking Steps stay held until the Step that goes on
from them ends (`inbound/temporal/workflow/step.rs`): it claims their
session only once its work starts, so a Run cancelled, or work that fails,
before that still has the session stopped. While the Run waits on the
owner the harness may let the session go: it releases an idle session from
memory (`AGENT_SESSION_IDLE_TIMEOUT_SECS`, 15 minutes) and stops an idle
Daytona sandbox (5 minutes), and the next prompt starts the sandbox and
restores the session (`session/resume` or `session/load`). A session that
cannot be restored (its runtime still disconnected 5 minutes after the
prompt, `RESUME_SETTLE`; the prompt never shown; or no history to go on
from) is lost: it is deleted, its Agent recorded passed over as
`session_lost`, and the Step goes on in a new session of its Role's first
Agent, prompted with the whole Step afresh, the owner's answers in it, and
told that what the lost session did not push is gone. Every later agent
Step's prompt carries the Run's answered questions in a block marked as the
owner's answers, as data, escaped as reviewers' findings are
(`answers_brief`). A Step's work asks at most the Flow's `questions`: the next
question ends the Run `manual`, the question in the Step's output, and its
session is stopped. Insights count a question and the Step that goes on
from it as one visit, and leave the question, its Gate and the answer out
of the entry's outcomes, answers and Gates. A reviewer's question works the same way: once every
reviewer of the round ended, each question goes to the owner in the entry's
order, the reviewer goes on with the answer (a Step outside the round's first
places, its findings recorded on the round), and the round closes on every
reviewer's result; a reviewer past its questions closes the round failed and
ends the Run `manual`. Only the owner answers a question Gate, in person, as
any Gate: whatever answers Gates for a person must leave it alone
(`GateKind::answered_only_by_a_person`, which the answer path checks). No Flow's Gate
opens `after` `question` (`FlowError::GateAfterQuestion`). A cancelled Run, or one that ends
while a Step holds its session for a question, has that session stopped.

An agent whose turn ends with nothing handed back is reminded once
(`session_agent/reminder.rs`): a later prompt in the same session
(`session_agent/remind.md`), at place 1 of the Step's later prompts, under
an id made from the first prompt's, so a retry records and sends the same
one, which the session refuses as one it holds, and waits on it. A
reminder that ends with nothing handed back too is the agent stopping
without a result (`StoppedWithoutResult`), with its last message, read
from the session (`RoutineSessions::reply`): what it said in the
reminder's turn, else in its first turn. Its Step ends `no_result`
(`src/domain/no_result.rs`), that message its output, and keeps its
session; the workflow (behind the `factory-no-result-gates` patch marker)
opens a Gate at once, of no kind, that shows the message and offers
`retry` and `stop`, and tells the owner as any Gate left to them. No
Standing Rule or stand-in answers it, and the service refuses an agent's
answer to it (`PersonOnly::PersonsGate`); the owner's words are read to
one of them, and it is no past answer the stand-in reads. On
`retry` the same Role goes on in a Step of its own, at the next place, in
that session, told the owner asks for the result
(`no_result::retry_prompt`), as a question's answer goes on; on `stop` its
agent is stopped and the Flow routes `failed`, as a Step whose work failed,
while the Step stays `no_result`; the entry's own Gate, even one `after`
`failed`, does not open, since the Step had its Gate. Insights count a
retried Step and the one that goes on from it as one visit, a stopped one
as `failed`, and neither Gate nor its answer.
A reviewer's Step that stops so fails, as before: its round asks no one. A
Step that ends `no_result` without the marker fails the same way. The task
thread names the Role that stopped, not its words. No Flow's Gate opens
`after` `no_result` (`FlowError::GateAfterNoResult`).
The reminder is a prompt of the factory's: it is sent out of sight, opens
the Step Terminal's steering again (`begin_turn`), and its wait follows the
owner's note, pause, resume and stop as any prompt's does; the agent's last
message is read from the last turn the wait followed, an owner's note's
when there was one. The reminder's wait keeps the time the Step was
paused before it, and one still going when the attempt's time runs out is
`Overdue`, which a retry waits on. A wait closes the steering only of the
prompt it waits on (`close_steering` names it): an attempt retried late,
once the Step went on to the reminder, waits on the reminder rather than
the first prompt, and never closes the reminder's steering. A Step its
owner stops ends `failed` (`StoppedByOwner`), never `no_result`, as does
one whose stop is recorded before its reminder is sent; one whose pause
ran out takes no more steering (409). A verifier (`accept`)
stopping without a result ends its Step `no_result` as any agent does,
before any acceptance check runs, so its Gate is the only one the Step
opens: never `accept`'s Blockers Gate too. The Step that goes on from a
`retry` is the same visit, so a Blockers Gate it then opens counts the
fixes of the visits before it, not the retried Step.

People call the Run API with their own login (`UserOrInternal`), and internal
services for an acting user (the internal key's identity headers). A Run starts
only of a Flow by name, its team's Factory Definition's or a built-in one (below):
no caller sends a Flow definition. The acting
user owns the Runs they create, and reads, answers, links and cancels only
those; someone else's Run reads as missing (404). A call with no user is
refused (401), and one whose user has no account (400). A Run may work on a
Macro task: `POST /runs` takes an optional `taskId` (the task document's id),
and `PUT /runs/{id}/task` links or relinks one. The owner must see the document
through entity access, and document storage must say it is a task that is not
deleted (`outbound/document_task_directory.rs`); a hidden document and a missing
one are both "no task" (400). `GET /tasks/{task_id}/runs` lists the caller's own
Runs on a task they can see, newest first, at most `TASK_RUNS_LISTED`; other
people's Runs on it are not listed, and a hidden or missing task, or a document
that is not a task, is 404.

The Orchestrator, a person's standing agent in Macro chat, uses the same
Run API from code mode (the SDK's `macro.factory`, its instructions in
`system_skills`' Orchestrate Factory Runs), as the person and with their
access only. It starts, reads, links and cancels their Runs as they would.
`POST /runs/{id}/steps/{seq}/answer` takes an answer from a code-mode call
of a chat's agent session (the correlation layer's `current_session`, from
a person's credential) as `Answerer::Agent`; a routine's run (signed for it
or carrying its id) and a background run held to a scope are refused
(`PersonOnly::NotAChat`), as is an internal caller. The service
(`agent_may_answer`) then takes an answer only to the Gate the agent names,
while it still waits (409 once answered), only a Flow's Gate whose entry
asks the same kind of Gate (`plan`, `blockers`, `merge`) in every built-in
Flow that has it and that is not kept for a person, and never from a
session a factory Step works in. Sending work back (`revise`, `stop`) is
always the agent's to give. The approving answer is not: never `approve`
on a merge Gate, nor on a Tampered Run's plan, nor with Control Files; and
`approve` on a plan or `fix` on Blockers only where the Run's Orchestrator
Definition lets the Orchestrator approve that kind (`orchestrator`, off by
default, kept on the Run with its rules), and for Blockers never past
`rule_fix_limit` fixes in a row (the Step's visit, as the workflow counts
them) nor, with the owner section on, on Blockers found again after a rule's
fix (`repeated_blockers`). Each refusal is 403 `reservedForAPerson`. The
agent's session reaches the workflow on the answer (`AnswerGate::agent_session`,
left out when none, so histories replay as before) and is kept on the Gate
(`agentSessionId`), whose answer then reads as the Orchestrator's and is
left out of the owner's past answers, the replay measure and Insights' Gate
answers. A reply in words and the factory's settings stay the person's own
session's.

Standing Roles (`src/domain/agents/standing.rs`, `src/domain/service/standing.rs`)
narrow which agent does what. A Factory Definition's `agents.json` may have a
`standing` section: each Standing Role names the acts it allows (`start`,
`cancel`, `link`, `answer_plan`, `answer_blockers`, a fixed set: none merges,
and an act that names a merge, or any other, does not read) and the Agents whose
bots fill it. The Run keeps it with its Agents (`definition_agents`);
`FACTORY_AGENTS` takes none (`StandingOutsideDefinition`), as a Run under no
Definition keeps no Agents. The section governs bots. A call not in person
acts as an agent (`acting_agent` for start, cancel and link, an
`ActingAgent`, and for a start through a Benchmark suite or a routine
firing an Automation, `program_agent`, unless the scheduler's proof makes it
the Automation's creator, see Automations; `Answerer::Agent` for an answer).
A schedule or event Automation's firing that Macro's scheduler started
itself needs no Standing Role for the Macro routine bot: it starts its Run
as the creator. Any other run of its routine (Run Now, an edited routine, a
routine writing as its owner) acts as that bot and is checked. Otherwise,
code in an agent session
(the correlation layer's session on a person's credential) acts as that
session, and the service reads its bot (`StandingActRepository::session_bot`,
the owner's sessions only); code in no session whose credential is signed
for a bot (`agent_id`: a self-hosted agent's code-mode call, a routine's run
for an agent) acts as that bot, which the signature names. A program signed
only for the person (their API key or token, a routine running as them with
no agent) is the person acting, which the section does not govern; nor an
internal caller. Public MCP signs the person's own MCP client or API key,
direct or through code mode, for the Macro bot (`MACRO_AI_BOT_ID`) only to
label its posts, so such a credential in no session and no routine is the
person too (`signed_for_the_person`); in an agent session it acts as that
session. A Gate answer still takes an agent session. When the
Definition the Run starts under, or the one it kept, has the section, the
act is taken only when the bot is an Agent of a Standing Role that allows
it; else 403 `notInStandingRole`, naming the act and the Standing Roles that
allow it. A Gate answer is checked after
`agent_may_answer`, by the kind it read: a plan Gate is `answer_plan`, a
Blockers Gate `answer_blockers`, and a merge Gate no act at all, so under the
section an agent's every answer to a merge Gate is refused
(`PersonOnly::Merge`). Without the section, or for the person in person or an
internal caller, nothing changes. The Run keeps who did each act, with its bot,
Agent, Standing Role and session, if any (`factory_standing_act`): a start before its workflow
starts (a start whose record fails is removed as unstarted), a cancel once the
workflow takes it, a link once linked, and an answer in `factory_answer_gate`, from
`AnswerGate::standing` (left out when none, so histories replay as before; the
workflow only hands it on). The Run API lists them as `standingActs`.

`POST /runs/{id}/retry` (`domain/service/retry.rs`) retries a Run of the
caller's that ended `failed`, `infrastructure` or `blocked` (409 otherwise,
and for a Benchmark trial, a self-improvement Run or a Run that kept no
Flow): a new Run of the same task, request, Flow (the one the Run kept),
Factory Definition commit, rules and Agents, whose `retry` names the Run it
retries (`retriedFrom`), the Run whose branch it works on (`branchOf`, the
first of a chain of retries) and the place of its own first Step
(`firstSeq`). It starts at the entry of the Run's latest own Step, at the
first Step of that visit (`retry_start`: the Run's own Steps right before
it at consecutive places with the same entry and visit number, since a
reset, and each retry, counts visits from 1 again; never below a retry's
`firstSeq`). A retry that took no Step of its own is retried where it
started, so a review panel's round or a Step an
agent's question went on from runs again whole; the
Steps before it that ended are kept as the retry's own, at their places,
with their answered Gates, sessions and the review rounds that ended
before it, so the plan, the branch and the Blockers a fix reads are not
made again. Its workflow starts there (`RunStart::start_at`,
`Interpreter::start_at`): visits and facts start afresh, and the retry
takes as many Steps of its own as a new Run. Every Step works on the
branch of `branchOf` (`StepInput::branch`, `AgentTask::branch`), so it
pushes to, reviews and proposes from the retried Run's branch and pull
request; Scorers read its change there too (`ScoringSubject::branch_of`).
Its first own Step's prompt says it takes over a failed Step's checkout,
which may hold that Step's unfinished changes (`RESUMED_RUN_NOTE`). A Run
is retried at most once (`AlreadyRetried`, 409; retry the newest retry
instead); a retry is held to the Queue limit a new Run would be, and an
agent acting for the owner needs a Standing Role that allows `start`. The
retry is created in person only when its owner retried it in person, from
an account that is not an agent's. The kept Steps' sessions count against
its token budget, so a Run its budget stopped stops again; Insights, the
quality view and the arm and Agent Rule tallies count kept Steps, Gates,
sessions and review rounds once, on the Run that made them, and a Run and
its retries as one attempt of their arm (the newest among the Runs
read). Insights counts a Run a retry took over by how it ended and as
`retried` (`OutcomeCounts::retried`). A retry's start writes its Flow as
`retry_flow` (`RunStart`'s `Serialize`), which a worker from before
retries cannot read: its workflow task fails until a newer worker takes
it, rather than walk the retry from its first entry. Each of a retry's
Step requests carries `branch_of`, which such a worker's activities
refuse (`StepRequest` denies unknown fields), so none works on a branch of
the retry's own. Deploy every worker before the API all the same.

Deploy order and rollback: before Standing Roles, `AgentDefinitionFields` is
`deny_unknown_fields`, and an agent Step reads its Run's kept Agents
(`SessionAgent::run_agents`). A worker from before them fails every agent Step
of a Run that kept a `standing` section (`DefinitionAgents`). Roll every
service and worker task onto this code before any team's Definition adds the
section. Once a Run has kept one, rolling back below this code fails that
Run's later agent Steps (a Run keeps the section it started with, so removing
it from the Definition does not help them): remove it, then let the Runs that
kept it end, or cancel them, before rolling back. `factory_standing_act.session_id`
is nullable from `20261119000100_factory_standing_act_sessionless`, with
no down migration: code from before it reads the column as non-null, so a
Run that kept an act from a bot in no session no longer reads there.
Roll back no lower than that code once such an act is kept, or delete
those rows (`session_id IS NULL`) first.

Agent Rules (`src/domain/agents/rules.rs`) choose another first Agent for
some Runs. A Factory Definition's `agents.json` may have a `rules` list:
each rule has a `name`, the `role` it chooses for, what it matches (a `tag`
of the Run's task, the `repository` the Run works on as `owner/name`, or
both, all ignoring case) and the `agent` the Role's Steps start with. Rules
are matched in the order listed, and the first that matches a Role
chooses; a rule matches only what it names, so one naming neither does not
read. `start_run` matches them once, before the workflow starts, beside the
arms (`AgentDefinition::chosen_by_rules`), and keeps the rule that chose
each Role (`factory_run_agent_rule`, `EvaluationRepository::record_agent_rules`);
a Run whose rules are not kept does not start. The task's tags are read
then, as the owner sees them (their own and their team's tag sets,
`TaskDirectory::task_tags`), only when a rule names a tag; tags that cannot
be read are taken as none, so no tag rule matches and the Run starts, as
for its Project, and the Run keeps that they went unread
(`factory_run_task_tags_unread`), which the Run API shows as
`taskTagsUnread`, so a silent default is not read as "no rule matched". The workflow never matches: `SessionAgent` reads the kept
choice with the arms' (`RunAgents::ruled`), so a retry, a replay, a tag
changed later or a changed Definition does not change a Run under way, and
no workflow code changed (no patch marker). The order a Step takes its
Agents in is the arm's choice, then the rule's, then the Role's own, each
once; the Role's own still take the Step on lack of quota, as before, and
a rule's Agent the Run's Definition no longer has is passed over and
logged, as an arm's. An arm comes before a rule because an Experiment
measures its arms on the Runs it splits: a rule that moved an arm's Runs
onto its own Agent would count them as fallen back and compare nothing. So
the start keeps no rule for a Role an arm chose for, and the session order
only keeps a Run that kept both on its arm. A rule is the Definition's
standing default for a kind of task; an arm with no choice for the Role
(an Experiment's control) leaves the rule in place. A Benchmark trial and
a self-improvement Run take no rules, as they take no arms: a trial runs
on its suite's configuration only. A rule's Agent must be one its own
Role lists (`for_role`; `RuleAgentOutsideRole`): tags are set by people,
so a tag edit must never route a Run onto an Agent kept for something
else, such as a bot only a Standing Role lists to answer Gates. A rule
only moves one of the Role's Agents first. Its Agent is also checked where
the Definition is read as an arm's is (`evaluation::check_choices`,
refused as `agents.json: agent rule {name} gives role ...`), so any Agent
an arm could not take is refused for a rule too. A rule naming a Role the
Definition does not fill, two rules of one name, a rule matching the same
Runs for the same Role as an earlier one (`RuleShadowed`, which could
never choose), an empty or too long tag, a repository not `owner/name` as
a Run keeps it (`CodeRepository::parse`: no `.git` ending, no `.` or `..`
segment, no trailing slash), and more than `MAX_AGENT_RULES` rules do not
read, in a Definition and in `factory_definition_check` alike. `FACTORY_AGENTS` takes no rules
(`RulesOutsideDefinition`): the service holds no deployment agents at a
Run's start. The Run API lists the kept rules as `agentRules` (Role,
rule, Agent); the web Run view shows them under the Run's summary.

Deploy order and rollback for Agent Rules: as for Standing Roles, a worker
from before them reads a Run's kept Agents with `deny_unknown_fields`, so
it fails every agent Step of a Run that kept a `rules` list. Roll every
service and worker task onto this code before any team's Definition adds
the list. `factory_run_agent_rule` (`20261120020000_factory_run_agent_rule.sql`)
and `factory_run_task_tags_unread`
(`20261120020100_factory_run_task_tags_unread.sql`) are new tables, so
code from before them ignores them; rolling back leaves
its rows unread, and a Run that kept a `rules` list fails its later agent
Steps on the old worker, as above.

A person's Queue is their Runs waiting on them at a Gate or an agent's
question (`GET /runs/waiting`, Insights' "Waiting on you"). A team's
Orchestrator Definition, or a Project's own, may set a Queue limit
(`queueLimit`, 1 to 1000, else 400; none by default). `POST /runs` counts
the owner's Queue across all their Projects, against the limit of the
rules the Run would start under, and refuses the Run while the owner holds
as many places as the limit or more, so a limit of N lets them hold at
most N (409 `queueFull`, naming the team's or the Project's rules), for
the person, an agent for them and an Automation alike; Runs under way
go on. A place is an open Run at an unanswered Gate the stand-in has not
answered, or an open Run of a Flow with a Gate of its own (`asking_flows`:
`plan`, `code`; not `reply`, which holds one only once it asks a question)
that has not reached a Gate yet, which holds the place it will ask for. The
refusal's message counts both kinds, and `GET /runs/waiting` lists both
(`runs`, `starting`), so the person sees what holds their places. Every
start is kept behind a lock on the owner's Queue, and under a limit the
places are counted in the same transaction (`insert_run_in_queue`,
`RunInsert::QueueFull`), so starts at the same time are counted one after
the other. A Run whose workflow start failed and could not be told to have
started (`forget_unstarted` keeps it) holds its place until it is
cancelled: it is listed among `starting`, and cancelling a Run whose
workflow never started ends it. An Automation's firing on a task meets the
`task_busy` check only once its creator may see the task. A Gate in the seconds before its rule or the stand-in decides counts:
the limit holds the Queue back, not to the exact count.

A team's Orchestrator Definition, or a Project's own, may set a token
budget (`tokenBudget`, 10,000 to 10,000,000,000 tokens, else 400
`InvalidTokenBudget`; a Factory Definition's `rules.json` is checked the
same, `DefinitionError::TokenBudget`; none by default), which
bounds what a Run's agents spend as `MAX_STEPS` bounds its Steps
(`src/domain/token_budget.rs`). A Run keeps the budget with its rules
when it is created (`factory_run.token_budget`), so a change to the
definition, or a task linked later, does not change a Run under way, and
`RunStart::token_budget` tells its workflow to check it. Before each
agent Step (an entry with a `role`, or a review panel, whose reviewers are
checked once, together), and before each Step that goes on from an
agent's question (a reviewer's too) or from the owner's `retry` after
`no_result`, the workflow asks the worker
(`factory_check_budget`, behind the `factory-token-budget` patch marker,
for at most two minutes): the worker reads the sessions of the Run's
Steps (`step_sessions`, each once, those a Step left for a new one, as
when its session was lost after its agent worked in it, included) and
sums what the usage log recorded for them as the Run's owner
(`ai_usage`'s `SessionUsageReader`), as
Insights counts tokens: input, output, and input read from or written to
the prompt cache, all of which the model bills, so a Run's spend is the
one Insights shows for it. A Run that used its budget or more keeps why
(`record_budget_stop`, `BudgetStop::Spent` with what it used) and ends
`blocked` before the Step starts. A usage log that cannot be read is
logged and lets the Step run; the workflow counts such checks in a row
(a check whose activity failed outright counts too) and hands the count
to the next, and the worker ends the Run, as `BudgetStop::Unread`, at
the `MAX_UNREAD_CHECKS`th (3), so at most two agent Steps in a row run
unchecked. A check retried after the stop was kept finds it and stops
again, whatever the log reads then. A Run stopped before a Step that
would go on from a question or a `retry` ends `blocked` with the asking
Step still held, so the workflow stops its session, as at `MAX_STEPS`. The
count of unread checks in a row lives in the workflow's state
(`FlowWorkflow::budget_unread`), across the Run's Steps. Once the Run's end
is recorded, `factory_end_run` tells its owner through the Gate notice's
path (`token_budget::budget_notice`: a `factory_gate_waiting`
notification flagged `budgetStopped`, pushed, one per Run, whose title
says the Run stopped at its token budget and whose text says how much was
used of it, or that the use could not be read; the inbox reads it as
stopped, not waiting, and nothing waits on it), unless the Run's
definition is quiet.

The budget applies only to measured runtimes: what an agent's model calls
recorded in the usage log with its session's id. An agent that records
none (a local or self-hosted runtime, as Codex or Claude Code through
macrod on its owner's own login) reads as 0 used, and its Run is never
stopped; a Run whose Steps ran on both counts only the measured ones' use
against its budget. When a Run has Step sessions and none recorded any usage,
the Run API says so (`tokenBudget.measured` false) and the Run view shows
the budget as not measured. Usage the factory spends outside the Run's
Step sessions is not counted: the Gate model's reading of the owner's
words, the stand-in's answers and the Orchestrator's chat run on other
sessions. A Benchmark's trials are Runs like any other and take the
budget of the rules they start under. A Run with no budget is never
checked: its workflow asks for no marker and calls no activity, so its
history is what it was, and its Steps run as before. The Run API shows a
Run's budget (`tokenBudget`: `budget`, what its agents `used` so far, read
from the usage log when one Run is read, none when it cannot be read or
the Runs are listed, and `stop` when the budget ended it), and the Run
view shows it under the Run's header. The marker is taken only by a Run
with a budget, which only a service with this code starts; a worker from
before it ignores `RunStart::token_budget` and walks such a Run
unchecked, and a newer worker replays that history without the marker.

`GET /insights?from=&to=&timeZone=&flow=` (`inbound/insights.rs`,
`domain/insights.rs`) sums up how the caller's own Runs went over a period of
at most 366 days in a time zone: Runs by day, outcome and Flow, Run and Step
durations, Gate waits and answers, the entries that failed or were revisited
with their newest Runs, and the tokens each agent Step's sessions used
(its current one and those it left for a new one, the same sessions the
token budget counts), read from the usage log by session id
(`ai_usage`'s `SessionUsageReader`). A Step counts once in the agent and
measured Steps however many sessions it worked in (`StepSessionRef::seq`),
and is measured when any of them recorded usage; a session counts its
tokens once for its entry in its Run, however many Steps shared it. It
reads the newest `INSIGHTS_RUN_CAP` Runs of the period. A bad period, time
zone or Flow name is 400.

`GET /insights/quality?from=&to=&timeZone=&flow=&repository=`
(`domain/quality.rs`) reads the same Runs, of one repository
(`owner/name` or its URL) or of all, for CI health, review quality, cost per
merged pull request and the autonomy rate. The autonomy rate is the share
of Runs that merged with no person stepping in (no Gate a person answered,
no Step they steered, no manual end) and were not found flawed, of the
Runs whose work ended on its own: as the success rate does, it leaves out
Runs a person cancelled and Runs that ended `infrastructure`. Each wait of
`ci` that ends `green`, `red` or `timeout` keeps the jobs that finished on
its head
(`PullRequests::finished_jobs`, `RunRepository::record_ci_wait`): passed,
failed, cancelled, or `setup` when the step that failed sets the job up
(`SETUP_STEPS`: GitHub Actions' own, checkout, cache, installing
dependencies or a known toolchain, never a step named "Setup" that runs
the pull request's work) or GitHub Actions says, at the start of a line, that it
could not download or resolve an action (`SETUP_ERRORS`); a bare `404` or
network error is a test's own output. A failed job keeps the first test its
check run annotated as failed, as `path: title`, at most 200 bytes (never
its message, which is log output). Beside them the wait keeps,
marked replaced, the runs cancelled on a head a newer push replaced: earlier
heads of the wait, and the head of the Run's last wait (`last_ci_wait`),
less the jobs that wait kept there (a matrix's fail-fast cancels before
any push), at most two, read without details. Keeping it is best effort and never fails
the Step: all of it is bounded to 25 s (`CI_RECORD_BOUND`, the jobs' reads
to 20 s), at most six reads of failed jobs' steps and annotations, and none
once the keeping would end past the wait's deadline. A retry that reads
another head or verdict replaces the kept wait. Insights then class a
failed job as flaky when the same test, or a step named for something
narrower than the job's whole work (not "Run tests" or a bare command),
failed in that job on two or more pull requests of the period, else as its
pull request's own, and list those that did, newest first. They measure a pull request's first wait to its first green one,
count red waits as CI fix rounds and review rounds that ended `changes` as
review fix rounds, count rounds per Run, the share clean after the first
round, Blockers by reviewer Role and by Agent, repeated Blockers (the same
text an earlier round of the Run found) and texts found in two or more
Runs. A Run merged with no person stepping in when it merged and ended
with no Gate a person answered (not a Standing Rule, the stand-in or an
agent), no Step its owner steered, and no `manual` end. Cost per merged
pull request is every Run's usage (the sessions its token budget counts)
over the merged ones: in dollars when every model call was priced, in
tokens otherwise, and neither while a Run of the period has an agent
session that recorded no usage (a Step on a runtime its owner signed in
to, as Codex or Claude Code through macrod), whose spend is unknown, not
nothing. A session that recorded none, of a bot another session of the
period recorded usage for (its Step's `bot_id`), made no model call, as
one that failed before its first: it cost nothing and does not withhold
the figure. A session a Step left for a new one has no known bot (the Step
keeps only its current Agent's), so one that recorded none withholds the
figure. A repository that names none is 400.

Evaluation (`src/domain/evaluation.rs`, `evaluation/service.rs`,
`evaluation/insights.rs`, `inbound/evaluation.rs`): an
`EvaluationDefinition` names Scorers, Experiments and Benchmarks. A team
that names a Factory Definition takes its `scorers.json` (the same JSON,
checked against the Definition's own Flows and Agents when it is read);
any other team takes the deployment's (`FACTORY_EVALUATION`, checked
against the built-in Flows and `FACTORY_AGENTS` at start). A Run of a
Definition keeps its evaluation (`factory_run.definition_evaluation`) with
the commit, and is judged by it once it ends; its arms are assigned from
it as it starts. Insights and a suite's start read the team's Definition at
its head (`FactoryService::team_evaluation`); one that does not read lists
no Scorers, Experiments or Benchmarks, never the deployment's.
A Scorer judges, with a small model on Macro's model router (the `RunJudge`
port, `outbound/model_run_judge.rs`, `FACTORY_SCORER_MODEL`, usage as
`AiFeature::FactoryGate`, billed to no one), the `samplePercent` of its
Flows' finished Runs: once a Run ends, the workflow asks the worker
(`factory_score_run`, behind the `factory-scoring` patch marker, its
failure ignored) and each sampled Scorer judges it once, from a hash of the
Scorer and the Run, so a retry samples the same Runs and judges nothing
twice; the activity is tried at most three times. A cancelled Run is not
judged.

A judge reads the Run as one JSON object: its request, how it ended, and its
last Steps' outputs. A Scorer that sets `reads: ["diff"]` is also given the
Run's change (`evaluation/service.rs`, `RunChange`), read once per scoring
attempt through `PullRequests`: when its pull request merged, GitHub's
compare of the merge commit's first parent to the merge commit; otherwise
of the base branch to the Run's branch's head, or to its pull request's
head once the branch is deleted. A rebase merge of several commits reads as
its last commit. The diff follows the JSON in a code fence nothing in it
can close (`fence_for`), and both the prompt and the judge's preamble say
it is data, never instructions. The whole addition, heading, fences and
note included, holds to `DIFF_BYTES` (40 000 bytes, about ten thousand
tokens): the diff is cut at a line, each line first capped at
`DIFF_LINE_BYTES` (1 000) with a marker, so one long line leaves room for
the files after it, and the judge is told how many lines and bytes were
left out. A Run with no change to read (no repository, merge, pull request
or branch) is judged on that. A change GitHub lists only in part (300
files or more), or one that could not be read (a GitHub read that fails on
the last attempt; an earlier one fails the attempt, to retry), is not
judged at all by the Scorers that read it, and is logged: a judgment
without the change would be noise in insights. Such Runs drop out of that
Scorer's mean, so an Experiment's arm that makes larger changes reads
better on it than it is: compare arms' Run counts on such a Scorer, not
its mean alone. A trial's branch is deleted
only once its Scorers judged it, or on the last attempt, so a retry still
reads its change; a trial whose branch is gone is not judged by them.
`reads` takes only `diff`: any other value does not read, in
`FACTORY_EVALUATION`, in a Definition's `scorers.json`, and in
`factory_definition_check`.

A sampled Scorer of the code a Run merged, `slop`, measures drift over time
without a blocking reviewer for matters of taste:

```json
{
  "name": "slop",
  "instructions": "Judge only the code the run's change adds or alters, not the request or the steps. Score 1 when it adds no slop and 0 when it is mostly slop. Slop is: dead code (unused functions, branches that cannot run, code left commented out); a helper that duplicates one the repository or the change already has; comments that restate what the code says instead of why; and drift from the repository's conventions as the surrounding code shows them (naming, error handling, layout, test style). Name the worst instance in the reason.",
  "samplePercent": 10,
  "reads": ["diff"]
}
```

An Experiment splits a
Flow's Runs on
a task into arms by a hash of the Experiment and the task (a Run on no task
takes none); `start_run` records the arms (`factory_run_arm`) before the
workflow starts, and `SessionAgent` starts each Step with the Agent the
Run's arm gives its Role, the Role's own Agents following it on lack of
quota; insights count an arm's Runs where another Agent took a Step of a
Role the arm chose for, or the chosen one was passed over (`fellBack`). A
Benchmark lists frozen tasks on one repository, each pinning a
`baseCommit` or not: `POST /benchmarks/{benchmark}/suites`
(`{"agents": {role: agent}}`) checks the caller reaches the repository
and reads its default branch's head once, records a suite, and starts one
trial Run of each task on the `benchmark` Flow (plan, then write, no Gate,
no pull request), its branch made from the task's pinned commit or that
head (kept on the trial, read into the prompt as `StepInput::base_commit`;
the agent is told to branch from it; scoring checks the pushed branch
contains it and took nothing of the base branch after it (the merge base
of the base branch's head and the branch's head is the pin or older),
scoring a trial that fails this 0; GitHub unable to compare the commits
(no shared history, a commit it lacks) fails it too. Any other GitHub
failure while checking fails the scoring attempt; its last attempt
scores the trial unchecked and logs it. A trial scoring 0 anyway, a
cancelled one among them, is not checked).
The Flow is `unattended`: its agents ask no question, a Step whose agent
stops without a result fails, and it runs only as a trial, left out of
`GET /flows`, live insights and live Scorers. A trial walks it even for a
team that names a Factory Definition, keeping the Definition, whose Agents
take its Steps: a Definition names no Flow `benchmark`, and one with
Benchmarks must fill the benchmark Flow's Roles. A trial runs on its
configuration only: an Agent out of quota fails its Step, with no other
of its Role taking it. Each trial is a Run its caller starts: the suite
reads the agent acting for them as `POST /runs` does (`acting_agent`), and
under a Definition with Standing Roles checks `start` once, before it keeps
anything (403 `notInStandingRole`); each trial keeps that agent as its start
(`FactoryService::standing_start`, `create_trial_run`'s `started_by`). One
running suite of a Benchmark per person, and
`MAX_OPEN_TRIALS` running trials per person, hold new suites (409); a
trial whose Run is refused stops the suite, which reports it (`refused`)
and plans only the trials it started, and a suite none of whose trials
started is deleted. A suite still starting counts the trials it
planned. Once a trial's Run ends it keeps its score (1 when write ended
`done`, 0.5 `red`, 0 otherwise) and its agents' tokens (the sessions its
Run's token budget counts, `ScoringSubject::sessions`), its Run's own
`factory/<run>` branch is deleted as its owner once its Scorers judged it,
or on the scoring's last attempt (best effort, through
`PullRequests::delete_run_branch`, which takes the Run, never a branch
name; one found missing counts as deleted), and, unless it was cancelled, the Scorers
its Benchmark names judge it into the trial's own record. A trial whose Run
ended `infrastructure` never had its work run: it keeps no score or tokens,
no Scorer judges it, and its suite counts it apart (`infrastructure`) and
leaves it out of its score, even one kept before such trials kept none. Unlike a merged
Run's branch, a trial's is deleted without reading its head or the pull
requests based on it: it replays a task already done from a pinned past
commit, is never offered for merge, and is deleted as its Run ends, so
nothing is meant to be pushed to it or based on it after.
`GET /insights/evaluation` (same
params as `/insights`) shows each Scorer's judgments, each Experiment's
arms with their Runs' outcomes and scores, the Benchmarks and the caller's
suites of the period. Each trial has when its Run was created and when it
ended (`startedAt`, `endedAt`): the span includes any wait before its
Steps ran, such as for a Machine, not only the work. `endedAt` is null
until the Run ends, and a Run whose workflow never started ends only when
a read finds it stale, so the web app should not run a live clock from
`startedAt`.

A Run's owner labels a live Scorer's judgment of the Run with whether it
should have passed (`PUT /runs/{id}/scores/{scorer}/label`, `{"passed"}`;
`evaluation/insights/calibration.rs`). Only the person in their own
signed-in session labels, never an agent's account (403); a new label
replaces the last; a Run not theirs, or a Scorer that did not judge it,
is 404; and a Scorer name outside the name rules is 400 before anything
is read. `GET /runs/{id}/scores` lists the Run's judgments with their
labels for its owner, however they call (an API key, an agent session
or an internal caller for them too), as `GET /insights/evaluation` does.

A Scorer's judge is the model the Scorers judge with
(`FACTORY_SCORER_MODEL`) and the Scorer's instructions with what it
reads, whose SHA-256 each judgment keeps
(`factory_run_score.instructions_sha256`,
`ScorerDefinition::instructions_digest`). A Scorer is calibrated once its
current judge agrees with the person's labels, over all of their Runs:
at least `CALIBRATION_LABELS` (20) of that judge's judgments labelled,
with its pass (a score of at least `PASS_SCORE`) the label's on at least
`CALIBRATION_AGREEMENT` (0.8) of them. A new model or new instructions
start the Scorer's calibration again from none; the labels of the old
judge's judgments are kept, and count again if the judge comes back.
Judgments made before fingerprints were kept calibrate no judge. Each
Scorer in the insights, and each judgment of a Run, carries its
`calibration`, and a judgment says whether the current judge made it
(`byCurrentJudge`); until a Scorer is calibrated, nothing should act on
its numbers. A Scorer no longer defined has no current judge and is
never calibrated. Calibration is each person's, from their own labels,
as the insights are of their own Runs; a trial's judgments are never
labelled.

The Step Terminal (`src/domain/step_terminal.rs`) lets the Run's owner
watch a running agent Step's session and steer it. A Step reads the
session it works in while it runs and holds one (`terminal.sessionId` in
the Run API), which the web app shows with Macro's own agent session
transcript, read-only. `GET /sessions/{session_id}/step` names the
caller's Step that works in a session, so Macro's session view sends them
to its Run rather than offer its composer. `POST
/runs/{id}/steps/{seq}/note` (`{"note"}`) sends the agent a note, a later
prompt of its session (`StepAgent::steer`, the routine sessions'
`follow_up`), quoted in a block nothing in it can close, as the owner's
notes are, which the agent reads once its turn ends; `/pause` stops the
agent's turn (the routine sessions' `cancel`) and holds the Step;
`/resume` (`{"note"?}`) tells the agent to go on, with the note if there
is one; `/stop` stops the turn and ends the Step `failed`, as work that
fails for good, so its session is stopped and deleted and the Flow routes
the failure. Each is kept on the Step first (`factory_step_steer`,
`terminal.steers`), then sent; a prompt the session refuses is kept
undelivered and no longer followed (502), and a refused resume leaves the
Step paused. Every prompt the factory sends a Step's session, the
Terminal's too, is marked out of sight (`PromptRoutineSession::out_of_sight`),
so it makes no one a follower of the hidden session and no sessions list
shows it.

Nothing new goes through the workflow: no signal, update or command, so
Runs in flight replay as before and no patch marker is needed. The Step's
work reads the steering while it waits on its agent's turn
(`SessionAgent::wait_turn`): once a turn ends it follows the owner's
latest note or resume sent after it, goes back to the turn before when the
session refused that prompt, does not end while the Step is paused (and
stops a turn that starts again meanwhile), and a stop fails it at once
(`StoppedByOwner`). A note keeps the agent's hand back, so a turn that
hands back nothing more ends with the one it gave. When the work has
nothing more to follow it closes the steering (`close_steering`,
compare-and-set on the prompt it followed), so a note that came meanwhile
is followed rather than lost; between the factory's prompts, as while the
engine runs the repository's checks, the Step takes no steering (409,
`terminal.betweenTurns`). Every prompt of the factory's own opens it again
and drops the owner's prompt it followed: a later prompt (`begin_turn`), a
Step handed to another Agent's session (`pass_session`), and the Step that
goes on from a question, which starts with none.

Time: the wait for a paused Step's agent does not count the pause against
`SESSION_DEADLINE`, within one attempt; the Step's work as a whole is
still bounded by `STEP_TOTAL` (two hours from its start, across attempts).
So a paused Step ends `failed` `PAUSED_STEP_ENDS_AFTER` (110 minutes)
after it started, before Temporal would end it: its work records why
(`pause_ran_out`), and `end_step` gives it a specific output (as it does a
Step its owner stopped). `PAUSE_WARNING` (15 minutes) before then the work
tells the owner once, quietly, on the Run's task thread when that section
is on (`ThreadPauseNotices`); the Run API reads when a paused Step ends
(`terminal.endsBy`), which the Terminal shows. A note while paused is
refused; it goes with the resume. Only the Run's owner steers it, in their
own signed-in session (`is_in_person`), from an account that is not an
agent's: code mode, a factory Step's own session among them, an API key or
token and an internal caller are refused (403). A note is 1 to 4096 bytes
once trimmed (400).

`POST /runs/{id}/cancel` cancels the workflow through Temporal; cancelling a
cancelled Run again is accepted, and a Run whose workflow never started ends
`cancelled` (its workflow, should it start late, takes no Step). Reading a Run
whose workflow never started ends it `infrastructure` once it is five minutes
old. A Temporal outage answers 503 (or 502 for an unusable answer), never 500.

A Run ends `infrastructure`, not `failed`, when it fails for a machine, quota
or network reason rather than its work. Only what is surely the factory's
counts; when unsure, the failure is the work's (`workflow/work.rs`,
`work_failure`). A Step's work failed for infrastructure when every Agent of
its Role was out of quota, or the usage limit of its Agent's own provider
account ended a turn that did work (`StepError::OutOfQuota`, type
`FactoryInfrastructure`), or when it timed out because its worker went
quiet (`Heartbeat`) or no worker took it (`ScheduleToStart`); an attempt
that ran past its own time (`StartToClose`) may be an agent slower than its
deadline, and is the work's. A Step whose whole time ran out reads as its
last attempt did, and as infrastructure's when none left a failure.
Temporal hands back attempts whose time (`STEP_TOTAL`) ran out after a
retried failure as that last failure, not as a timeout: one of no known
kind, as an agent still at work on a later prompt (`StepError::Overdue`,
type `FactoryAgentOverdue`) or a round of checks that could not go on, is
the work's. That reading decides whether the workflow
asks for its marker, so changing it changes how recorded histories
replay. Such a Step ends `failed` and the Flow routes
`failed` as before. When that `failed` takes the Run straight to a `failed`
end, with no Step after it, the workflow records `infrastructure` instead,
behind the `factory-infrastructure-outcome` marker
(`workflow/run_end.rs`); a Run that walks on past it and fails later ends
as that later failure does, and so does one whose entry it went back to
had run out of visits and ends `failed` (`on_exhausted`): the loop did not
converge, which is the work's. So does a `failed` routed past an entry its
`when` skips to a `failed` end: only a `failed` that ends the Run itself
counts. A review round
counts only when every reviewer that failed did so for infrastructure and
none ended without a review (`pass` or `changes`). An answer to its Gate
routes as any answer. A Flow may not end a Run
`infrastructure` or `flawed` itself (`FlowError::ReservedEnd`).

A Run that ended `success` can be marked `flawed` afterwards
(`src/domain/flaws.rs`, `mark_flawed`): its result was found wrong, by a
Scorer (by its name) or by its owner. The Run keeps who, why (1 to 4096
bytes, trimmed) and when; marking a flawed Run again keeps its first mark,
and a Run that did not end `success` is refused (409). `POST
/runs/{id}/flaw` (`{"reason"}`) marks it for its owner in their own
signed-in session, as a Step Terminal's steer is (403 otherwise), and
answers the Run. Insights count `infrastructure` and `flawed` Runs apart
from `failed` and `success`.

The built-in Steps (`src/domain/built_in_steps.rs`) are `reply`, which
acknowledges the request; `plan`, `write`, `review`, `fix` and `ci_fix`, which
hand an agent a prompt built from the Step's instructions
(`built_in_steps/*.md`), the Run's request, and what the Run's earlier Steps
produced; and `publish`, `ci`, `ready_to_merge` and `merge`, which take the
Run's branch to a merged pull request (`src/domain/pull_request_steps.rs`),
and `update`, which brings a branch behind its base up to date
(`src/domain/built_in_steps/update.rs`). The
agent is the `StepAgent` port.
Each entry an agent does names the Role it fills (`"role": "planner"`), or
the Roles of its review panel (`"reviewers": [...]`): a Flow that leaves
both out does not read (`FlowError::NoRole`). The service also requires
`reviewer`, which Runs started before the review panel still use. A Run started
before Flows named Roles carries a Flow without them, and its agent Steps
fill the Role their built-in Step implies (`plan` a planner, `write` a
writer, `review` a reviewer, `fix` and `ci_fix` a fixer). The factory's
agent definition (`src/domain/agents.rs`, `FACTORY_AGENTS`)
lists the Agents that fill each Role, in order (a review panel's Role it
does not list, `correctness-reviewer`, `scope-reviewer` or `tests-reviewer`,
takes `reviewer`'s, so a definition written for one reviewer keeps working;
one that fills neither stops the service with a message naming both): an Agent is a Macro agent
(`bot`), whose harness runs its sessions, with an optional model and
instructions. `SessionAgent` (`src/domain/session_agent.rs`) does the Step in
a Macro agent session of the Role's first Agent, owned by the Run's owner,
through the routine sessions capability (`agent_harness_service_client`). The
Step records its Role and the Agent whose session it works in. An Agent the
harness refuses for lack of quota (`RoutineSessionError::Admission` with
`AiAdmissionError::Denied`, on prepare or on prompt: a definitive refusal
before any work starts) hands the Step to the next Agent of its Role, in a
new session, and the Step records the Agent passed over with the refusal's
code (`passed_over`, `pass_reasons`); a session that opened is deleted
first. Only that refusal passes the Step on; when no Agent is left, the Step
records the last one passed over too and fails for good. The worker
remembers such a refusal per owner and Agent for 15 minutes
(`session_agent/quota_memory.rs`, `QUOTA_COOL_DOWN`): a Step that made its
session itself (a first attempt, or one it was just handed on to) skips an
Agent refused lately without opening its session, recording it passed
over as `no_quota_lately`, while another Agent of the Role is left; the
last is always asked. Neither the harness nor billing tells ahead of a
session whether a quota is used up, so recovery is found by asking again
once the cool-down passed: a start forgets the refusal, another refusal
remembers it anew, and a skip never renews it. The memory is each
worker's own, and a restart forgets it. A turn the usage limit of its
runtime's own provider account ended (`RoutineFailureReason::UsageLimit`:
the routine status reads it from a failed turn the runtime classified as
`ProviderUsageLimit`, as Codex's adapter's error does, below) is out of
quota too (`AgentFailure::out_of_quota`), and remembered as a refusal is.
A first turn that ended so before calling any tool (`RoutineSessions::activity`)
changed nothing: its session is deleted and the next Agent of the Role
takes the Step in a new session, the Agent recorded passed over as
`usage_limit` (`USAGE_LIMIT`). One that called a tool, or whose activity
cannot be read, or with no Agent after it, ends the Step out of quota
(`StepError::OutOfQuota`), so its Run ends `infrastructure` rather than
`failed`.

A Role runs on Codex as on any runtime (`docs/runbooks/factory-codex-role.md`
sets one up on a bench): its Agent's `bot` is the agent of a machine whose
macrod runs Codex (`macrod service --agent codex`, the
`@agentclientprotocol/codex-acp` adapter over the installed `codex` CLI,
signed in with its owner's own login), and its `model` one `codex` lists
(`gpt-5.5`), set on the session as Claude Code's is. Its sessions open,
take the Step's prompt, reach Macro's tools through the MCP servers macrod
forwards, and hand back with `macro.factory.handBack`, as Claude Code's
do; its turn's end, no result, last message and Step Terminal are read
from the same session history. The adapter answers a turn Codex ended in
error with JSON-RPC's bare `Internal error` and Codex's message and error
class in `data` (`{ message, codexErrorInfo }`; `codex_error_info` from the
deprecated Rust adapter): the session's fold keeps Codex's message, and
reads `usageLimitExceeded` as a `ProviderUsageLimit` notice
(`agent_fold`'s `harness/codex.rs`), so the Step is out of quota as above.
Codex's cloud agent (`bot_id::CODEX_BOT_ID`) cannot take a Step: its
sessions work in a cloud environment of their own and are handed no Macro
tools, so they never hand back. An agent definition whose work Roles list
an Agent on it does not read: `FACTORY_AGENTS` stops the service, and a
Definition's `agents.json` is refused, each naming the Agents on it
(`AgentDefinitionError::CloudCodex`). An Agent on it that only a Standing
Role lists reads: it takes no Step, so nothing waits on it. An Experiment
arm or a Benchmark configuration may not give a Role such an Agent
(`check_choices`, `EvaluationDefinitionError::CloudCodexAgent`): the
arm's Definition or `FACTORY_EVALUATION` does not read, and the suite does
not start. Neither a Codex nor a Claude Code Agent signed in with its
owner's own login records token use, so their Steps are unmeasured
(below). An attempt prepares, opens and prompts only the session the
Step works in now, so an overlapping attempt never prompts a session
handed on: it deletes the one it opened and goes on in the Step's current session. Each session keeps the
bot that runs it, so a stop or a release never depends on the agent
definition; a session claimed before Steps recorded their Agent and bot is
of the first Agent of its Role. A Step whose Agent its Role no longer lists
passes to the Role's first Agent. The Step claims its
session in `factory_step` first, so a retried Step goes on in the same session
and does not start the work over: it always asks, and the session refuses a
prompt it already has with a conflict. That guard is a check of the session's
history, not exactly-once delivery, so a prompt whose delivery is unknown is
never sent again: the session's status settles it, and one that never shows
fails the Step for good. The prompt asks the agent to hand back its result with
`macro.factory.handBack({ outcome, text })` from code mode, which reaches `POST
/hand-backs` as the Run's owner; the correlation layer names the session
(`agent_correlation::context::current_session`), and the hand back must offer
one of the Step's outcomes. When the session's first turn ends, the latest hand
back is the Step's outcome and output; with none, the agent is reminded
once, and a Step it still hands nothing back for waits for the Run's owner
(above). `LocalAgent` does no
work and hands back the prompt it was given: the tests' stand-in, and the
service's when no agent is configured.

A Flow with `"repository": true` (`code`) works on one GitHub repository: `POST
/runs` must name it as `repoUrl` (`https://github.com/owner/name`, the canonical
form a coding session takes: no `.git`, no trailing slash), other Flows refuse
one, and the owner's linked GitHub account must reach it and may push to it;
its default branch, read then, is the Run's base (`factory_run.repo_url`,
`base_branch`). A refusal for either reason carries a `code` in its body
(`unreachableRepository`, `noPushAccess`) beside the message, for a client to
name the cause. Each agent Step of such a Run opens its session with that
repository (`PrepareRoutineSession.repo_url`), so its runtime checks it out: a
managed sandbox through the agent egress, macrod in a per-session worktree or
its configured workspace. It names the Run's branch too
(`PrepareRoutineSession.work_branch`, `factory/{run id}`), which reaches an
external runtime on its request (`AgentSessionRequestedEvent.work_branch`): a
macrod that shares one workspace among its sessions gives the branch a git
worktree of its own beside it (`coding_agent_worker`'s `branch_worktrees.rs`,
`<workspace>.macrod-worktrees/factory+<run id>`), which every Step of the Run
works in and no other Run does, so two Runs at once on one machine never
share a checkout's files or `HEAD`. macrod removes the worktree once no
session used it for an hour, origin no longer lists the branch and the
worktree holds nothing origin lacks, or once no session used it for 3 days;
never while a turn runs in it.
Nothing is recorded or replayed in the workflow, so no patch marker guards
it: a Run in flight gets its worktree at its next Step. The prompt names the
Run's branch and its base: the agent checks the branch out, and `write`,
`fix` and `ci_fix` commit and push it from their runtime; `review` changes
nothing. A `write` or `fix` whose agent hands back `done` with none of its
work on the branch on GitHub ends `failed`, saying why, before its checks
(`checks_round.rs`, `unpushed`, which `update` and conflict resolution skip):
the branch changes nothing against the Run's base (GitHub lists no changed
path; a list GitHub cannot give in full passes), or, for a fix that says it
fixed a Blocker of its review round, is still at the commit that round
reviewed. Either is read again after 2 s before the Step fails. A branch
GitHub does not have, even after 2 s, fails the attempt for a retry, as a
round's read of it does, until the Step's time for rounds (105 minutes) is
up: then the Step fails saying the branch is not on GitHub. Like the
checks, this runs in the Step's activity, behind no patch marker. The pull request Steps act on GitHub as the Run's owner, with
the token of the GitHub account they linked to Macro, which the
authentication service gives (`outbound/github_pull_requests.rs`, over the
`github` crate's user-token client), read on every call. A missing link, a
token GitHub no longer takes (401), a refusal that is not throttling (403) and
a repository out of reach (404) fail the Step for good; throttling, 5xx and
transport failures are tried again. Each Step reads before it writes, so a
retry or a Run sent back finds what it did: `publish` finds the branch's pull
request or opens it, and a refusal because another attempt opened it reads as
published; `ci` reads the pull request's head, its check suites, commit statuses and
GitHub Actions workflow runs and, when required checks are named, its check
runs, every 20 s, with one token per read and every page of each list,
heartbeating. Once per Step it reads the checks the base requires (branch
protection and rulesets, joined from what can be read, and marked complete
only when both were read in full). Reading the workflow runs needs the GitHub
App's Actions read permission: a token that may not read them holds CI only
while an Actions suite sits empty. Its guarantees:
- It ends `red` as soon as a suite, run or status fails, naming them.
- It ends `green` only on two reads in a row that saw the same checks, each
  with every suite that holds a run passed and every status succeeded, and
  every required check that could be named appeared and passed: as a run of
  the app the rule names, or, when it names none (or GitHub's -1, any
  source), any app's run or a status of that name.
- Once a complete, non-empty required set all passed, empty suites no longer
  hold it back. A set read only in part must pass, but drops no empty suite.
- Short of that, a GitHub Actions run on the head that has not finished
  (queued, pending, waiting, requested, in progress, or a status GitHub adds
  later), as one a concurrency group holds, holds it up to the 25-minute wait,
  whether or not its suite exists yet; any other app's empty suite holds it
  only until 5 minutes after GitHub made it.
- A CI app other than GitHub Actions whose suite stays empty past 5 minutes is
  not waited for; make its check required so `ci` waits for it.
- A list longer than the 10 pages read, or no check reported, keeps it waiting.
- After 25 minutes, within one Step attempt, it ends `timeout`.
- GitHub does not say which app posted a commit status, so a required check
  that names an app is met only by that app's check run: one such app that
  reports only by status (as some deploy previews do) keeps CI waiting until
  it times out.

The pull request's description holds the request, then the Run's report,
then the lines that name the Run and its task
(`src/domain/pull_request_steps/description.rs`). The report is a section
marked by HTML comments: the plan (without its acceptance block), the
Done-when checklist with each clause's result in the latest `accept` Step,
the review rounds with how each Blocker ended (fixed, disputed and upheld or
dropped), the latest round of the repository's checks, and the follow-ups
filed. `publish` opens the pull request with it, and `publish` and `ci` (once,
on its first read, so a Run that comes back from `update` shows it) write it
again on each visit: only the marked section is replaced, so what a person
wrote around it stays, and a description without it gets it at its end. A
description takes about 1% less than GitHub's 65,536 characters: the
request is kept whole lines (cut by characters only when its first line
alone does not fit), and the rest of it, then what does not fit of the
report, go into numbered comments, each found again by its marker and
edited in place; one no longer needed, or a second of one number, says so.
Only comments by the GitHub account the owner's token acts as (`GET
/user`) count, whoever opened the pull request: a marker anyone else writes
is never edited, and when that account cannot be read no comment is
written. Every marker names the Run
(`run=<id>`), and only this Run's count: the section is the first of its
start markers that its end marker closes, and its other sections and lone
markers are removed until none is left. The request and what agents wrote
are escaped one way, code included: `&`, `<` and `>` as entities, and a
word joiner after `@` before a name, `#` before a number, `GH-` and a
closing keyword before a link or a Markdown link; web and email addresses
get no joiner. Follow-ups are not named by their task id, which would link the
pull request to them. Writing the report reads before it writes, writes
only what changed, and a failure is logged without failing the Step; when
`publish` cannot read the review rounds, it opens the pull request without
them, and a later write adds them.

On a repository, `write` and `fix` then run the repository's own checks
(`src/domain/built_in_steps/checks_round.rs`). The repository declares them
in `.macro/checks.json` on its base branch (`src/domain/checks.rs`): each a
`name`, a `run` command from the repository's root, and optional `paths`
(files or folders from the root, no patterns) whose change makes it run,
and an optional `timeout` (seconds, 1 to 1200, default 600);
`rounds` (1 to 10, default 3) is how many times a Step runs them. The file is
read from the Run's recorded base, never the Run's branch, which the agent
pushes: the first round keeps it, with the checks and commands it runs, and
every later round and retry goes by that record. A later round runs the
first round's checks and any others the branch's changes now reach (every
check when GitHub lists only part of the changes); a check only a later
round reached is dropped again once the changes no longer reach it. Where the agent's
session runs on a machine Macro reaches, the engine runs each check itself
(`StepAgent::run_check`, the routine sessions' `exec`), on the Run branch's
head as GitHub has it (`PullRequests::branch_head`, read again once after
2 s when missing, and checked to be a commit id), in a worktree it sets up
from the agent's checkout (`checks_round/worktree.rs`: `git worktree add
--detach` in `${XDG_CACHE_HOME:-$HOME/.cache}/macro-check/<round>`, a
`0700` directory of the machine user's own), and removes after the round.
Setting it up removes worktrees left there over four hours, fetches the
commit only when the repository lacks it (at depth 1 in a shallow clone,
with the machine's global git configuration, where its credentials are),
keeps a worktree an earlier attempt left at the commit (reset to it) and
replaces one at another, then checks the worktree is the commit as it is,
with no entry git is told to skip. Git otherwise runs by absolute path where
the machine has one, without system or global configuration, hooks,
filesystem monitor, replace objects or sparse checkout, so Git LFS files
are checked as their pointer files. Each check waits at most 60 s for the
round's lock (`flock -w`, else a `mkdir` lock taken over from a holder that
died, which the run logs and reports), then runs with `bash --noprofile
--norc` under its own `timeout` from the checks file (600 s by default,
1200 at most), without the lock's descriptor; its exit code decides. The
round is `verified`, keeps the commit it checked (`checked_commit`) and each
check's exit code and the end of its output (`results`, 4 KiB once escaped
as JSON) as it ends, so a retry runs only the checks still to run. A check
starts only with its timeout and the lock's wait left: short in the attempt
(25 of its 30 minutes, `ATTEMPT_CHECKS_FOR`), the attempt fails for its
retry; short in the Step's rounds, the round and the Step end `red` at once.
A worktree that cannot be set up, a branch GitHub does not have, and a run
that found the lock held fail the attempt for a retry and record nothing. A
Step whose checks the factory ran before never leaves a later round to the
agent: a session that runs no command then is retried until the rounds'
time is up. Nothing the agent hands back decides a verified round, and
nothing it leaves in its own checkout reaches the checks. What remains in
the agent's hands: the machine and its toolchain (`BASH_ENV`, which a
sandbox's dev shell needs, the compilers, git where it is not at a fixed
path), so a verified round says the checks passed there on the pushed
commit, and CI on that commit stays the gate before a merge. Machines:
Daytona, local Docker and Namespace sandboxes run the checks, and so do
Macro computers for a session macrod gave a worktree of its own (as it
reported when the session opened; a worktree gone since is an error, which
is retried); a paired machine of someone's own, a computer session in a
workspace macrod shares, Cursor and
Codex do not. Elsewhere (those, or a harness from before
the `exec` route, which answers 404: `run_check` answers none) the agent runs
them: a round is a later prompt of the Step's session
(`StepAgent::follow_up`, the routine sessions' `follow_up`) that asks it to
run them verbatim, change nothing, and hand back `passed` or `failed` with a
report; a round whose prompt was sent stays the agent's on a retry. A
failure goes back to the same session in another prompt with the report
(for a verified round, only the checks that failed, with their output), and
the next round checks the fix. Each round, with its prompts' ids, is
recorded before it runs or is asked (`factory_check_round`), so a retried
Step goes on with its round; a new prompt clears the session's hand back and offers its own
outcomes (`begin_turn`), and a stop watches it. A later prompt goes to the
session the Step works in now, of the Agent that took it after any passed
over, run by the bot recorded with it; `begin_turn` records only for that
session and never goes back to an earlier prompt: each later prompt has a
place the Step gives it (`AgentTurn::place`, a round's checks at twice its
number, its fix just after), which orders them whatever the workers'
clocks. A Step that took a later prompt is never passed on, so every round
stays in the session that did the work. An attempt reads the hand back of its own prompt only: one
the Step went on from, as when an overlapping attempt started the checks
while this one waited on the work, fails `Superseded`, which a retry goes
on from. A later prompt refused for lack of quota is not passed to another
Agent, since the work is in its session: the Step ends `red`, with an open
round ended failed saying the quota ran out, and the round is kept as
ended by it (`quota_ran_out`), so a retry ends the Step the same way
without asking again. A usage limit that ends the agent's turn on a later
prompt is out of quota the same way: the Step ends `red`, the round ended
by it (`quota_ran_out`). The agent's work and all
rounds run in the Step's one activity: waiting for them stops
`ROUNDS_END_BY` (105 minutes) after the Step started, short of its two
hours, and the Step ends `red` saying so, with a round still open ended
failed (or with the agent's hand back for it, if it gave one). Each wait is
also bounded by `SESSION_DEADLINE` (29 minutes) from when it began: the
first work's ends the Step failed, as for any agent Step, and a later
prompt's is retried and goes on with the same turn. The Step
ends `done` once a round passes, the agent's own outcome when its work
changed nothing or no check applies, and `red` when the last round failed,
the rounds ran out of time, or the file does not read; the `code` Flow ends
such a Run `manual`. Its output says which, then the last report and the
agent's work, cut to the 64 KiB a Step keeps. The rounds show on the Step
(`checks` in the Run API, each `verified` or reported by the agent).
`LocalAgent` runs no command, and answers a round of checks `failed`: no
check ran.

Each check the engine runs is told the change it checks
(`src/domain/check_context.rs`), since its worktree may hold only the
checked commit (fetched at depth 1 in a shallow clone), so git alone
cannot say what changed. Its shell exports two variables, outside the
check's own command, which the round records and shows as written:

- `MACRO_CHECK_BASE`: the commit the change is made against, the merge base
  GitHub's comparison of the Run's base (the commit a Benchmark trial is
  pinned to, else the base branch) with the checked commit gives. Setting
  the worktree up fetches it when the repository lacks it (at depth 1 in a
  shallow clone), so `git diff --name-only "$MACRO_CHECK_BASE" HEAD` works;
  a fetch that fails is said in the set-up's output and on the file's
  first line (`partial`), and holds nothing up.
  Empty when the comparison failed and the Run pins no commit.
- `MACRO_CHECK_CHANGED`: the absolute path of a file listing the changed
  paths, `<round>.changed` beside the round's worktree in
  `${XDG_CACHE_HOME:-$HOME/.cache}/macro-check`: outside the repository's
  tree, so git never sees it and nothing commits it. Setting the worktree
  up writes it afresh on each attempt (to a file of its own, moved into
  place); removing the worktree removes it, and the four-hour sweep takes
  one left behind.

The file's first line says what the list holds: `complete` (every changed
path, and the base is in the repository), `partial` (the list or the base
cannot be trusted whole: GitHub lists at most 300 files of a comparison,
and none of one too large to list; a path holding a line break, and paths
past 64 KiB once single-quoted for the shell, are left out, since
sandboxes run the set-up as one `bash -c` argument, which Linux holds to
128 KiB; or the base could not be fetched, or GitHub gave none), or
`unavailable` (none: the comparison failed). Each line after it is one path from the
repository's root, as GitHub names it: a renamed file under both its names,
the earlier first, and a removed file though the worktree no longer holds
it. A check that keeps to the change checks everything unless the first
line is `complete`, or when `MACRO_CHECK_CHANGED` is unset. A comparison
that fails never holds the checks up nor fails the round: the checks run,
told the list is `unavailable`, and the engine logs why. Both are read from
GitHub in the Step's activity, on each attempt, so the workflow and its
replay are unchanged. A setup command is told neither, and
`.macro/setup.json` cannot set either (`MACRO_*`), so no repository
overrides them. Where the agent runs the checks (a runtime that runs no
command), neither is set. `accept`'s acceptance checks, run by the same
round, are told the change the same way.

A repository prepares a fresh checkout for the factory in `.macro/setup.json`
on its base branch (`src/domain/setup.rs`): `setup`, a list of commands, each
a `name`, a `run` from the repository's root and an optional `timeout`
(seconds, 1 to 1200, default 600), at most 16; and `env`, at most 32 plain
values, `export`ed inside the script `bash` runs for each command and each
check (never before `timeout` or `bash`), single-quoted and so never
expanded: `$PATH` in a value stays that text. It is read as the
checks file is, from the Run's recorded base branch, never the Run's branch,
and is a default Control File. A file that does not read (over 16 KiB, not
JSON, a key named twice or not known, a bad name, command or timeout, an
`env` name that is not a shell variable's, or one that changes the shell
(`BASH_ENV`, `ENV`, `SHELLOPTS`, `BASHOPTS`, `PS4`, `BASH_XTRACEFD`), the
home (`HOME`), where git finds the repository, its objects or its
configuration (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`,
`GIT_CEILING_DIRECTORIES`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`,
`GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_CONFIG*`, `XDG_CONFIG_HOME`), what
git runs (`GIT_EXEC_PATH`, `GIT_SSH_COMMAND`, `GIT_SSH`, `GIT_ASKPASS`,
`GIT_TEMPLATE_DIR`), the linker (`LD_*`, `DYLD_*`) or is the factory's own
(`MACRO_*`, such as the checks' `MACRO_CHECK_BASE` and
`MACRO_CHECK_CHANGED`); `PATH` and harmless git values such as `GIT_TERMINAL_PROMPT`
are allowed; a value over 1 KiB or with a control character) ends the Step
`failed`, saying why. Secrets never live there: a file holding something
shaped like a key or a token (the Definition's check), or an `env` name
that says it holds one (`TOKEN`, `SECRET`, `PASSWORD`, `API_KEY`, ...), does
not read. A base with no file, or one that asks for nothing, runs no setup,
as before.

Each agent Step on a repository reads it (`BuiltInSteps::setup`) and hands it
to its agent (`AgentTask::setup`); its first prompt names the commands that
ran and the `env` values. `SessionAgent` runs the commands in each new
session after it opens and before its first prompt, in order, through the
routine sessions' `exec` (`bash --noprofile --norc` under the command's
`timeout`), recording on the Step how many passed in that session
(`factory_step.setup_session_id`, `setup_passed`), so a retry in the same
session skips those, and a new session (handed to the next Agent, or one
that went on afresh after a lost session) runs them again. A session whose
first prompt already landed is never set up. Each command holds a lock in
the checkout's git directory (`macro-setup.lock`, taken as a check takes
its round's: `flock -w 60`, else `mkdir`; `src/domain/shell_lock.rs`), so a
retry's command never runs beside one an attempt lost with its worker left
running; a lock still held after 60 s fails the attempt for a retry. A
command starts only with its
whole timeout left in the attempt (`SESSION_DEADLINE`, or the time the Step
gives): otherwise the attempt fails, and its retry goes on with the commands
still to run. A command that fails ends the Step `failed`, with the
command, its exit code and the end of its output (4 KiB; each line that
looks like a secret left out, and a private key's block whole, to its
`-----END` or the output's end) as its output, and
no agent turn spent; so does a runtime that runs no command (a workspace
macrod shares, a paired machine, Cursor, Codex, `LocalAgent`), saying the
setup cannot run there: the Step never goes on without it. A reviewer's
failure fails its round. Either is the Step's report, not a work failure,
so the workflow and its replay are unchanged.

A checks round (and `accept`'s round of acceptance checks) keeps the setup
file its first round read (`factory_check_round.setup`), and a verified
round runs its commands in the round's worktree, under the round's lock,
after setting it up and before the checks, within the same time limits as
a check. Each command's run is recorded among the round's results as
`setup:<name>`, so a retry runs only the commands still to run: a long
setup and a long check fit in two attempts. A retry's worktree at the same
commit is `reset --hard`, never cleaned, so it keeps the untracked files
the setup made (`node_modules`, ignored build output) but not its edits to
tracked files, which the reset undoes and no command passed runs again: a
setup whose output is checked in (a `just generate` whose files are
committed) leaves a retry's checks the committed files. A command that
fails ends the round failed, as a red check: its report, which goes to the
agent's fix, shows the command and the end of its output, and says the
checks did not run. The round keeps the setup's text, not what was read
from it, and reads it again on each attempt, as it does the checks file: a
deploy whose rules refuse more (a newly refused `env` name) fails the Step
of a round in flight that kept such a text ("a recorded setup file does
not read").

An entry with `reviewers` is a review panel (`src/domain/review_rounds.rs`,
`factory_review_round` and `factory_review_finding` in `factory_records`).
Each visit is a review round: the workflow opens it
(`factory_open_review_round`), which records the commit of the Run's branch it
reviews (read from GitHub as the owner) and the diff under review: the Run's
whole change against its base in round 1, and in a later round the fix diff,
from the commit the round before reviewed; a fix that pushed nothing has the
whole change reviewed again. Then it starts one Step per reviewer, at the
places after the round's own (`NextStep::Run`'s `seq` and the ones after it,
in the entry's order; the interpreter counts them all against `MAX_STEPS`),
runs their work at once (`join_all`, scheduled and read in the entry's order),
and records each end in that order. Each reviewer is told its Role's focus,
the commit and the `git diff` under review
(`built_in_steps/review_panel.md`, `review_focus/`), and hands back `pass` or
`changes` with a fenced `json` block of Blockers (the last such block, in
any case), each a file, optional lines at that commit, what is wrong and
why. From round 2 the reviewers are also given the round before's Blockers,
numbered, to check wherever they are now; one found again names its number
(`previous`) and blocks wherever it is. The review Step reads the answer
(`ReviewerAnswer`, `built_in_steps/review_panel.rs`); one that does not read,
or whose outcome disagrees with its list, is asked for once more in the
reviewer's session, in a prompt kept before it is sent
(`factory_reviewer_answer`), and one that still does not read is one Blocker
on the whole change. The findings are recorded on the round with the
answer's having been read, and the Step's output is the summary and the
Blockers written out. Once
every Step ended, the workflow closes the round
(`factory_close_review_round`, which heartbeats): a finding that blocks
anywhere, or whose file and lines meet a hunk of the diff under review
(GitHub's compare, as the owner), is a Blocker; one outside it becomes a
follow-up task and holds nothing. Reviewers that report one problem (the
same file, at lines that meet; a finding with no lines covers its file) make
one task, which names each of them with what each reported; each finding
stays recorded and is linked to it. A problem a task of an earlier round
of the Run is about (its file again, at lines that meet), a revise's rounds
among them, makes no second task while the task is still there for the
owner (`task_access`): the oldest such task wins. Its findings are linked
to it, and a finding linked to a task widens what the task is about for
later rounds. Then the owner comments on the task, once a round
(`POST /documents/{id}/comments/text`, quoting the task's `factory run
{id}`), that the problem was reported again, in which round, by whom and
where, and the comment is marked on the round's findings
(`follow_up_commented`). The links come before the comment and the mark
after it, so a retried close makes a missing comment and never a second
one, unless the mark itself fails to be kept. A comment that fails is
logged at error level and holds nothing; it is made again only when the
close is tried again for another reason. Joined problems are linked and
commented on before new tasks are made. A task that is gone, or a
follow-up document storage refused, is not joined: the problem makes a
task of its own. The task is made as the Run's owner, not
shared, in the project of the Run's task: the initiative its Project
property names, read and set as a Main Hold's task does
(`outbound/task_project.rs`; document storage's `POST
/documents/create_task`, with the internal key and the owner as the acting
user), assigned to the owner and not started (named with the Project, since
naming any property drops document storage's defaults), with an id kept on its findings before it is asked for, so a retry
finds it rather than making a second; an outage (or an unaccepted key or
user, a timeout or throttling) fails the close, which is tried again for at
most two hours, then the round fails; a project the owner may not edit
(entity access on the initiative, read before asking) has the task made in
no project, saying so; properties of the Run's task that do not read put it
in none too; a refusal leaves the follow-up with no task; and a task made under another id stops the round for good, since every
retry would make another. A diff GitHub
cannot list in full makes every finding a Blocker. The round ends `changes`
with a Blocker, `pass` without, and `failed` when a reviewer's work failed or
a reviewer's Step ended with no answer read (a worker without review panels
ran it). A round that cannot open, as when the Run's branch is gone, is
recorded as failed with why (`failure`) and runs no Step. The round's
outcome is what the Flow routes; a last round (its visit at the entry's
`max_visits`) that still finds Blockers routes `unresolved`, which `code`
ends `manual` rather than run a fix no round would review; a panel with
`max_visits` must route `unresolved` (`FlowError::NoUnresolvedRoute`). A
round that fails routes `failed`, which `code` ends `manual` too. The next `fix` is
given the round's Blockers in a block marked as reviewers' findings, as
data: no `<` or `>`, code fence or heading line (ATX or setext) left in it,
paths included, cut to `BRIEF_BUDGET`. Reviewers' outputs reach every later
Step's history fenced the same way. A finding's path must be a plain path:
no control character, backtick or angle bracket. An answer that never read
is not carried to the next round. The fix ends its text with a fenced `json` block that
says, per Blocker by its number in its brief, `fixed` or `disputed` with a
`reason` (`review_rounds/fix_answer.rs`); once its work hands back `done`,
before its checks, that is kept on the Blockers (`fix_status`, `dispute`, cut
to `MAX_DISPUTE_BYTES`, 2 KiB). A Blocker it leaves out, a dispute with no
reason, a block that does not read and an answer that never read are
`fixed`. A reason is kept on one line, with no NUL or other control
character. In the next round's brief every carried Blocker is listed
first, within `BRIEF_BUDGET`; the reasons follow in a budget of their own
(`DISPUTE_BUDGET`), each escaped as findings are and then cut to 2 KiB
(`fit_findings`), so no reason pushes a Blocker out. The reviewer whose Role
found the Blocker (the round's first, for a finding of no Role) rules on
each dispute whose reason the brief carries: it lists it again with
`previous`, and upholds it, or names it in `"dropped"`, and it blocks
nothing (`ruling`, by its Step, `ruled_by`). A dispute it does neither for,
or whose reason the brief had no room for, is upheld, and a carried
Blocker the brief had no room for stands too: the engine adds each to that
reviewer's answer, blocking wherever it is, so neither a dispute nor a lack
of room passes a round. A reviewer that drops a number it does not rule
on, or one it also lists again, is asked again. A new finding by the
reviewer that dropped a Blocker, in a later round of the visit (the
entry's rounds since its last reset), at lines that meet the dropped one's
and with the same problem once case, punctuation and spacing are set aside,
is kept `dismissed` (`repeats_dropped`) when it is in the diff under review:
it blocks nothing. Outside the diff it is a follow-up as any other. A
finding with no lines, or a dropped Blocker with none, dismisses nothing,
and nothing is dismissed in the round of the drop. No Standing Rule merges
a Run with a dismissed finding (`any_dismissed`): its merge Gate waits for
a person. The Run API counts per Run the Blockers fixed (as the fixer
says), disputed and upheld, disputed and dropped, and its findings
dismissed (`blockerResolutions`); Insights' review quality counts them over
its Runs (`resolutions`). None of this is in the workflow: the fix, the reviewers
and the round's close are activities, so Runs in flight replay as before. In `code`, the review panel
runs at most `max_visits` rounds (three, or `FACTORY_REVIEW_ROUNDS`, which
the service writes into the Flow a Run starts with). Rounds and their
findings show on the Run (`reviewRounds` in the Run API), numbered across
the whole Run (`round`), with the visit of their entry (`visit`): a reset
(as when the merge gate's `revise` sends the change back) starts the
visits, and the round limit, again from 1, and that round reviews the whole
change, with no Blockers carried, but it is still the Run's next round.

The head `ci` calls green is kept (`factory_run.merge_head_sha`);
`ready_to_merge` ends `changed` unless the pull request's head is still that
commit and its checks still pass, and otherwise ends `done`, so its Gate asks
the owner whether to merge; `merge` merges only that commit, naming it to
GitHub, and a merged pull request reads as merged. A head that moved goes back
to `ci`, and a person's `revise` goes to `fix` with the review's visits reset.
The merge-gate entry runs at most three times, including visits that end
`changed` without asking; then the Run ends for a person. `held` and
`behind` start its visits over (below).

A Run never merges a head its base moved past, and Runs merge into one
base one after the other (`pull_request_steps.rs`, `built_in_steps/update.rs`,
`factory_records`' `merge_queue.rs`). Both apply only where the Flow the Run
kept as it started routes them (`StepInput::routes`, read from
`factory_run.flow_definition`; a Run that kept none is told nothing, since
it walks an older Flow than the built-in one of its name, and an outcome
its Flow does not route above `merge` would fall through to the merge):

- `ready_to_merge` and `merge` end `behind` when the head lacks the commit
  the base branch is at (`PullRequests::merge_base`), so a person is never
  asked about, and `merge` never merges, a head whose checks ran without a
  commit the base gained; `merge` also reads a refused merge whose base
  moved meanwhile (a branch protection that wants branches up to date) as
  `behind`. The base can still move between `merge`'s read and GitHub's
  merge, by a merge made outside the factory: only that protection closes
  that window.
- `update` (the writer's Step) merges the base into the branch, never
  rebases it: the branch keeps every commit it had, so nothing anyone
  pushed is lost, nothing is force-pushed, and the commits review rounds
  and checks recorded stay reachable. GitHub merges it (`update_branch`,
  "Update branch", only while the branch's head is the one read; a push
  since fails the attempt for a retry); when GitHub answers with a merge
  conflict, the writer resolves it in its session (`resolve.md`: `git
  merge` the base's head, no rebase, reset or force-push). Any other
  refusal from GitHub (a token without a scope, a branch it may not push
  to) is no conflict an agent resolves: the Step ends `conflict` with
  GitHub's message, which opens the entry's Gate for the owner (below),
  so a person grants what GitHub asks and answers `retry`, rather than
  the Run failing. Nothing is kept for it, so `retry` asks GitHub again. The update is
  kept before the agent is asked (`factory_branch_update`), so a retry goes
  on the way it started. GitHub merges after it answers, and may never
  merge (its merge failed later, or someone pushed in between): once its
  merge has not shown on the branch within `UPDATE_SHOWS_WITHIN` (two
  minutes), the branch is read again and GitHub asked again from its head
  now, which replaces the update kept; still nothing shown fails the
  attempt for a retry, which does the same, so the Step's retries bound
  it. Then the repository's checks round runs on the
  branch, as for `write` and `fix`. It ends `done` only when the branch's
  head is a merge commit whose parents are exactly, in order, the Run's
  reviewed head and the base's commit the update asked GitHub to merge,
  that GitHub made itself: committed as its `web-flow` account with a
  signature GitHub verified (`PullRequests::commit_made`,
  `CommitMade::made_by_github`), the branch's
  head is still that merge, and the checks passed: only the base came in,
  so the Run goes back through `ci` to the merge Gate, which asks again,
  and the merge becomes the reviewed head, for the next update to merge
  into. The reviewed head (`factory_reviewed_head`) is the head a review
  round last passed on (`ReviewRoundsImpl::close_round`) or such a merge;
  nothing else writes it, so asking GitHub again from a head someone
  pushed (`restart_branch_update`) never makes that head reviewed. Where
  the update started does not count: the head GitHub was asked from may be
  anyone's. Anything else ends it `resolved`, which goes back through
  `review` and `accept`, their visits reset, then `publish`, `ci` and the
  merge Gate: an agent's resolution, a fix a checks round asked for,
  someone's push after the merge or during the wait for it (even one that
  merged the base itself), a person's resolution GitHub merged a moved
  base into after their `retry`, a branch that already held its base when
  the update started, and a Run that keeps no reviewed head (its review
  passed before the factory kept one), and a merge made by hand with
  exactly those two parents and changes of its own: it may name GitHub's
  email as its committer, but cannot sign as GitHub. `conflict` when the writer could not resolve it, or what it
  pushed does not hold both the base's head and the branch's former head,
  opens the entry's Gate (no kind, so no rule, stand-in or Orchestrator
  answers it): `stop`, or `retry` once a person resolved it on the branch.
  `retry` runs `update` again, which finds the branch holding its base
  through the person's commits and ends `resolved`, so a person's
  resolution is reviewed and accepted before the merge Gate asks, and a
  `merge_when_green` rule never merges it on CI alone. A `behind` from the
  merge Gate or from `merge` starts the merge Gate's visits over, so a base
  people keep merging into never uses up its three askings; what bounds
  that loop is `update`'s ten visits a Run (a conflict's `retry`s count
  too), then the Run ends for a person. Each update is a full checks round
  and CI run, so ten in a row means the base moves faster than the Run's CI
  can follow, which a person settles (holding merges, or merging it by
  hand); the merge queue keeps other factory Runs from overtaking one at
  the front, so only merges made outside the factory use it up there.
- The merge Gate's approval never carries over an update (a wait in the
  queue keeps it, below): whatever came in, the merge Gate asks again (a `merge_when_green` rule answers it as
  before). Telling "unchanged apart from the base" apart would mean
  trusting a textual merge that can still break the change, and every way
  into `merge` stays that Gate's `approve` (`Flow::check_merges`).
- `merge` enters its base's queue (`factory_merge_queue`) once Control
  Files and the Main Hold let it, keeping its place from the first time it
  reached its merge, and waits while a Run that reached it first is ahead:
  one that has not ended and is at its merge or left it less than
  `QUEUE_PLACE_KEPT` (90 minutes) ago, to update its branch or be asked
  again, so the Runs behind it do not merge first and leave it behind
  again. A Run past that goes on without it, and gets its place back when
  it returns. One Run of a base merges at a time: its turn
  (`MergeQueueRepository::take_merge_turn`, an advisory lock per base,
  taken in a transaction that touches no factory table) is
  taken before it reads the Runs ahead and held through GitHub's merge,
  so a Run that comes back to its place just as another passed its turn
  waits for that merge, then finds its head behind the base, rather than
  merge beside it untested; it is let go on every path, by being dropped. After `QUEUE_WAIT` (25 minutes) still behind someone, `merge`
  ends `queued`, which `code` routes back to `merge` itself with the
  approval it has, asking no one (`Flow::check_merges` lets a `merge`
  entry's `queued` lead only back to that entry). The approval holds only
  while nothing changed: `merge` merges the approved head only while its
  checks still pass (else `changed`, back through `ci` to the Gate), the
  head holds its base (else `behind`), and GitHub's merge names that head
  (a push since is `changed`). `merge` runs at most twelve times per
  approval (each `approve` resets its visits; about five hours of waits),
  then its merge Gate asks again; the Gate's three visits bound a Run's
  queue waits to some fifteen hours before it ends for a person. A Run
  that merged leaves the queue; one that leaves its merge otherwise keeps
  its place. The wait for a Main Hold and the queue's share those 25
  minutes, short of a Step attempt's 30; a hold set while it waited ends
  it `held` at once.

Once its pull request merged, the Run's branch `factory/{run id}` is
deleted, as GitHub's "delete branch after merge" does: by `merge` after
its merge, and by `ready_to_merge` or `merge` when they find the pull
request merged outside the Run, unless the repository has GitHub delete
merged branches itself (`delete_branch_on_merge`) or can no longer be
read. The deletion is best effort: a failure is logged, never the Step's,
and it takes at most a minute, its reads included, so a slow GitHub keeps
the branch rather than the merge Step's time.

Only the Run's own branch is deleted (`PullRequests::delete_run_branch`
takes a Run's id and names its branch, so any name that is not a Run's
branch cannot reach it), and only while it is at the commit merged and no open pull
request is based on it: a branch pushed to since keeps its commits, and
deleting a branch through the API closes the pull requests based on it,
where GitHub's own deletion moves them to its base. Both checks read
GitHub before the deletion, which GitHub makes on no condition, so they
race it: a push, or a pull request based on the branch, made in the
moment between the reads and the deletion is lost with the branch, or
closed.

A Run that ends without merging (cancelled, failed, closed, refused, or
ended for a person) keeps its branch, for a person to look at; nothing
deletes it later, nor the branch of a pull request merged after its Run
ended.

Control Files (`src/domain/control_files.rs`) are the files that decide
whether a repository's checks pass: tests and what they compare against
(snapshots, fixtures), test and lint configuration, CI workflows, package and
build scripts, agent instructions and their tools' settings (`AGENTS.md`,
`CLAUDE.md`, `.claude/`, `.agents/`, `.cursor/`), `.macro/checks.json`,
`.macro/verify.json` and `.macro/setup.json`.
They are glob patterns matched against each changed path from the root,
ignoring case (`*` within a folder, `**` across folders, `**/name` also at
the root). `DEFAULT_CONTROL_FILES` applies until an admin or the owner of
the team of the Run's owner sets the team's own with
`PUT /settings/control-files` (`factory_team_settings`; `GET` reads them
with whether the caller may change them; a null list goes back to the
default, an empty one checks nothing). Only that admin's own signed-in
session sets them, as for a Gate: a shorter list lets an agent change more
unseen. A Run keeps the list, and the team it came from, as they were when
it was created (`factory_run.control_file_patterns`,
`control_files_team_id`), so a change to the team's list later, by anyone,
does not change a Run under way; a Run created before Runs kept them is
checked against the default. "In person" means the owner's own session
(`is_in_person`: no internal caller, API key or token, agent or routine
program, nor code mode) from an account that is not an agent's
(`"User"."isAgent"`, which signs in with ordinary sessions): the service
refuses an agent's account a Gate answer and a settings change, never keeps
its Run as created in person, and never counts it as a team admin. Team settings show them in a Control files section (behind
`enable-factory-runs`). A path says nothing of a change inside a file, so a
check weakened in ordinary source (a lint suppression, an ignored test) is
the review's to catch.

`publish` (on the pull request's head), `ready_to_merge` and `merge` (on the
commit to merge) compare that commit with the base (`PullRequests::changed_paths`) and keep
the Control Files it changed on the Run (`controlFiles` in the Run API):
`named`, those the request asks for, and `tampered`, the others, which make
the Run Tampered. The request asks for a file only by naming its path from
the root, whole (`.github/workflows/ci.yml`), and only when its owner
created the Run in their own signed-in session (`factory_run.created_in_person`):
an agent could otherwise start a Run whose request names the file it means
to weaken. A pattern, a folder or a bare file name asks for nothing. A
comparison GitHub cuts short (300 files), or more files than a Run keeps
(`ControlFileChanges::MAX_FILES`, as a rename lists both paths), is Tampered
and `unlisted`. Approving a Gate (`approve`) of a Tampered Run needs
`approvedControlFiles` in the answer, listing exactly the Tampered files the
person saw (400 `controlFilesNotApproved` otherwise), which only the owner's
own session can send; the answer keeps them
(`factory_gate.approved_control_files`). `merge` reads the commit's Control
Files again and merges a Tampered one only when the latest Gate answer
before it approved every Tampered file (an unlisted commit, an approval
given while it was unlisted). Otherwise it ends `changed`, so the Run goes
back through `ci` to the merge Gate, which shows the files: that route is
in every `code` Flow a Run carries, so a Run in flight when this shipped,
whose Gate approved before its files were read, takes it too, and nothing
in the workflow or the interpreter changed. A pull request merged outside
the Run, found by `ready_to_merge` or `merge`, is recorded as the Run's
merge and its base watched, as one the Run made; it is not compared again, since GitHub's comparison of a merged head
with its base lists nothing: the Steps keep, and name, the Control Files
last read. Nothing but a person's answer approves them.

The `code` Flow checks that a change does what its request asks before
its merge: its verify Phase. It is on for every repository by default,
including one with no verify file. The repository turns it off in
`.macro/verify.json` (`src/domain/verification.rs`): `{"verify": false}`
(`true` when left out). A `deploy` key, which an earlier factory read to
wait for a deploy after the merge, is read and ignored, so
`{"verify": false, "deploy": ...}` still turns the Phase off. A file with
any other key does not read and turns nothing off: the change is checked,
and `accept`'s report and the planner's note say "`.macro/verify.json`
does not read (why): the change is checked". `accept`
and `plan` read the file on the base branch, which the Run has not merged
into. The file is also a default Control File (a team that keeps its own
list must add it).

- On a repository, `plan` asks the planner to end its plan with a fenced
  `json` block of acceptance checks (`built_in_steps/plan_acceptance.md`,
  `src/domain/acceptance.rs`): one per Done-when clause of the request (at
  most 16), each the `clause` and either `run`, a command from the
  repository's root (at most 1 KiB) that exits 0 only when the clause
  holds (`timeout` as a check's), or `none`, why no command checks it. A
  `deployed` mark, which plans recorded before Runs ended at their merge
  may hold, is read and never written: nothing deploys the change now, so
  such a check is never run, and its clause is checked by hand (it would
  fail, with no deployment to reach). Any other key does not read. The
  Step keeps the plan with the checks written out for its Gate, then the
  block as the factory read it (`plan_output`); checks too large to keep
  are dropped with their block, and a note says so. Later Steps read the
  latest `plan` Step's (`AcceptanceChecks::of_run`); checks that do not
  read, or were dropped for their size, are logged, and `accept`'s report
  says so, its verifier checking every clause by hand.
- Plan review (`src/domain/plan_review.rs`,
  `src/domain/built_in_steps/plan_review.rs`): on a repository the planner
  also ends its plan with a size block (`built_in_steps/plan_size.md`), a
  fenced `json` block before the acceptance checks: `size` `fits`, with
  the `files` it touches (`add`, `edit`, `delete`, at most 40) and
  `doneWhen`, each Done-when item of the request quoted with the plan work
  that does it; or `oversized`, with `why` and 2 to 8 `children` (a `name`
  and a `request` each). The `code` Flow's `plan-review` entry runs
  `plan_review` (the `plan-reviewer` Role, which takes `reviewer`'s Agents
  when a definition does not list it). Its mechanical checks read the
  block: a block that does not read, an edited or deleted file the base
  (the pinned commit, else the base branch) does not hold, an added one it
  does, or a Done-when item of the request (the list under its `Done when`
  heading) the block maps to no work is a Blocker, and the Step ends
  `blockers` without asking the reviewer. The Control Files the plan
  changes are flagged, for the reviewer and the Gate. A plan the checks
  pass goes to the plan reviewer, which ends `pass` or `blockers` with a
  `json` block of Blockers (`problem`, `why`). `blockers` goes back to
  `plan`, whose next visit reads the review in its history; the third
  visit, or a pass, ends `reviewed`, after which the plan Gate opens, its
  output the review's result. A person's `revise` resets the entry's
  rounds. An `oversized` plan ends `oversized` when the entry routes it,
  and `split` files each child through `ChildTasks` (as a follow-up task
  is filed: the owner's, in the project of the Run's task, its body
  linking that task), with an id fixed by the Run, the Step and the child
  (`child_task_id`), so a retried `split` files no second task; it ends
  `done` with their links, and the Run ends `manual`. Where nothing files
  tasks (`NoChildTasks`), `split` ends `failed`.
- Trust: the checks live in the plan Step's recorded output, never in the
  branch, so the writer and fixer cannot change the commands. They are an
  agent's commands, not people's: `.macro/checks.json` is written and
  merged by people, acceptance checks by the planner, and they run in the
  verifier's session with the same trust as any agent's command there. A
  Standing Rule or the stand-in may answer the plan Gate, so they can run
  with no person having seen them. What a command runs may be code the
  writer wrote (a test it adds): the verifier is told to read what each
  check runs and confirm it really tests its clause and does not pass
  trivially.
- `accept`, once the review panel passes and before `publish`, is the
  phase before the merge: a verifier (the `verifier` Role, in a session of
  its own, never the writer's session; a definition that does not list the
  Role fills it with `reviewer`'s Agents, and may give it the writer's
  Agent) checks the Done-when clauses on the head of the Run's branch, and
  the factory runs every acceptance check there, as a checks round of one
  round (`built_in_steps/accept.rs` over
  `checks_round.rs`'s exec path: a worktree at the commit, the exit codes
  decide, the round recorded before it runs, its time from the Step
  attempt's start; where the session runs no command the verifier runs
  them and reports). Each check runs after `export MACRO_VERIFY_COMMIT=...`,
  which the round keeps out of the commands it records and shows. It ends
  `pass` when the verifier passed and every check passed, else `blockers`,
  which opens the entry's Blockers Gate (`kind: blockers`): the Standing
  Rules (`fix_blockers`, bounded by `rule_fix_limit` as a panel is, from
  the Step's visit), the stand-in or a person send it to `fix` (with
  `review`'s visits reset, so it never eats review rounds) or stop. At its
  last visit (`max_visits`, three) it ends `unresolved` instead, "acceptance
  kept failing", and the Run ends `manual`, so no fix runs that no
  acceptance would check.

A Run ends at its merge: `merge`'s `merged`, or a merge `ready_to_merge`
found, ends it `success`. What follows a merge, a deploy or a check of the
deployed change, belongs to other applications.

A Run started on a `code` Flow that still went on, once merged, to a
`deploy` Step (a wait for a GitHub deployment) and a `verify` Step (a
verifier checking the merged change) carries that Flow in its workflow's
input, and walks it. The worker still runs both Steps, for such Runs only:
each ends at once (`past_merge_outcome` in `built_in_steps.rs`), `deploy`
with `off` and `verify` with `pass`, the outcomes every such Flow routes to
the Run's `success` (the first one has no `off` from `verify`), so the Run
ends at its next Step. No new Flow may name them: `serves` refuses both,
and only a Run's Flow, in its workflow's input, still does. `verify` keeps
its `verifier` Role, so a session an earlier attempt held is let go. A Run
parked at a Gate inside an old `verify` (its verifier's question, or its
stop without a result) waits as before: an answer or `retry` goes on in a
Step of its own, which ends `pass` at once; `stop` routes `failed` as that
Flow says. Nothing in the workflow changed: its decisions come from the
Flow in its input and the recorded results of its activities, and the
activities a Run schedules stay the same, so a history that ran `deploy`
or `verify` replays as it ran. None of the recorded histories in
`test/replay.rs` reaches those Steps: the claim rests on the workflow code
being unchanged. `interpreter/test.rs` walks both such Flows
(`first_deploy_verify_code.json`, `deploy_verify_code.json`), and an
ignored Temporal test in `inbound/temporal/test.rs` runs one on this
worker to `success`. Their Steps already recorded keep their
outcomes (`deployed`, `not_needed`, `deploy_failed`, `fail`): the Run view
shows them as any Step's, `fail` and `deploy_failed` in the failure tone,
and Insights still count those two as a failed Step.

`accept` is an entry of `flows/code.json` only: a Run carries its Flow in
its workflow's input, so one started before it walks the Flow it started
with and ends at its merge as before (`interpreter/test.rs`,
`review_panel_code.json`). The interpreter did not change. The workflow
changed in one place: a single Step's Blockers Gate tells the Standing Rule
the fix rounds and their limit (`workflow/step.rs`), where it told zero;
no Flow a Run carried before has such a Gate, so every recorded history
asks as it did, and no patch marker is needed.

A Main Hold stops merges while a base branch is red (`src/domain/main_hold.rs`).
`merge` records each merge it makes or finds (`factory_merge`) and signals the
watch of its base to look again (`factory_look_again`, signal-with-start): one
`MainWatchWorkflow` per repository and branch
(`src/inbound/temporal/main_watch.rs`, workflow id
`factory-main-watch:{repo url}:{branch}`), so a merge that lands while a watch
ends is still looked at; the signal names the Run, as whose owner the watch
reads too. A watch that does not start is logged, never the merged Step's
failure. Each look (`factory_look_at_main`) reads GitHub as the owners of the
base's 10 latest merges, newest first, then of the Run that last signalled,
passing to the next when GitHub refuses one (a link that no longer works,
access lost); it looks at once and then every 3 minutes. Only the checks
the base requires decide: one that failed, with no run or status of it that
passed, is red; all passed is green, so a re-run that passes clears it; one
not finished keeps it pending. A required check with no run or status on
the head at all, as one that runs only on pull requests, does not apply once
every other check on the head finished, something was reported, no suite
made in the last 5 minutes is still empty, and `REQUIRED_GRACE` (20
minutes) passed since the base's latest merge: a fresh head whose runs
GitHub has not made yet, or a status a CI outside GitHub posts late, keeps
it pending. A green look counts for a head at or after the latest merge
because the merge is recorded only once GitHub merged it, and a merge
recorded after the look's read began keeps the watch going. Required
checks no owner can read keep a hold a red look set, and otherwise hold
nothing: the base is marked `unread`. A base that requires no checks, as
read in full, holds nothing: a hold is lifted, its task gets a comment
saying why, and the base is marked `none`. Runs at their merge show
either (`baseRequiredChecks`). GitHub that cannot be read leaves the base as
it was. An operator clears a hold no look can clear with `POST
/internal/main-holds/clear` (`{"repoUrl", "baseBranch"}`, the internal key
only, not in the OpenAPI document); a later red look holds it again.
A red look holds the base (`factory_main_hold`) and files one task per red
period, as that Run's owner, in the initiative of that Run's task (its
Project property), naming the factory merges to the base since its checks
last passed, oldest first, with their pull requests
(`outbound/document_hold_tasks.rs`, through the document storage service).
A Run task whose properties do not read, or that the owner no longer sees,
files the task in no initiative (with a warning for the first); a read
document storage fails, times out or refuses the key for fails the filing,
which is tried again. The task is assigned to the owner and not started, as
a follow-up is.
The task is claimed before it is filed, so a retry or a second look files
none; the claim chooses the id it is filed under, kept by every later
claim of the red period, so a filing whose answer was lost is found under
it rather than filed twice; a filing that fails gives the claim back, a
claim never filed is taken again after 15 minutes, and a task filed then deleted is not filed
again in the same red period. A green look clears the hold, unless its
read started before the hold's red one. The watch never ends while the base
is held or a look fails; with nothing held it ends on a green or unread
look, unless a merge landed after the look's read or signalled during it,
and, while checks still run, once the base's latest merge is 6 hours old.
It continues as new every 200 looks. While the base is held, a Run at its
merge (an entry that runs `ready_to_merge` or `merge`) shows the hold
(`mainHold` in the Run API), and `merge`, even once its Gate was approved,
merges nothing: it signals the watch, waits for the hold to clear for up to
`HOLD_WAIT` (25 minutes), then ends `held`, which the `code` Flow sends back
to its merge Gate (the Gate's visits start over), so the person is asked
again with the hold shown. A Run in flight on an older `code` Flow, which
routes no `held`, fails there instead. The pull
request's description names the Run's task as `MACRO-{short id}`, so GitHub's
webhook links them, and is cut to GitHub's 65,536 characters in its request
part only.

Standing Rules (`src/domain/standing_rules.rs`, `factory_records`'
`domain/standing_rules.rs`) let the factory answer a Gate on its own, with
a Rule Answer. A team's Orchestrator Definition says, for each kind of
Gate, whether its rule decides it: `plan` (`approve_plan`: approve the
plan Gate), `blockers` (`fix_blockers`: send a review round's Blockers to
be fixed) and `merge` (`merge_when_green`: merge once CI is green). A
Gate's kind is what its Flow names (`GateDefinition::kind`), never read
from the work of its entry, and a Gate of a kind must offer exactly its
kind's answers (`approve` and `revise`; `fix` and `stop` for `blockers`),
which the Flow's check and the rule both require. The built-in Flows name
the `plan` Flow's and `code` Flow's plan Gates, the `code` review panel's
and its merge Gate. A Gate with no kind, as any other Flow's, is never
answered by a rule. The default, for a team
that set none or an owner on no team, decides `blockers` only, so a Run's
review keeps fixing its Blockers on its own, as before Standing Rules, and
the plan and the merge wait for a person. An admin or the owner of the team
sets the team's with `PUT /settings/standing-rules`
(`{"definition": {"plan", "blockers", "merge"}}`, null for the default;
`factory_team_settings.standing_rules`), and a Project's own (an
initiative's, which replaces the team's for a Run whose task is in it)
with `PUT /settings/standing-rules/projects/{project_id}`
(`factory_project_rules`; null removes it), in their own signed-in session,
as for Control Files; a Project's also needs the admin to be able to edit
the initiative (400 `UnknownProject` otherwise, as for one that does not
exist). `GET /settings/standing-rules` reads the team's, or the default,
with its Projects' and whether the caller may change them. A Run keeps
the rules it was created with, and where they came from
(`factory_run.standing_rules`, `standingRules` in the Run API): its task's
Project's when its owner's team set some, else the team's, else the
default (a Project that cannot be read then gives the team's, with a
warning, rather than refusing the Run); an agent's account never may
change them (`canEdit`); a change to either later, or a task linked later, does not change
a Run under way.

The `code` Flow's review panel has a Gate `after` its round ends
`changes` (`GateDefinition::after`, which must be an outcome a round ends
with: `pass`, `changes`, `unresolved` or `failed`), on the round's latest
Step (its last reviewer's, or the last that went on from a reviewer's
question): `fix` sends the Blockers to `fix`, `stop` ends the Run `manual`.
A round with a reviewer's question waits for the answer, and closes once
the reviewer went on with it; only then does the Gate open
(`inbound/temporal/workflow/review_round.rs`). A round at the panel's last
visit routes `unresolved`, which asks nothing.

Once a Gate is recorded, and before a person may answer it, the workflow
asks the worker (`factory_rule_gate`, behind the `factory-standing-rules`
patch marker) whether the Run's rules answer it. The worker reads the Run:
no rule answers a Gate the Run's own Flow keeps for a person
(`person_only`, as the `improve` Flow's plan and merge), no rule approves a Tampered Run's plan or merge (a Rule Answer approves no
Control Files, so `merge` would not merge it either), though its Blockers
may still go to a fix, which approves nothing; `merge_when_green` answers
nothing while the base branch has a Main Hold (and `merge` holds anyway);
and `fix_blockers` sends at most `rule_fix_limit` fixes in a row since the
panel's visits last started: two (`MAX_RULE_FIX_ROUNDS`), and never more
than the rounds before the panel's last. With the `code` Flow's three
rounds the cap is never what stops it: the rule fixes the first two
rounds' Blockers, and the third round's route `unresolved`, which ends the
Run `manual` for a person. A panel bound to more rounds
(`FACTORY_REVIEW_ROUNDS`) reaches the cap, and its next Blockers wait at
the Gate for a person, who may send them to another fix or stop. The
worker writes the Rule Answer only on a Gate with no answer and reads
back what the Gate keeps, which is what it returns. A Rule Answer is
recorded on the Gate with its rule
(`factory_gate.rule`, `rule` on the Gate's answer, with no author) and
routed as the entry's outcome; the activity's result carries it, so a
replay reads it from the history. An attempt that finds the Gate answered
returns that answer, so a retry never decides again. A Gate no rule answers
waits for a person, as before. A Run created before Runs kept rules has
none, so no rule answers its Gates; a Gate opened before a worker had the
marker replays without asking. A rule therefore never answers a Gate opened
before the rules it would apply were kept. A question Gate is never put to
the rules: the workflow asks `factory_rule_gate` only about a Flow's Gate
(its kind comes from the Flow alone, and a question Gate has none), and the
worker answers none for a Gate `GateKind::answered_only_by_a_person` keeps
for the owner, whatever the ruling names.

A Gate left for a person (a Flow's Gate no rule or stand-in answered, or
an agent's question) notifies the Run's owner by default
(`src/domain/gate_owner.rs`): once the workflow takes its answers it calls
`factory_notify_gate` (behind the `factory-gate-owner` patch marker; tried
for at most five minutes, and a failure leaves the Gate waiting), and the
worker sends a `factory_gate_waiting` notification through the
notification ingress (`outbound/ingress_gate_notifier.rs`), pushed to the
owner's phone, that opens the Run, with an id derived from the Run and the
Step, so a retry is one notification. It is about the Run's task, or, for
a Run started without one, about the owner (the metadata's `taskId` is
then empty). A Run whose Orchestrator Definition is quiet (`quiet_gates`,
`quietGates` in `PUT /settings/standing-rules` and its Project route; off
by default and when a definition leaves it out) notifies no one: its Gates
still wait on the owner and are listed as waiting. The setting is an
opt-out: a team or Project that never set rules, the factory's default and
a Run created before Runs kept rules notify. Only the owner section
notified before; notices are now on for everyone and turning them off is
new, so `20261119010000_factory_quiet_gates.sql` only allows the name and
makes no kept definition quiet: one with the owner section off, a Run's
under way included, notifies from now on. Rolling the service back past
`quiet_gates` once one is stored is not clean: the older build reads a
definition naming it as no definition at all (`from_kinds` returns `None`),
and its `deny_unknown_fields` refuses a `PUT` carrying `quietGates`, as with
every name added to the definition before it. The marker and the activity run for every
Gate left to a person, whatever the definition says: a quiet Run's worker
sends nothing, so the workflow and its histories are as they were. Once
the Gate is answered (`factory_answer_gate`, or `factory_read_gate_reply`
for words, by whoever answers), or the Run ends with it open
(`factory_end_run`), the worker marks its notification done through the
notifications' reader, as the owner marking it done would, which also
clears its push. The ingress creates the notification only when it reads
the message, so an answer or a cancel within moments of the notice can
come first: when there is no notification to mark, the worker withdraws
its id (`NotificationWithdrawal`, a row in `notification_withdrawal`), so
the ingress never creates or pushes it, and the egress drops a push queued
before the withdrawal; it then marks it done again in case it was created
in between. So the worker closes only notices that may have gone out
(`gate_owner::notice_may_be_out`, `stand_in_notice_may_be_out`,
`told_entries`): no waiting notice on a quiet Run, none for a Gate a rule
or the stand-in answered or while the stand-in's answer stood, a stand-in
notice only for a Gate the stand-in answered, and a notify-only notice only
for the entries the Run's Flow gives a notify-only Gate, with a task or
without one. A notice marked done
leaves no row; one closed before the ingress created it leaves one small
row for good: only the rare notice that lost that race, at most one per
Gate, notify-only entry and stand-in answer of a Run. A Gate a
Standing Rule or the stand-in answers was never left to a person, so it
sent no such notification. `GET /runs/waiting` lists the caller's Runs
waiting at an unanswered Gate, whatever the definition says.

A Run started without a task files its notices under the owner's own User
entity: a waiting Gate's, a notify-only Gate's and the stand-in's, under
the same ids as a Run on a task's, which the notification service keeps
once (`ON CONFLICT DO NOTHING`). The inbox lists a User's notices only to
that user (Soup's `user_gate`), each such Run on a row of its own
(`notification_inbox`'s `place_about_the_viewer`), so the notices reach
the owner in the inbox as well as live (and, for a waiting Gate, by push),
open the Run from there, and are marked done or withdrawn as a task's are,
the lighter ones once the Run ends at the latest. Such a Run's waiting Gate
also shows in "Waiting on you" (`GET /runs/waiting`), which lists every
waiting Run, with a task or without. A Run without a task that was already
running when Runs without a task began sending the lighter notices sent
none before then, so it closes notices it never sent: each leaves one
harmless withdrawal row, at most one per Gate, notify-only entry and
stand-in answer.

A team's Orchestrator Definition also has an owner section
(`owner_gates`, `ownerGates` in `PUT /settings/standing-rules` and its
Project route; off by default and when a definition leaves it out, so a
team's or a Project's definition saved before it keeps it off until an
admin turns it on there), kept on the Run with its rules
(`src/domain/gate_owner.rs`). With it on:

- a review round whose Blockers are the round before's, unchanged, after
  the `fix_blockers` rule sent those to a fix, is left for the owner
  rather than fixed again (`standing_rules::repeated_blockers`), on top of
  the rule's fix cap. The round before is the latest earlier round of the
  same review panel (the entry of its first Step) one visit before it.
  "The same" is `review_rounds::same_blockers`: every
  Blocker of each round has one in the other found by a reviewer of the
  same Role, in the same file, at lines that meet (no lines is the whole
  file); what a Blocker says is not compared, and an answer that never
  read is no finding. A person's fix, and a round after a reset, are never
  compared;
- the owner's comments on the Run's task since its latest Gate opened
  reach every later agent Step's prompt, and the prompt of a Step that
  goes on from a question, fenced as the owner's notes
  (`owner_notes_brief`, escaped as reviewers' findings are; a single note
  past the budget is cut). The comments the factory posts as the owner (a
  follow-up's "reported again", a Main Hold task's, a Run's task thread) end with
  `FACTORY_SIGNATURE` (`factory_signed`), and are left out. Macro marks
  only a bot's comments as its own: one an agent session, a routine or an
  API call posted as the owner reads as theirs, so the notes may include
  agent-written text. They decide nothing, are fenced as data, and the
  brief says so. They are read
  as the owner through document storage's `/messages/document/{id}` routes
  (`outbound/document_task_notes.rs`); a task the owner no longer reads has
  none, and an outage fails the Step's attempt, for a retry.

With it off, as for a Run created before it, none of this happens.

The Orchestrator Definition's task thread section (`task_thread`,
`taskThread` in the settings routes; off by default and when a definition
leaves it out, kept on the Run with its rules) posts a Run's main updates
to its task (`src/domain/task_thread.rs`, the words in
`task_thread/comments.rs`), as its owner, through document storage's
`POST /messages/document/{id}` (`outbound/document_task_thread.rs`), asking
it to notify no one (`notification_policy=Silent`, which the messages
routes honour for an internal caller only): one comment thread per Run,
whose first comment links the Run (`TaskThread::run_link`, the app's
`/app/factory/runs/{id}`; it names neither the repository nor anything
else of the Run), and one reply per update: each Step's end (a built-in
Flow's `publish` or `merge` Step, by its entry, names its pull request),
each review round as one comment once it closes, with its outcome, its
Blockers and each reviewer's result (its reviewers' Steps are not told one
by one), a Gate left for a person (an agent's question says only which
Role asks, never its words), a Gate's answer and who or what gave it (the
person by the name they show others, `RunRepository::display_name`, else
"the run's owner"; read from their words or not; the Standing Rule; or the
stand-in), a notify-only Gate's notice, and the Run's end. A Run with no
task posts nothing. The worker posts each from the activity that records
what it tells, once that record is written: `factory_start_step` (the
first Step), `factory_end_step`, `factory_close_review_round`,
`factory_notify_gate`, `factory_answer_gate`, `factory_rule_gate`,
`factory_keep_stand_in`, `factory_read_gate_reply`, `factory_tell_gate`
and `factory_end_run`, so the thread adds no command to the workflow and
Runs in flight replay as before. Each comment's id is a UUIDv7 of when its
update happened, whose other bytes come from the Run and the update
(`task_thread::comment_id`): a retried activity posts it again and
document storage refuses it as a conflict, which counts as posted, so each
update is one comment. Document storage takes a comment's own id only
within a day of its time, so a first comment never posted cannot be posted
under its first id once the Run is a day old: a reply whose thread is not
there looks for the Run's thread among the task's discussions as the owner
(`TaskThread::find_thread`: the owner's, signed, linking the Run), replies
there, and only when there is none posts a new first comment with an id of
the current time, which the next reply finds the same way. Posting never
fails or holds the Run: it is bounded by `POST_TIMEOUT` (15 s), and what
cannot be read or posted is logged at warn level and dropped. Every
comment ends with `FACTORY_SIGNATURE`, so the owner's notes leave it out;
a Flow's question is quoted escaped, on one line, with no link, image or
web address. Messages records a post as a bot's (`triggered_by`) only for a
bot caller, so these comments read as the owner's, marked only by the
signature.

Gate replies (`src/domain/gate_replies.rs`, `gate_replies/service.rs`):
a person may answer a Gate in words rather than with a button, with `POST
/runs/{id}/steps/{seq}/reply` (`{"reply"}`, 1 to 4096 bytes, else 400), in
their own signed-in session as for an answer. The workflow takes the words
with the `factory_reply_gate` update (refused, as an answer is, for a Gate
the Run does not wait on, after an answer or other words it has not read,
and for a Gate opened before replies) and asks the worker to read them
(`factory_read_gate_reply`): a small model (the `GateModel` port; on
Macro's model router in `outbound/fast_gate_model.rs`, Claude Haiku 5.5
(`DEFAULT_GATE_MODEL`) unless `FACTORY_GATE_MODEL` names another,
`AiFeature::FactoryGate`, its usage recorded against the Run's owner but
not counted toward their allowance; each call's usage row names the Run as
its entity (a replay's names none) and is priced from `ai_pricing`, and
its prompt, which no later call shares, is written to no prompt cache)
picks one of the Gate's answers or `unclear`, with how sure it is. The
question, its answers and the words reach the model as one JSON object,
so the words open no section of their own. At or above the reply
threshold (`DEFAULT_REPLY_THRESHOLD`, 85 percent, or
`FACTORY_GATE_REPLY_THRESHOLD`) the Gate is answered with it as
the person's answer, the words kept as its note and the confidence beside
it; below it, `unclear`, an answer the Gate does not offer, a call that
fails or takes longer than `MODEL_TIMEOUT` (30 s), the factory asks back on
the Gate (`asked_back`, with how the model read the words: its pick, how
sure it was and why, none of them when it did not answer, and whether
the words approved a Tampered Run, which only the Approve button does:
`refused_tampered`), which waits on
and takes an answer or new words.
Words never approve a Tampered Run: its Control Files are approved with the
button, which lists them. An agent's question takes the words as its
answer, with no model (the API sends them as the Gate's `answer`, so any
worker takes them).

A comment on the Run's task never answers a Gate. A comment that an agent's
session, a user API key or a routine posts as the owner reads, in the
message service, the same as one the owner wrote: messages only mark a bot
as the sender (`triggered_by`, `triggered_via` and `bot_profile` are a
bot's), so a comment could approve a plan or a merge, or answer an agent's
question, in the owner's name. Reading comments as replies needs the
message service to record how the session that posted each comment
authenticated (in person, or a program acting for the person), as the
Run API's `is_in_person` does for an answer.

The stand-in (`src/domain/stand_in.rs`) answers a routine Gate for the
Run's owner once no Standing Rule did, and before the person: a team turns
it on per kind of Gate in its Orchestrator Definition (`standIn`: `plan`,
`blockers`; it has no setting for a merge Gate), a Project's own replacing
it, and a Run keeps it with its rules; it is off by default. The workflow
asks the worker (`factory_stand_in_gate`) about a Flow's Gate of a kind;
the worker answers none for a merge Gate, an agent's question, a Gate its
Flow keeps for a person (`GateDefinition::person_only`), a Gate of a
Tampered Run or under a Main Hold of its base, and a Run created before
Runs kept rules. It asks the model with the Gate, the Run's request, the
asking Step's output, the plan (the latest planner's Step before it) and
the owner's last `PAST_ANSWERS_READ` answers to the same entry's Gates in
Runs of the same Flow (never an agent's question asked there), as one JSON
object, so nothing an agent or the request wrote can pass for a section of
its own, such as a past answer. At or above the stand-in threshold (`DEFAULT_STAND_IN_THRESHOLD`, 90
percent, or `FACTORY_STAND_IN_THRESHOLD`) its answer, reason and confidence
are kept on the Gate with the end of its window (`DEFAULT_STAND_IN_WINDOW`,
10 minutes, or `FACTORY_STAND_IN_WINDOW_SECS`, from 60 seconds to a day,
else the service does not start), and the Gate takes the owner's answers
and words meanwhile: one in the window is the Gate's answer, as any other,
with the stand-in's kept beside it. Once the window's timer fires the Gate
closes and the worker reads the Run again (`factory_keep_stand_in`): when
it still lets the stand-in answer, the stand-in's answer becomes the Gate's
(`by_stand_in`, with no author); when it no longer does (the branch turned
Tampered, the base is held, the Run ended), the stand-in's answer is taken
back and the Gate waits for the owner. Below the threshold, unsure, or
failing, it keeps no answer, and the Gate waits for the person, keeping
how the model read it (`stand_in_declined`: its pick, how sure it was and
why, none of them when it did not answer), which the
Run view shows the owner. A retry returns the answer kept first, with what
is left of its window. While the window is open the owner is told, lightly,
whatever the definition says (`gate_owner::stand_in_notice`): a
notification about the Run's task (for a Run without one, filed under the
owner, as above), of the waiting Gate's kind flagged
`notifyOnly` as a notify-only Gate's, not pushed to their phone, saying
what the stand-in answered (by the answer's label,
`gate_owner::answer_label`, which mirrors the web app's `flowWordLabel`:
keep the two alike) and that they can change it in the Run until its
window closes (the Run view says when),
one per Gate (`gate_owner::stand_in_id`, apart from the Gate's waiting
notice); it is marked done once the window closes, the owner answers in
it or replies in it (words asked back end the window too), or the Run
ends. Run insights count neither its
answers nor Rule Answers.

A notice is sent through the notification ingress, a queue, while it is
marked done at once through the notifications' reader. So a notice
marked done in the moments before the ingress creates it, as when the
owner answers within seconds of the stand-in, or of being told a Gate
waits, is created afterwards and stays open until they mark it done:
it then opens a Run that no longer waits. The waiting Gate's notice
(`notice_id`) has the same race: a mark made before the notification
exists changes nothing, and nothing retries it.

The model calls are activities, so a replay reads their results from the
history. The stand-in and replies are behind the `factory-gate-replies`
patch marker, asked once per Gate after its rules:
a Gate opened before the marker takes only an answer, as before, and
replays without them. The thresholds come from the replay measure
(`gate_replies/measure.rs`, the `factory_gate_replay` command of
`services/factory_service`), which runs the model over the Gates people
answered themselves with a button (never an answer read from words, nor
the stand-in's, which are the model's own): each Flow's Gate answered with
a note, its note read as the reply, and each built-in Flow's plan and
Blockers Gate, asked as the stand-in would be with the Run's plan and the
owner's earlier answers, and reports at each threshold how many the model
would have answered, agreed on and picked wrong. It sets a threshold only
from `ENOUGH_GATES` (30) Gates read, and only one with no wrong pick among
at least `ENOUGH_DECIDED` (20) Gates the model answered at it. On
2026-11-18 the factory's records on the test bench held 48 Gates people
answered, from 27 Runs: 12 answered with a note and 12 built-in plan
Gates, too few to set a threshold from (and no model key was at hand to
call the model), so the conservative defaults stand: 85 percent for
replies and 90 for the stand-in. Run the command again once the records
hold more answered Gates, before lowering either.

The workflow asks about a Flow's Gate in this order, once it is recorded:
the Standing Rules (`factory-standing-rules`), then the stand-in
(`factory-gate-replies`), then, only once the Gate waits for a person, the
owner's notice (`factory-gate-owner`). A Gate the stand-in answers, or the
owner answers in its window, is told to no one as a waiting Gate (only the
stand-in's light notice above); one whose stand-in answer is
taken back, or whose owner's words in the window are asked back, waits for
the owner and tells them then, once: words asked back on a Gate they were
already told of tell them nothing more. The stand-in never answers a
Blockers Gate the rule leaves to the owner: past the fix rounds the rule
may send in a row, or, with the owner section on, Blockers found again
unchanged after a fix (`standing_rules::repeated_blockers`); that Gate
waits for the owner, who is told of it. A Run that ends while a Gate
waits (cancelled, or ended for a person) marks that Gate's notification
done in `factory_end_run`, as an answer would. An answer read from a
person's words marks the owner's notification done, as a button's answer
does. The owner's notes are their comments on the Run's task; a person's
words on a Gate are kept as that Gate's note and reach no agent as an
owner's note.

Automations (`src/domain/automations.rs`, `automations/service.rs`,
`inbound/automations.rs`) start Runs on their own: a team member makes one
(`POST /automations`, in their own signed-in session, from an account that
is not an agent's) with a trigger, a built-in Flow, the repository for a
Flow that works on one, and a request template. Its Runs act as its
creator, with their permissions: each is created as `create_run` would
create it for them outside their signed-in session (their task access,
their linked GitHub account, their team's Control Files and Standing Rules,
no Control File named), and keeps which Automation started it, with its
name and trigger kind as they were (`factory_run.automation_*`,
`automation` in the Run API). A Flow and repository the creator could not
start a Run of now are refused when it is made (`check_run_order`). The
triggers:

- `schedule` (cron and time zone), `repository_event` (GitHub event names
  and optional actions, on the Automation's repository) and `task_event`
  (a task property and the values it must gain: the status becoming one,
  a tag added) are a Macro routine of the creator's, made through the
  scheduler's API with the internal key as the creator
  (`outbound/routine_automation_triggers.rs`, the `AutomationTriggers`
  port): a cron trigger, a `github` event filter on the repository, or a
  `task.property_changed` filter with a `property` selector. The factory
  keeps no scheduler. The routine's one code step calls
  `macro.factory.fireAutomation` (`POST /automations/fire`), skipping a dry
  run; the factory takes the routine from the credential the scheduler
  signed for it (`agent_correlation::context::current_routine`, never the
  body) and fires the Automation of that routine only for its creator.
  A webhook (which carries no login) fires it as its creator, unchecked by
  Standing Roles, through its own secret, never through a routine. The
  routine is made as an `Agent` routine, so the scheduler signs its code
  step for the Macro routine bot (`MACRO_NEW_BOT_ID`,
  `00000000-0000-0000-0000-00000000a2a2`). A run the scheduler started
  itself (a schedule its dispatch found due, or a watched event; never Run
  Now) also signs the scheduler's proof
  (`agent_correlation::context::current_scheduler_run`, the token's
  `scheduler_run`): the run's id and the routine's configuration revision,
  which every change to the routine, by anyone, moves. The Automation keeps
  the revision the factory last wrote the routine at
  (`factory_automation.routine_revision`: on making it, and on switching it
  on, which writes the whole routine back, `AutomationTriggers::rewrite`).
  Only a firing whose proof names this run (its key is the run's id) at
  that revision, signed for the Macro routine bot, fires it as its creator,
  unchecked by Standing Roles, with no Role for that bot
  (`routine_acting`); the firing key makes a proof fire once. Any other run
  fires it as an agent (`program_agent`): one for another bot or in an
  agent session as that agent, whatever proof it carries; Run Now (by a
  person or an agent), a run of a routine changed since, a proof of another
  run, and a run that signs no bot (a routine switched to write as its
  owner) as the Macro routine bot. Under a Definition with Standing Roles
  that agent must fill one that allows `start`, else the firing is skipped
  as `refused`; the Run keeps the agent as its start. An Automation made
  before the revision was kept has none: its routine's firings act as the
  Macro routine bot until its creator switches it off and on. A routine the
  scheduler's SDK code upgrade rewrote keeps its revision, but its runs carry
  no proof (`code_rewritten`): code a model rewrote, from a prompt that can
  hold team-written task content, is no longer the factory's, so its
  firings act as the Macro routine bot until the creator switches the
  Automation off and on. The firing key must be the proof's run id exactly
  as Macro writes it (lower-case, hyphenated): another spelling of the same
  id is another firing, so it is not the scheduler's own. A
  routine the scheduler refuses (an invalid cron, event routines the
  creator may not have, no linked GitHub account) takes its Automation
  with it, and the refusal says why (400). Only people's actions start
  event routines, so a task an agent moves starts nothing. A Run's own
  pushes and pull request are made with its owner's GitHub token, so
  GitHub names the owner, not a bot, as their sender: the code step hands
  over the branch a delivery is about (a push's, a pull request's head, a
  workflow run's or check suite's head branch), and one about a Run's
  branch (`factory/{run id}`, `is_run_branch`) starts nothing
  (`own_branch`). A delivery that names no branch, such as a comment, is
  not told apart. The routine is named and described as managed by the
  factory; the Routines page still shows and edits it like any other, and
  an edit there is not read back: it moves the routine past the
  Automation's revision, so its firings act as the Macro routine bot until
  the creator switches the Automation off and on, which writes the routine
  back.
- `webhook` is fired by `POST /automations/{id}/webhook`, which carries no
  login: the call is signed with the Automation's secret (shown once, when
  it is made) as Macro signs its own webhooks (`webhook_signature`:
  `x-macro-signature: v1=<hex>`, an HMAC-SHA256 of `"{timestamp}.{body}"`,
  and `x-macro-timestamp` in Unix seconds), within `WEBHOOK_TOLERANCE_SECS`
  of now (401 otherwise, the same answer as for an id no webhook has, after
  one read, so ids cannot be told apart). The body is empty or
  `{"taskId"}`. The secret is kept in the clear, as Macro's own webhooks
  keep theirs (`webhook.signing_secret`); encrypting it would follow
  `agent_api_keys`' KMS cipher, which needs a KMS key the factory does not
  have yet.

Each firing is keyed (`factory_automation_firing`): by the routine run's id,
so a retried run starts one Run, or by the SHA-256 of the timestamp and body
a webhook call signs (never the signature as sent, whose hex digest reads
in any letter case), so a replay within the tolerance is refused (409) and
one past it fails the timestamp check. A claim that never ended is taken
over after `FIRING_CLAIM_STALE_AFTER` (30 minutes, three times the ten
minutes a signature is taken), and a Run its first worker started under
the key is found then (`factory_run.automation_firing_key`, unique per
Automation), never started again. A firing starts nothing, and records why
(`skipped`), when the Automation is switched off (`disabled`), it is a
repository event about a Run's own branch (`own_branch`), its creator is
no longer on its team (`creator_left_team`) or their account is an
agent's (`agent_account`), the queue takes no more of the creator's Runs
(`queue_full`: the Run API refuses the Run with `QueueFull` at the
limit of the rules it would start under, the one the creator's own starts
meet, counted as the Run is kept), a Run already runs on its task,
whoever started it (`task_busy`: checked before the Run is created, and
held by the unique index `factory_run_automation_task_open_idx` between
two firings that race; a person's Run started in the same instant can still
overlap one), or the Run API refuses the Run as it would refuse its creator
(`refused`). An outage leaves the firing claimed, for a retry once the
claim is stale. A task a webhook names, or a Task event is about, is
checked as the creator sees it, not against the Automation's team: tasks
carry no team to check. A team's members list its Automations (`GET
/automations`); its creator, or an admin of the team, switches one off
(`PUT /automations/{id}/enabled`) or deletes it (`DELETE
/automations/{id}`), and its routine follows, as the creator. Only its
creator switches it on, in person from an account that is not an agent's,
as for making it, since it then starts Runs as them again. An Automation
goes with its team and its creator's account (the tables' cascade); its
routine, left behind, is switched off the next time it fires and finds no
Automation, and stays for its owner to delete. The workflow
is not touched: a Run an Automation starts walks its Flow as any other, so
every Run in flight replays as before.

A team's Factory Definition (`src/domain/definition.rs`) is the files, in a
GitHub repository the team names and changed by pull requests, that hold its
factory: `factory.json` (the format's `version`, 1, and its Flows by name),
`flows/{name}.json` (each Flow, in the built-in Flows' format, checked as they
are), `agents.json` (`FACTORY_AGENTS`' format; it must fill every Role its
Flows name; its optional `standing` section holds the Standing Roles), and the optional `rules.json` (`standingRules`, an Orchestrator
Definition, and `controlFiles`, each the factory's default when left out) and
`scorers.json` (the team's Scorers, Experiments and Benchmarks, in
`FACTORY_EVALUATION`'s format, naming only its own Flows and Agents; see
Evaluation above). No field takes a secret, and a file
holding something shaped like a key or token (`ghp_`, `sk-ant-`, a private
key, ...) does not read. An admin of the team names it, in person, with `PUT
/settings/definition` (`{"repoUrl"}`, null for none;
`factory_team_definition`), which reads it first as them and refuses one that
does not read (400 `invalidDefinition`, naming the file and the problem, or
`unreachableDefinition`); `GET` reads it at its head as the caller and says
why it does not read (`problem`). It also names the commit the newest Run of
the team's members started under it read (`newestRun`,
`RunRepository::newest_definition_read`), shows read-only what the
Definition holds at that commit, or at its head when no Run read it yet or
that commit does not read as the caller reaches it, for any reason (not
linked to GitHub, the commit gone, GitHub down or limiting them: logged,
never the setting's failure; none when the head does not read either)
(`contents`: Flows with their entries, Roles with their Agents, Standing
Rules and Scorers), and lists the repository's
open pull requests (`openPullRequests`, at most
`DEFINITION_PULL_REQUESTS_LISTED`; null when GitHub fails to list them,
which does not fail the setting). While a team names one, `POST /runs`, `GET
/flows` and an Automation's check read it as the Run's owner at the commit its
default branch is at (`PullRequests::branch_head`, then `file_text` at that
commit; `service/definition.rs`): the Run walks the Definition's Flow of that
name (a built-in Flow it does not name is `UnknownFlow`), with its rules
(`RulesSource::Definition`: neither the team's settings nor a Project's own
apply) and Control Files, and keeps the repository, the commit
(`factory_run.definition_*`, `definition` in the Run API, which the Run view
shows) and the Agents (`definition_agents`), which its agent Steps take
(`SessionAgent::run_agents`, read and checked again for each Step, so a
kept definition that no longer reads fails the Step, `DefinitionAgents`)
in place of `FACTORY_AGENTS`. Every Run keeps the Flow it walks
(`factory_run.flow_definition`) and whether it has a Gate of its own
(`flow_asks`): a Gate's kind for the Orchestrator, whether a Run is at its
merge, which entries name its pull request on the task thread, the replay
measure's stand-in kinds and its Queue place before its first Gate are read
from it (`built_in_flows::walked_flow`; a Run kept before falls back to the
built-in Flow of its name). A Definition's Flow must merge only through a
merge Gate (`Flow::check_merges`, which a test holds the built-in Flows
to): a Gate on `ready_to_merge` or `merge` is of kind `merge`, a `merge` Gate is only on
`ready_to_merge`, every way into `merge` is that Gate's `approve` (or a
`merge` entry's `queued` back to itself, which keeps that approval), and the
entry above `merge`, with no `next`, is that Gate's, never skipped and
routing every other outcome; so the Orchestrator never approves a merge
and only the `merge_when_green` rule merges without a person. A Run on its
team's Definition repository adds the Definition's own files
(`DEFINITION_FILE_PATTERNS`) to its Control Files, and a change to one is
Tampered even when the request names it (`ControlFiles::changes_guarding`),
so a person approves it; `rules.json` lists at most
`MAX_DEFINITION_CONTROL_FILES` (195) patterns to leave them room. `GET
/settings/definition` says whether the default branch takes changes only by
pull request (`requiresPullRequest`, from its rulesets or, for its admins,
its branch protection), and the Control Files and Standing Rules settings
say a Definition is named (`definitionNamed`). Only the default
branch is read, so a change reaches Runs once its pull request merges, and a
Run started already, or a Run's own branch, never changes the rules it runs
under. The team's Control Files and Standing Rules settings are refused
meanwhile (409 `definitionNamed`) and read as not editable. A team that names
none takes its settings and the built-in Flows and Agents, as before. The
Flow goes into the workflow's input as a built-in one does, so the workflow
and the interpreter did not change, and Runs in flight replay as before. A
starter Definition is in `definition_template/` (the built-in Flows, with no
post-merge Steps in `code`; `primary` and `backup` Agents whose bots must be
replaced: `read_definition` refuses a placeholder bot id, all zeros but its
last two digits (`is_placeholder_bot`), so the template as shipped does not
read; a person merges), which a test reads with its bots replaced; `factory_definition_check`
(`services/factory_service`) checks one in a folder.

An admin of the team changes the Definition from the factory's settings
with `POST /settings/definition/changes` (`{"changeId", "title",
"description"?, "files": {path: text}}`; `service/definition/change.rs`),
which answers the pull request it opened (`DefinitionPullRequest`, with
GitHub's own title and draft state). Only an admin, in person from an
account that is not an agent's (403 otherwise, as for naming the
Definition, and before the change is looked at), proposes: editing it
changes the rules every Run of the team runs under. The change's pull
request is looked up first, from its branch: once there, it is the answer
whatever happened since (open or merged), and nothing is checked or
written again, so a resend after a merge, or after the Definition moved
on, is no error, and a late resend never brings back a branch that would
undo later changes; one closed without merging is 409
`definitionChangeClosed`, never opened again. Otherwise the change is
checked as the Definition would read with it at its default branch's head
(`read_changed_definition`, the files it writes read over the others with
the same `read_definition` a Run's start and `factory_definition_check`
use; 400 `invalidDefinitionChange`, naming the file and the problem). Each
file it writes must be one the Definition then reads (`NotRead`: a Flow's
file the manifest does not name is never checked, so a secret could hide
in it), at least one must differ from the head's (a head file that cannot
be read, too large or not text, differs, so a broken Definition can be
repaired), it writes 1 to `MAX_CHANGED_FILES`, and its title is joined
into one line. It is committed, with the admin's linked GitHub account,
which must push to the repository (400 `noPushAccess`; a link GitHub no
longer takes or a repository hidden from them is 400
`unreachableDefinition`, `PullRequests::access_refusal`), as one commit
over the head on its own new branch, `definition-change/{changeId}`
(`PullRequests::create_branch`, which only ever creates a branch: one
already there, the default branch included, is left as it is), and
opened as a pull request onto the default branch, which a person merges;
nothing is pushed to the default branch. A branch GitHub declines (a rule
of the repository) or finds there already with no pull request (what it
holds was never checked), and a pull request it will not open, are 409
`definitionChangeNotOpened` with GitHub's reason. A team that names no
Definition has none to change (409 `noDefinition`). As for any pull
request on the Definition, the change reaches Runs only once it merges.
Each proposal is checked alone, against the head it was made at: two
pull requests that each read may not read once both merge (one adds a
Flow, the other drops the Role it names). Run `factory_definition_check`
in the Definition repository's CI, and require it on its default branch,
so the merged result is checked too.

Self-improvement (`src/domain/self_improvement.rs`): a team's Definition
learns from the Runs that failed under it. A failure is a live Scorer's
judgment below `PASS_SCORE` (0.5), or a Run its owner marked flawed, of a
Run that started under the Definition the team names now and is not itself
a self-improvement Run (`collect`). Each is kept once per Run and source
(`factory_improvement_failure`), pending until a self-improvement Run takes
it. The scoring activity the workflow already calls once a Run ends
(`factory_score_run`) runs the Scorers, then `SelfImprovement::after_run_end`
(`ImprovingScoring`): it keeps the Run's failures, settles the team's open
self-improvement Run, and, unless the team is paused (`paused_until`), starts
one when the pending failures reach the `selfImprovement` thresholds in
`scorers.json` of the team's Definition as it is now, read at its head as
the namer, as Insights reads it (`failures`, 25, or the oldest
`oldestDays`, 7, old; `enabled`, on), never a copy an ended Run kept; the
worker keeps what it read per team and Definition for
`THRESHOLDS_KEPT_FOR` (5 minutes), so a team's Run ends call GitHub at
most that often.
Failures are kept while `enabled` is off, so they count once it is on
again. After a self-improvement Run that settled without merging (closed
or cancelled by a person, ended without a pull request, refused, or kept
starting too long), the next waits `UNMERGED_PAUSE` (a day) from that
settlement, doubled for each more such Run in a row since the last merged
one, up to `MAX_UNMERGED_PAUSE` (a week); a merged one ends the wait. The
pause is read after settling, so a rejection never starts another in the
same call, and no improvement loops unattended. `POST /runs/{id}/flaw`
keeps the flaw at once; it counts at the next Run's end. A team that names no Definition keeps and
starts nothing, and neither do Runs of any other team. Nothing in the
workflow changed, so Runs in flight replay as before; a failed
self-improvement step fails the scoring attempt, which is retried with it.

The Run (`create_improvement_run`) walks the built-in `improve` Flow
(`flows/improve.json`: `code` without `accept`, which has no task to verify)
on the Definition's repository, owned by the admin who named the Definition
(`factory_team_definition.named_by_user_id`) while they are still an admin
of the team (`namer_standing`); none starts for a Definition with no namer
kept (named before this), a namer who left the team, or one no longer an
admin, which Insights says (`namer`). It is created as an Automation's Run is, not in person, and
counts in its owner's Queue. A team has at most one starting or open
(`factory_improvement_open_idx`): it claims the oldest
`MAX_GROUPED_FAILURES` (50) pending failures and is kept as starting before
its Run is recorded, and the Run is linked to it before its workflow
starts. Its request (`improvement_request`, after `self_improvement/request.md`)
names how many Runs failed on its first line, the pull request's title,
then lists them under "Regressions addressed", by Scorer and then the
flawed Runs, each linking its Run in the app (`/app/factory/runs/{id}`)
and the Scorers' judgments (`/app/factory/insights`), each reason on one
line; `publish` puts the request in the pull request's description, so the
section is there; each `@` and `#` in a quoted reason is followed by a
word joiner, so the description mentions no one and links no issue. A Run
refused (its Agents do not fill the Flow's Roles,
`DefinitionError::ImprovementAgents`; no push access; a full Queue) is kept
`refused` with a short reason (`refusal_reason`, never the error's own
words, which Insights shows the whole team), and its failures go back. A
start whose error came after its Run was kept and linked, while the Run
still exists (its workflow may have started), stays open and is settled by
that Run. One kept starting longer than `STARTING_STALE_AFTER` (10
minutes) is refused the same way. So none stays open for good, an open one
is settled `closed` when its Run kept no Step for `NO_STEP_STALE_AFTER`
(2 hours, a Step's whole budget: its workflow never started), or its pull
request could not be read (its owner's GitHub access revoked, say) for
`UNREAD_STALE_AFTER` (a week) after its Run ended. Naming another Definition, or none,
settles the team's open one as closed in the same write
(`set_team_definition`), so its failures do not wait on a Definition the
team no longer names; its Run goes on for its owner. Only
the factory starts one: `improve` is never listed or started by name
(`UnknownFlow`), and a Definition names no Flow `improve`
(`ReservedFlow`).

A person approves its plan and merges it, whatever it changes: the
`improve` Flow keeps its plan and merge Gates for a person (`person_only`),
so no Standing Rule (`rule_gate` reads the Gate's `person_only` from the
Run's own Flow), stand-in or Orchestrator answers them, even for a change
outside the Definition's files. The Run is also on its team's Definition
repository, so the Definition's files are its Control Files and any change
to one is Tampered (`ControlFiles::changes_guarding`). Once its Run ends, the next
`after_run_end` of the team settles it: a Run that ended `success` (its
`merge` merged) or `flawed` addressed its failures (`merged`); otherwise
its pull request is read as its owner: merged addresses them, open keeps
the Run open (no other starts), closed unmerged, or none after its owner
cancelled the Run in person, is a person's rejection (`Settlement::Rejected`, kept
`closed`) that dismisses them for good, and none otherwise sends them back
to pending (`closed`); each pauses the next as above. A cancel is in person only
from the owner in their own signed-in session, from an account that is not
an agent's (`factory_run.cancelled_in_person`, kept by `cancel_run` before
the cancel is sent, before the Run's end; once a person's cancel is kept, a
later one by an agent does not undo it, so an agent's cancel sent long after
still leaves the person's earlier rejection standing); a cancel by
an agent, a bot under a Standing Role, an internal caller or a program only
means the Run did not finish, so its failures are pending again. Dismissed failures
never start another Run; new failures, of the same Scorer too, count.
When the rejection was of the fix and not of the failures,
`POST /self-improvements/{id}/undo-dismissal` (`EvaluationService::undo_dismissal`,
in `domain/evaluation/insights.rs`) sends the failures it still holds
dismissed back to pending, where they count toward the next Run's
thresholds at the team's next Run end, after the pause the rejection
started. Only a current admin of the team who named its Definition or
owns the improvement's Run (who could reject it in Macro) may, as a
self-improvement Run starts only for an admin namer: a namer or Run owner
made a member may not. They undo in their own signed-in session and never
from an agent's account (403 otherwise; 404 for no improvement of their
team, 409 for one that dismissed nothing). It is kept once with who and
when, so a repeat answers the first (`restored` 0) and never takes back
failures a later improvement holds. Nothing kept needs forgetting: the
worker keeps only the thresholds for `THRESHOLDS_KEPT_FOR`, and reads the
pending failures afresh at each Run end. Insights show the last improvement's `id`, `dismissed`,
`dismissalUndoneAt` and whether the caller `canUndoDismissal`. A pull request a person
merges or closes after its Run ended is seen only at the team's next Run's
end. Pending failures are matched to the Definition's repository ignoring
case. `GET /insights/evaluation` shows the team's self-improvement
(`selfImprovement`: thresholds, pending failures by source, where the
namer stands, and the last Run with its state), none for a team with no
Definition.

## Start here

- `src/domain/flow.rs`: the Flow model and the checks a definition must pass.
- `src/domain/interpreter.rs`: `when`, `max_visits`, `on_exhausted`, `on`,
  `next`, facts and resets, with no Temporal in it.
- `src/inbound/temporal/workflow.rs`: the workflow that drives the interpreter.
- `src/temporal.rs`: the task queue and the workflow's and its update's names
  and types.
- `src/domain/gate.rs`: the Gate a Run waits on and the answers it takes.
- `src/domain/questions.rs`: an agent's questions to the Run's owner, the
  prompt the Step that goes on gets, and the brief of answered questions.
- `src/domain/built_in_steps.rs`: the Steps the worker serves and their
  prompts.
- `src/domain/session_agent.rs`: an agent Step in a Macro agent session.
- `src/domain/pull_request_steps.rs`: `publish`, `ci`, `ready_to_merge` and
  `merge`, over the `PullRequests` port, with the merge queue and the
  engine's half of `update`; `src/domain/built_in_steps/update.rs`: the
  `update` Step.
- `src/domain/review_rounds.rs`: review rounds: what a reviewer hands
  back, the diff a finding is in, and opening and closing a round.
- `src/domain/built_in_steps/checks_round.rs` and `src/domain/checks.rs`:
  the checks rounds of `write` and `fix`, and the checks file they read.
- `src/domain/control_files.rs`: Control Files, the default patterns, and
  what makes a Run Tampered.
- `src/domain/definition.rs` and `src/domain/service/definition.rs`: the
  Factory Definition's files, their checks, and reading a team's at its
  head; `definition_template/`: the starter Definition.
- `src/domain/plan_review.rs` and `src/domain/built_in_steps/plan_review.rs`:
  a plan's size block, the mechanical checks of a plan, the `plan_review`
  Step and the `split` Step.
- `src/domain/verification.rs`, `src/domain/acceptance.rs` and
  `src/domain/built_in_steps/accept.rs`: the verify Phase, the plan's
  acceptance checks and the `accept` Step.
- `src/domain/main_hold.rs` and `src/inbound/temporal/main_watch.rs`: the
  Main Hold, its looks at a base branch and the watch that makes them.
- `src/domain/gate_replies.rs` and `src/domain/stand_in.rs`: a person's
  words on a Gate, the stand-in, and what it never answers;
  `gate_replies/measure.rs`: the replay measure that sets their thresholds.
- `src/domain/standing_rules.rs`: which Gates a Standing Rule answers, and
  what it never answers.
- `src/domain/automations.rs` and `automations/service.rs`: Automations,
  their triggers and firings; `outbound/routine_automation_triggers.rs`:
  the routines that fire them.
- `src/domain/gate_owner.rs`: Gates and their owner: the notice a waiting Gate
  sends, a notify-only Gate's lighter one, and the owner's notes a Step
  reads. Project rules are set through the API only: team
  settings show how many Projects have their own.
- `src/domain/step_terminal.rs`: the Step Terminal: what the owner's note,
  pause, resume and stop do, and the prompts they send the agent.
- `src/domain/task_thread.rs`: the task thread: the comment each update of
  a Run posts to its task, and its id.
- `src/domain/board.rs`, `src/domain/service/board.rs` and
  `src/inbound/http/board.rs`: the board (`GET /runs`): a page of the
  caller's Runs, or their team's (`scope=team`), newest first, each with
  where it is, and who may read a team member's Run. A team member's Run is
  listed and read (`GET /runs/{id}`) only on a task the caller can see, by
  someone on its owner's team; one with no task stays its owner's, and only
  the owner acts on a Run. A Run the caller cannot see never shapes a
  page: the board reads on past them, in up to five rounds of at least 50
  Runs, until it has one Run the caller sees past the page; `next` names
  the last Run listed, and only while such a Run follows, so a page of
  hidden Runs alone reads as no Run. Past five rounds of hidden Runs in a
  row (some 250) the page ends there, with no next page, and says it
  stopped early (`stoppedEarly`) only on a board no filter or search
  narrows, so the flag never tells how many hidden Runs match a filter. A team member's
  Run on a task that is not a live task is hidden without asking; the
  others' tasks are asked in one check a round (`TaskDirectory::
  viewable_documents`, entity access's batch receipts), and each answer is
  kept 30 s. Only the owner's read of a Run whose workflow never started
  ends it.
  `POST /runs/by-pull-requests` (`{"urls"}`, at most `PULL_REQUESTS_MAX`,
  100, else 400) finds the Runs that opened some pull requests, by the
  page each Run keeps (`pull_request_url`), in any case: for each, the
  newest Run on the caller's team board they see (theirs, or a team
  member's on a task they can see, as above), with where it is and
  `mergeGateSeq`, the Step whose merge Gate it waits on (its latest Step,
  at an entry that runs `ready_to_merge` or `merge` in the Flow the Run
  walks, read in one `runs_flows` read, with a Flow's Gate no one answered,
  while it goes; a Run whose kept Flow does not read waits at none). A
  pull request only a hidden Run opened is left out, as one no Run opened.
  It reads each pull request on its own: the newest Run of each owner and
  task, at most 10 (`PULL_REQUEST_RUNS_READ`), newest first; a pull request
  whose read reached that with no Run shown is listed in `unknown`, as one
  whose Run may be among those left unread, never as one no Run opened.
  Another pull request's Runs never change its answer. A Run opens its pull
  request from its own branch (`factory/<run_id>`), and only that sets its
  page, so a pull request has one Run and one owner and task: `unknown`
  guards against records written by hand, a state the engine can't create.
  The web app's pull request views (Reviews, a pull request's detail and
  block, its Merge button) read it through `PrMergeGuardProvider` to mark a
  factory Run's pull request, and merge one only through that Gate
  (`POST /runs/{id}/steps/{seq}/answer`); until it answered, or while a pull
  request is `unknown`, they merge nothing. A view rendered outside the
  provider merges nothing either for a person who may have the factory
  (`usePrMergeGuard`'s default), and the provider wraps the app's ownerless
  dialogs too. Macro's GitHub merge route (`POST
  /github_pull_requests/merge` in `services/authentication_service`) refuses
  too, whoever calls it, the web app, an agent or another client: a pull
  request whose page a Run that has not ended keeps
  (`factory_records::domain::merge_guard`, any owner's Run) is refused
  before GitHub is asked, 409 naming the Run and its merge Gate to a caller
  who sees the pull request on GitHub (its branch, `factory/<run_id>`,
  already shows them the id), and 404 `Not Found`, as GitHub answers, to
  anyone else, so the refusal tells no one of a Run they could not find on
  GitHub. A read of the Runs that fails refuses the merge (500): they are in
  the database the route reads the caller's GitHub grant from, so the
  merge would rarely get further, and no pull request's head is known
  there to tell a Run's branch without asking GitHub. A pull request whose
  Runs all ended, or that no Run opened, merges as before. The engine never
  calls that route: the `merge` Step (whether a person or
  `merge_when_green` answered its Gate) merges on GitHub itself, with the
  owner's token (`outbound/github_pull_requests.rs`), so the refusal never
  stands in its way. Limits: GitHub itself still merges a Run's pull
  request; and in the web app a viewer who cannot see the Run (a teammate
  on a hidden task, someone off the team) sees no mark, as the lookup must
  not tell them a Run exists, though the route refuses their merge. A Step that finds the Run's pull request keeps its page
  (`keep_page`); `ci` writes it once per wait, not on every poll. A write
  that fails fails the Step, which is retried until the Step's time runs out
  (`STEP_TOTAL`, 2 hours), and then fails as usual. Until the write lands,
  or if it never does, the pull request is open on GitHub and neither guard
  can see it, so it merges as no Run's.
- `src/domain/self_improvement.rs`: self-improvement: the failures a
  Run's end keeps, when a self-improvement Run starts, its request, and how
  it is settled.
- `src/domain/quality.rs`: CI health, review quality, cost per merged
  pull request and the autonomy rate (`GET /insights/quality`), and how a
  `ci` wait's finished jobs are classed before they are kept.
- `src/domain/pull_request_steps/flaky_ci.rs`: the `rerun_flaky_ci`
  Standing Rule: a red of only known flaky jobs (the Definition's
  `knownFlakes`, a job quarantined in the repository, or the same failure
  on other pull requests) is rerun once through GitHub before `ci-fix`;
  passed retries quarantine a job, and `service/quarantine.rs` files its
  task as the Run ends.
- `src/domain/evaluation.rs`: Scorers, Experiments and Benchmarks, their
  sampling and arm assignment; `evaluation/service.rs` scores a Run that
  ended; `evaluation/insights.rs` the evaluation insights and suites.
- `flows/`: the built-in Flows, each named for its file
  (`src/domain/built_in_flows.rs`). `GET /flows` lists those whose Steps the
  worker serves, with their `description` and whether they work on a
  `repository`: `code`, `plan` and `reply`; for a team that names a Factory
  Definition, its Flows instead. `benchmark` and `improve` run only as a
  suite's trial and a self-improvement Run.

## Scope

- Owns: the Flow format, its interpreter, Gates, the workflow and its
  activities, the Run API's router, the built-in Flows (`flows/`) and Steps,
  the `StepAgent` port with its local stand-in, the `PullRequests` port
  with its GitHub adapter, and the Main Hold: its watch workflow, the
  `MainWatches` port with its Temporal adapter, and the `HoldTasks` port
  with its document storage adapter.
- Not here: the Run, Step and Gate tables → `crates/factory_records`; the process
  that hosts the API and the worker → `services/factory_service`.

## Boundaries

- Depends on: `factory_records`, `ai_usage` (agent sessions' usage, and
  the Gate model's), `agent` (the model router the Gate model calls),
  `agent_session` (the routine sessions port),
  `agent_correlation`, `macro_authorization`, `macro_user_id`,
  `entity_access` (ports), `document_storage_service_client`,
  `authentication_service_client` (accounts and linked GitHub tokens),
  `models_properties` and `system_properties` (a task's Project property),
  `model_notifications`, `notification` and `model-entity` (a waiting
  Gate's notification), `messages` (the discussions on a Run's task),
  `github` (the user-token client), the Temporal Rust SDK.
- Used by: `services/factory_service`.

## Recipes

- Add a built-in Flow: a file in `flows/` with a `description`, and an entry
  in `BUILT_IN_FLOWS` (`src/domain/built_in_flows.rs`).
- Add a built-in Step: `src/domain/built_in_steps.rs` (`BuiltInStep`, `named`
  and `run`); an agent Step also gets its instructions in `built_in_steps/`,
  and each entry that runs it names a Role.
- Add a Role: name it on the entries that need it; every deployment's agent
  definition must then fill it, or the service does not start. A review
  panel's new Role also goes in `REVIEWER_ROLES` (`src/domain/agents.rs`), to
  default to `reviewer`'s Agents, with its focus in `REVIEW_FOCUS`
  (`built_in_steps.rs`).
- Change the default Control Files: `DEFAULT_CONTROL_FILES` in
  `src/domain/control_files.rs`, and its test.
- Let the stand-in answer another kind of Gate: its field on
  `StandInKinds` and its stored name (`stand_in_*`) in `factory_records`'
  `domain/standing_rules.rs` and the migration's checks, `stands_in`, and
  their tests; never a kind whose answer approves Control Files or merges.
- Add a Standing Rule: a `RuleGateKind` and its `StandingRule` in
  `factory_records`' `domain/standing_rules.rs` (and the migration's checks
  of the stored names), the kind's field on `OrchestratorDefinition`,
  `kind_answers` and `rule_answer` in `src/domain/standing_rules.rs`, the
  `kind` on the built-in Flows' Gates it decides, and their tests.
- Add a field to the checks file: `src/domain/checks.rs` (field, check
  and test), then `checks_round.rs`: the prompt, if the agent needs it, and
  the command the engine runs.
- Add a file or field to the Factory Definition: `src/domain/definition.rs`
  (field, check and test), `definition_template/` and its README, then what
  reads it at a Run's start (`service/definition.rs`, `start_run`).
- Add a Flow keyword: `src/domain/flow.rs` (field and check) →
  `src/domain/interpreter.rs` → `src/domain/interpreter/test.rs`.

## Checks

- `cargo test -p factory_engine`
- The workflow on a running Temporal (`src/inbound/temporal/test.rs`, ignored
  in CI): `cargo test -p factory_engine -- --ignored` with the local stack
  up, or `TEMPORAL_ADDRESS` set to another dev server.

## Gotchas
- The pull request Steps hold no session: `stop` and `release` do nothing
  for them, and a cancelled `ci` only drops its wait. A CI wait is bounded by
  one Step attempt (25 of its 30 minutes); a longer one needs the workflow to
  wait on a timer instead.
- Deploy `agent_harness_service` before `factory_service`: an older harness
  drops the routine session's `repo_url`, so a `code` Run's agents would work
  outside its repository. One without the `exec` route leaves rounds that
  have not started their checks to the agent.
- A round's checks share its worktree: a check that changes the tree changes
  what later checks of the round see.
- The agent pushes the Run's branch with its runtime's own git credentials
  (the egress's GitHub App token in a managed sandbox, the operator's git on
  macrod), and the pull request Steps use the owner's linked account: both
  must reach the repository. A macrod that manages no repositories serves
  only the repository its workspace is configured with.

- A Step or Run that ended can still have its agent at work until the stop
  lands, up to the stop's fifteen minutes of retries. Its hand back is refused
  meanwhile: the Step's end is recorded first, and a hand back needs a Step
  that has not ended. If every stop fails, nothing in the factory ends the
  agent's turn: it runs until the agent ends it, in a session no attempt
  opens or prompts again.
- A runtime that opens a requested session more than five minutes after its
  prepare, as macrod may when its event stream lags or replays, leaves a
  session no stop deletes: by then the stop has written it off. The recorded
  prepare and open narrow that window; no bound closes it.
- `RoutineSessionError::is_transient` (in `agent_session`) exists for the
  factory's retries; it is not meant for upstream on its own.
- The workflow must stay deterministic: everything it decides comes from the
  interpreter and the activities' results, never a clock, a random number or I/O.
- Renaming `TASK_QUEUE`, `FLOW_WORKFLOW`, `ANSWER_GATE_UPDATE`,
  `MAIN_WATCH_WORKFLOW` or an activity's `name` strands the Runs and watches
  in flight on the old name.
- A watch whose owner's GitHub link stops working keeps the hold and goes
  on looking; the next merge to the base, or a held `merge`, makes its
  owner the one the watch reads as.
- Steps running when the Step Terminal deploys: an attempt that started on
  an older worker does not read the owner's steering. A note reaches the
  session, but the Step may end with the turn before it; a pause stops the
  turn, which fails the Step as any stopped turn did. Its next attempt, on
  a new worker, follows the steering. Deploy every worker before the web
  app offers the Terminal, and `agent_harness_service` before
  `factory_service` so it reads `out_of_sight` (an older harness follows
  the session for its owner, as before).
- Changing what the workflow or the interpreter decides breaks the replay of
  Runs already running: gate the change with Temporal patching
  (`ctx.patched`) or worker versioning.
- A Run started before questions carries a Flow without `questions`: its
  Steps are not offered `question` (`StepRequest::may_ask`), so it never
  takes the question's path and replays as before. A worker older than
  `questions` cannot read a Flow that has it, nor a `StepRequest` with
  `may_ask` or `resumes`, so deploy every worker before the API starts such
  Runs.
- A Run carries its Flow in its workflow's input, so a Run started on the
  single-reviewer `code` Flow keeps walking it, with `review.md`, and replays
  the same way: only an entry with `reviewers` takes the review round's path
  in the workflow. A worker older than `reviewers` cannot read a Flow that
  has it, so deploy every worker before the API starts such Runs.
- `StepRequest` refuses fields it does not know, so a worker older than a
  field fails the activity rather than run a Step it would misread; a
  reviewer's Step a worker without review panels ran anyway records no
  answer, and its round fails.
- A follow-up task is made with an id document storage takes only from an
  internal caller, and only a UUIDv7 (`CreateTaskRequest.document_id`), so
  deploy document storage before a factory that sends it.
- A Gate opens only for a Step that ends `done` (or, on a review panel, a
  round that ends with the outcome its `after` names); any other outcome
  routes as usual, so a failed plan is never put to a person.
- A worker older than `GateDefinition::after` cannot read a Flow that has
  it, and one older than `factory_rule_gate` fails that activity: deploy
  every worker before the API starts Runs on the new `code` Flow.
- Removing the `factory-infrastructure-outcome` patch breaks the replay of
  Runs that ended `infrastructure` (`plan_infrastructure_after_marker`); a
  Run that ended `failed` on such a Step before it
  (`plan_infrastructure_before_marker`) replays without it.
- Deploy the workers and the API together with the outcome kinds: an older
  API, or an older worker reading Runs, does not read a Run that ended
  `infrastructure` or `flawed`. Rolling back past them needs those Runs set
  back first (`infrastructure` to `failed`, `flawed` to `success` with its
  flaw columns cleared), or the older code fails to read them.
- Known limits of `infrastructure`: an agent whose provider's usage limit
  ends its turn mid-way is told from any other failed turn only when its
  runtime classifies it (Codex's adapter, a cloud agent's notice): Claude
  Code's adapter does not yet, so its usage limit fails the Step as any
  failed turn, and a round of checks that ends red because
  the fixer's quota ran out (`Red::QuotaRanOut`) is the Step's outcome, not
  a failure of its work: both still end the Run `failed` when the Flow
  routes them there. An edge the other way: when an earlier attempt's
  worker went quiet and a later, slow attempt is still running as the
  Step's time (`STEP_TOTAL`) runs out, the timeout may carry the earlier
  heartbeat timeout as its cause, and the Run can read as infrastructure.
  A network or GitHub outage surfaces as a retried
  internal error (`StepError::Internal`), whose last failure reads as the
  work's once the Step's time runs out.
- Removing the `factory-notify-only-gates` patch breaks the replay of Runs
  that passed a notify-only Gate (`code_notify_only_plan_with_thread`). A
  worker older than `notify_only` cannot read a Flow that has it, so deploy
  every worker before the API starts Runs on one.
- Removing the `factory-no-result-gates` patch breaks the replay of Runs
  that asked their owner about a Step whose agent stopped without a
  result. A worker older than `StoppedWithoutResult` fails such a Step for
  good, and an older harness answers `reply` as not found, so the Gate
  then says the last message could not be read: deploy the harness first.
- Removing the `factory-scoring` patch breaks the replay of Runs that
  were scored once they ended (`plan_scored_judge_failed`); Runs that
  ended before it replay without it.
- A worker older than `Flow::unattended` cannot read the `benchmark` Flow.
  The API and the worker are one binary: add Benchmarks to
  `FACTORY_EVALUATION` only after every pod runs this version.
- Removing the `factory-gate-owner` patch breaks the replay of Runs that
  told their owner of a Gate; the histories recorded before it
  (`code_rules_fixed_then_merged`, `code_waiting_at_plan_gate_with_rules`,
  and the Gate replies' histories) replay without it.
- Removing the `factory-standing-rules` or `factory-gate-replies` patch,
  or asking anything new between recording a Gate and waiting on it
  without a patch of its own, breaks the replay of Runs in flight: `inbound/temporal/test/replay.rs`
  replays recorded histories, and gets new ones recorded beside them when
  the workflow changes.
- A Run under a Factory Definition keeps its Agents, so a Definition that
  renames an Agent changes only later Runs. Insights orders a Flow's
  entries by the built-in Flow of that name: a Definition's Flow of another
  name lists its entries in no set order (display only).
- Deploy every worker before the API starts Runs under a Definition: an
  older worker cannot read a Run whose `standing_rules_source` is
  `definition` (`RulesSource`), and fails the Step's activity, and it reads
  Gate kinds from the built-in Flows rather than the Run's kept Flow.
- Renaming or removing an Agent while its Steps run fails their retries
  (`UnknownAgent`: its model and instructions are gone), though stops and
  releases still reach their sessions through the recorded bot.
- A subscription that runs out mid-turn (a turn the runtime ends with the
  provider's usage limit) is out of quota but not a refusal to start: once
  the turn called a tool, no other Agent takes the Step. A worker older
  than the routine status `usage_limit` does not read it from a newer
  `agent_harness_service`: the read fails as a transient error, so
  Temporal retries the attempt until its retries or its time run out, and
  the Step fails as a failed turn failed it before, only later. Either may
  deploy first.
- Deploy `agent_harness_service` signing `scheduler_run` before
  `factory_service` reads it, and run the
  `factory_automation_routine_revision` migration first: until both, an
  Automation's routine firings carry no proof (or the Automation no
  revision) and act as the Macro routine bot, so under a Definition with
  Standing Roles each is skipped as `refused`, as before this change.
  Automations made earlier, and any whose routine an SDK code upgrade
  rewrote, need switching off and on once. A routine switched to write as
  its owner (signing no bot) used to fire its Automation as its creator; it
  is now checked as the Macro routine bot, so under Standing Roles it needs
  a Role that allows `start` for that bot.
- Deploy `agent_harness_service` with an SDK that has
  `macro.factory.fireAutomation` before a factory that makes Automations:
  saving the routine type-checks its code step against the harness's SDK.
  Event routines need the creator's `enable-event-routines` flag in
  PostHog, and the harness starts no event intake without PostHog: then a
  repository event's or a Task event's routine is refused, or never runs.
- Deploy the `factory_merge_queue` migration, then every worker, and only
  then the API: a Run started on the new `code` Flow against an older
  worker fails at the unknown `update` Step for good. A Run whose review
  passed on an older worker keeps no reviewed head, so its first update
  goes back through review. Runs started before
  keep walking their Flow, take no queue, and never end `behind`. Nothing
  in the workflow changed, so no patch marker is needed and the recorded
  histories replay as before.
- The merge queue is per repository and base branch across owners: a Run
  waits on another person's Run ahead of it, which it cannot see: `queued`
  says only that earlier Runs into the base have not merged yet, never
  which, nor how many. That much shows by design: a shared queue tells
  each Run's readers that some Run they may not see is ahead of it on
  that base. A Run
  stuck at its merge holds the Runs behind it until it ends, merges, or
  has been away from its merge longer than `QUEUE_PLACE_KEPT`.
- A worker older than `plan_review` and `split` cannot serve a Run of the
  `code` Flow that names them: deploy every worker before the API starts
  such Runs. A Run started before them keeps its Flow, with the plan Gate
  on `plan`, and never reaches them. A Definition whose `code` Flow still
  gates `plan` keeps working as before.
- A worker older than `accept` cannot serve a Run of the `code` Flow that
  names it: deploy every worker before the API starts such Runs. A Run
  started before it never reaches it.
- A worker keeps running `deploy` and `verify`, ending each at once,
  while a Run whose Flow names them may still run: removing them sooner
  leaves such a Run's next Step failing for good, and the Run `manual`.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
- Self-improvement runs only in the scoring activity, behind the
  `factory-scoring` marker: a Run that ended before the marker is never
  scored, so its failures are never kept. A Definition named before
  `named_by_user_id` starts no self-improvement Run until an admin names it
  again. Thresholds are checked only when a team's Run ends: a team whose
  Runs stop starts none, however old its failures.
- A worker older than a Scorer's `reads` cannot read an evaluation that
  sets it (`deny_unknown_fields`), nor a Run that kept one: set `reads` in
  `FACTORY_EVALUATION` or a Definition only after every pod runs this
  version.
- Deploy the `factory_review_dispute` migrations, then every worker and
  the API together: an older binary reads a finding kept `dismissed` as
  `UnknownReviewWord` and fails the read of
  the Run's review rounds (the Run API, the Steps of a review panel, the
  Gates' Standing Rules). Rolling back past them needs the findings set
  back first (`kind` `dismissed` to `blocker`).
- A worker older than `selfImprovement` in `scorers.json` cannot read a
  Definition that sets it (`deny_unknown_fields`); one that leaves the
  defaults keeps the JSON it kept before.
- Run the `factory_ci_quarantine` migrations (the table one, then the two
  concurrent index builds) before deploying the factory. A worker older
  than `rerun_flaky_ci` cannot read a Run whose kept rules store it, nor a
  Definition whose `rules.json` sets `rerunFlakyCi` or `knownFlakes`
  (`deny_unknown_fields`): deploy every worker and the API together after
  the migrations, as for `selfImprovement`, and only then turn the rule on.
  The rule is off by default. A `ci` Step reruns only GitHub Actions jobs,
  with the owner's token: one that may not write Actions gets the rerun
  declined, and the red goes to `ci-fix` as before. A red the rule may
  rerun is held until the head's other checks finish, or at most three
  minutes after its jobs' workflow runs completed, so its `ci-fix` starts
  later.
