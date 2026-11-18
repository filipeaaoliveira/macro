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
(every entry's, since no record says which told; one never sent changes
nothing). It is sent whatever the owner section says, since the Gate
exists only to tell the owner (`gate_owner::gate_told`); the worker also
posts the notice to the task thread when that section is on. No built-in
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
ran out takes no more steering (409). A verifier (`accept`, `verify`)
stopping without a result ends its Step `no_result` as any agent does,
before any acceptance check runs, so its Gate is the only one the Step
opens: never `accept`'s Blockers Gate too. The Step that goes on from a
`retry` is the same visit, so a Blockers Gate it then opens counts the
fixes of the visits before it, not the retried Step.

People call the Run API with their own login (`UserOrInternal`), and internal
services for an acting user (the internal key's identity headers). A Run starts
only of a built-in Flow, by name: no caller sends a Flow definition. The acting
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

`GET /insights?from=&to=&timeZone=&flow=` (`inbound/insights.rs`,
`domain/insights.rs`) sums up how the caller's own Runs went over a period of
at most 366 days in a time zone: Runs by day, outcome and Flow, Run and Step
durations, Gate waits and answers, the entries that failed or were revisited
with their newest Runs, and the tokens each agent Step's session used, read
from the usage log by session id (`ai_usage`'s `SessionUsageReader`). It
reads the newest `INSIGHTS_RUN_CAP` Runs of the period. A bad period, time
zone or Flow name is 400.

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
whose workflow never started ends it `failed` once it is five minutes old. A
Temporal outage answers 503 (or 502 for an unusable answer), never 500.

The built-in Steps (`src/domain/built_in_steps.rs`) are `reply`, which
acknowledges the request; `plan`, `write`, `review`, `fix` and `ci_fix`, which
hand an agent a prompt built from the Step's instructions
(`built_in_steps/*.md`), the Run's request, and what the Run's earlier Steps
produced; and `publish`, `ci`, `ready_to_merge` and `merge`, which take the
Run's branch to a merged pull request (`src/domain/pull_request_steps.rs`). The
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
worker's own, and a restart forgets it. An attempt
prepares, opens and prompts only the session the Step works in now, so an
overlapping attempt never prompts a session handed on: it deletes the one
it opened and goes on in the Step's current session. Each session keeps the
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
its configured workspace. The prompt names the Run's branch,
`factory/{run id}`, and its base: the agent checks the branch out, and
`write`, `fix` and `ci_fix` commit and push it from their runtime; `review`
changes nothing. The pull request Steps act on GitHub as the Run's owner, with
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
without asking again. A quota that runs out while the agent works on a
turn is the session's failure, and fails the Step as any other. The agent's work and all
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
is not carried to the next round. In `code`, the review panel
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
`changed` without asking; then the Run ends for a person.

Control Files (`src/domain/control_files.rs`) are the files that decide
whether a repository's checks pass: tests and what they compare against
(snapshots, fixtures), test and lint configuration, CI workflows, package and
build scripts, agent instructions and their tools' settings (`AGENTS.md`,
`CLAUDE.md`, `.claude/`, `.agents/`, `.cursor/`) and `.macro/checks.json`.
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

The `code` Flow checks that a change does what its request asks, before
and after its merge: its verify Phase. It is on for every repository by
default, including one with no verify file. The repository declares it in
`.macro/verify.json` (`src/domain/verification.rs`): `verify` (`true` when
left out) turns the Phase on or off, and an optional `deploy` names the
signal a merged Run waits for: `{"environment": "production"}`, a GitHub
deployment to that environment. `accept` and `plan` read the file on the
base branch, which the Run has not merged into; `deploy` and `verify` read
it at the merge commit's first parent, the base as it was before the Run's
change, so a Run never turns its own verification off. The file is also a
default Control File (a team that keeps its own list must add it).

- On a repository, `plan` asks the planner to end its plan with a fenced
  `json` block of acceptance checks (`built_in_steps/plan_acceptance.md`,
  `src/domain/acceptance.rs`): one per Done-when clause of the request (at
  most 16), each the `clause` and either `run`, a command from the
  repository's root (at most 1 KiB) that exits 0 only when the clause
  holds (`timeout` as a check's, and `deployed` for one that needs the
  deployed change), or `none`, why no command checks it. The planner is
  told whether the repository declares a deploy, so it marks checks
  `deployed` only where one runs. The Step keeps the plan with the checks
  written out for its Gate, then the block as the factory read it
  (`plan_output`). Later Steps read the latest `plan` Step's
  (`AcceptanceChecks::of_run`).
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
  the factory runs every acceptance check not marked `deployed` there, as a
  checks round of one round (`built_in_steps/verify.rs` over
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
- `deploy`, after `merge` (or a merge found by `ready_to_merge`), is a
  pull request Step (`pull_request_steps/deploy.rs`): `not_needed` when the
  repository declares no deploy, else every 20 s for `DEPLOY_WAIT` (25
  minutes, within one Step attempt) it reads the newest deployment of the
  pull request's merge commit (`PullRequest::merge_commit`) to the
  environment: `deployed` once it succeeded (or succeeded and was replaced
  since), or once one of the environment's five newest deployments that
  succeeded is of a later commit containing the merge commit (GitHub's
  compare: `ahead` or `identical`), which deployed it too; `deploy_failed`
  when the merge commit's own deployment failed or errored and no later one
  deployed it; `timeout` otherwise. The address is the newest status's
  that names one (512 bytes at most). `off` ends the Run `success` at its
  merge, and a file that does not read `unreadable`, for a person.
- `verify`, after `deploy`, is the phase after the merge: the verifier
  checks the merge commit, on the deployment when there is one, and the
  factory runs every acceptance check there, told `MACRO_DEPLOY_URL` too.
  A check marked `deployed` with no address to reach (no deploy declared,
  or a deployment that names none) is not run: the verifier checks its
  clause by hand, and the report says why. Nothing goes back once merged:
  `pass` (or `off`) ends the Run `success`, and `fail` ends it `failed`, as
  does `deploy_failed`. A `deploy` or `verify` Step that fails for good (a
  crash, not a failed check) ends the Run `manual`, as does an `accept`
  Step's.

The Steps are new entries of `flows/code.json` only: a Run carries its Flow
in its workflow's input, so one started before them walks the Flow it
started with and ends at its merge as before (`interpreter/test.rs`,
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
no rule approves a Tampered Run's plan or merge (a Rule Answer approves no
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

A team's Orchestrator Definition also has an owner section
(`owner_gates`, `ownerGates` in `PUT /settings/standing-rules` and its
Project route; off by default and when a definition leaves it out, so a
team's or a Project's definition saved before it keeps it off until an
admin turns it on there), kept on the Run with its rules
(`src/domain/gate_owner.rs`). With it on:

- a Gate left for a person (a Flow's Gate no rule answered, or an agent's
  question) tells the Run's owner: once the workflow takes its answers it
  calls `factory_notify_gate` (behind the `factory-gate-owner` patch
  marker; tried for at most five minutes, and a failure leaves the Gate
  waiting), and the worker sends a `factory_gate_waiting` notification
  about the Run's task through the notification ingress
  (`outbound/ingress_gate_notifier.rs`), with an id derived from the Run
  and the Step, so a retry is one notification. A Run with no task has
  nothing to notify about. The marker and the activity run for every Gate
  left to a person, whatever the section says: with it off the worker
  sends nothing. Once the Gate is answered (`factory_answer_gate`, or
  `factory_read_gate_reply` for words, by whoever answers), the worker marks its notification done through the
  notifications' reader, as the owner marking it done would, which also
  clears its push; one never sent changes nothing. `GET /runs/waiting` lists the caller's Runs
  waiting at an unanswered Gate, whatever the section says;
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
`AiFeature::FactoryGate`, billed to no one) picks one of the Gate's answers
or `unclear`, with how sure it is. The question, its answers and the words
reach the model as one JSON object, so the words open no section of their
own. At or above the reply threshold (`DEFAULT_REPLY_THRESHOLD`, 85
percent, or `FACTORY_GATE_REPLY_THRESHOLD`) the Gate is answered with it as
the person's answer, the words kept as its note and the confidence beside
it; below it, `unclear`, an answer the Gate does not offer, a call that
fails or takes longer than `MODEL_TIMEOUT` (30 s), the factory asks back on
the Gate (`asked_back`), which waits on and takes an answer or new words.
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
back and the Gate waits for the owner. Below the
threshold, unsure, or failing, it keeps nothing, and the Gate waits for the
person. A retry returns the answer kept first, with what is left of its
window. Run insights count neither its answers nor Rule Answers.

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
owner answers in its window, tells no one; one whose stand-in answer is
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
  body) and fires the Automation of that routine only for its creator. A
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
  an edit there is not read back.
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
  `merge`, over the `PullRequests` port.
- `src/domain/review_rounds.rs`: review rounds: what a reviewer hands
  back, the diff a finding is in, and opening and closing a round.
- `src/domain/built_in_steps/checks_round.rs` and `src/domain/checks.rs`:
  the checks rounds of `write` and `fix`, and the checks file they read.
- `src/domain/control_files.rs`: Control Files, the default patterns, and
  what makes a Run Tampered.
- `src/domain/verification.rs`, `src/domain/acceptance.rs`,
  `src/domain/built_in_steps/verify.rs` and
  `src/domain/pull_request_steps/deploy.rs`: the verify Phase, the plan's
  acceptance checks, the `accept` and `verify` Steps, and the deploy wait.
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
- `src/domain/gate_owner.rs`: the owner section: the notice a waiting Gate
  sends, a notify-only Gate's lighter one, and the owner's notes a Step
  reads. Project rules are set through the API only: team
  settings show how many Projects have their own.
- `src/domain/step_terminal.rs`: the Step Terminal: what the owner's note,
  pause, resume and stop do, and the prompts they send the agent.
- `src/domain/task_thread.rs`: the task thread: the comment each update of
  a Run posts to its task, and its id.
- `flows/`: the built-in Flows, each named for its file
  (`src/domain/built_in_flows.rs`). `GET /flows` lists those whose Steps the
  worker serves, with their `description` and whether they work on a
  `repository`: `code`, `plan` and `reply`.

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
- Add a deploy signal: a field of `DeploySignal` in
  `src/domain/verification.rs` (and its check and test), then
  `pull_request_steps/deploy.rs`, which waits for it, over a
  `PullRequests` read.
- Add a field to the checks file: `src/domain/checks.rs` (field, check
  and test), then `checks_round.rs`: the prompt, if the agent needs it, and
  the command the engine runs.
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
- Removing the `factory-notify-only-gates` patch breaks the replay of Runs
  that passed a notify-only Gate (`code_notify_only_plan_with_thread`). A
  worker older than `notify_only` cannot read a Flow that has it, so deploy
  every worker before the API starts Runs on one.
- Removing the `factory-no-result-gates` patch breaks the replay of Runs
  that asked their owner about a Step whose agent stopped without a
  result. A worker older than `StoppedWithoutResult` fails such a Step for
  good, and an older harness answers `reply` as not found, so the Gate
  then says the last message could not be read: deploy the harness first.
- Removing the `factory-gate-owner` patch breaks the replay of Runs that
  told their owner of a Gate; the histories recorded before it
  (`code_rules_fixed_then_merged`, `code_waiting_at_plan_gate_with_rules`,
  and the Gate replies' histories) replay without it.
- Removing the `factory-standing-rules` or `factory-gate-replies` patch,
  or asking anything new between recording a Gate and waiting on it
  without a patch of its own, breaks the replay of Runs in flight: `inbound/temporal/test/replay.rs`
  replays recorded histories, and gets new ones recorded beside them when
  the workflow changes.
- Renaming or removing an Agent while its Steps run fails their retries
  (`UnknownAgent`: its model and instructions are gone), though stops and
  releases still reach their sessions through the recorded bot.
- A subscription that runs out mid-turn (a turn that fails with the
  provider's usage limit) is not a refusal to start: it fails the Step as any
  failed turn does, and no other Agent takes it.
- Deploy `agent_harness_service` with an SDK that has
  `macro.factory.fireAutomation` before a factory that makes Automations:
  saving the routine type-checks its code step against the harness's SDK.
  Event routines need the creator's `enable-event-routines` flag in
  PostHog, and the harness starts no event intake without PostHog: then a
  repository event's or a Task event's routine is refused, or never runs.
- A worker older than `accept`, `deploy` and `verify` cannot serve a Run
  of the `code` Flow that names them: deploy every worker before the API
  starts such Runs. A Run started before them never reaches them.
- The `deploy` wait is bounded by one Step attempt (25 of its 30
  minutes); a deploy that takes longer ends `timeout` for a person, and
  needs the workflow to wait on a timer instead.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
