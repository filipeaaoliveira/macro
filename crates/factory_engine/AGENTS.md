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
the entry's outcome. An answer the recorded Gate does not offer is refused
before it is sent (400); the update's validator (`domain/gate.rs`) refuses an
answer to a Gate the Run does not wait on and a second answer (409). Only the
Run's owner answers its Gates, in their own signed-in session: an internal
caller, a user API key or Macro API token, and an agent's program acting for
them are refused (403).

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

`GET /insights?from=&to=&timeZone=&flow=` (`inbound/insights.rs`,
`domain/insights.rs`) sums up how the caller's own Runs went over a period of
at most 366 days in a time zone: Runs by day, outcome and Flow, Run and Step
durations, Gate waits and answers, the entries that failed or were revisited
with their newest Runs, and the tokens each agent Step's session used, read
from the usage log by session id (`ai_usage`'s `SessionUsageReader`). It
reads the newest `INSIGHTS_RUN_CAP` Runs of the period. A bad period, time
zone or Flow name is 400.

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
records the last one passed over too and fails for good. An attempt
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
back is the Step's outcome and output; none fails the Step. `LocalAgent` does no
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
stays recorded and is linked to it. The task is made as the Run's owner, not
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

## Start here

- `src/domain/flow.rs`: the Flow model and the checks a definition must pass.
- `src/domain/interpreter.rs`: `when`, `max_visits`, `on_exhausted`, `on`,
  `next`, facts and resets, with no Temporal in it.
- `src/inbound/temporal/workflow.rs`: the workflow that drives the interpreter.
- `src/temporal.rs`: the task queue and the workflow's and its update's names
  and types.
- `src/domain/gate.rs`: the Gate a Run waits on and the answers it takes.
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
- `src/domain/main_hold.rs` and `src/inbound/temporal/main_watch.rs`: the
  Main Hold, its looks at a base branch and the watch that makes them.
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

- Depends on: `factory_records`, `ai_usage` (agent sessions' usage),
  `agent_session` (the routine sessions port),
  `agent_correlation`, `macro_authorization`, `macro_user_id`,
  `entity_access` (ports), `document_storage_service_client`,
  `authentication_service_client` (accounts and linked GitHub tokens),
  `models_properties` and `system_properties` (a task's Project property),
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
- Changing what the workflow or the interpreter decides breaks the replay of
  Runs already running: gate the change with Temporal patching
  (`ctx.patched`) or worker versioning.
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
- A Gate opens only for a Step that ends `done`; any other outcome routes as
  usual, so a failed plan is never put to a person.
- Renaming or removing an Agent while its Steps run fails their retries
  (`UnknownAgent`: its model and instructions are gone), though stops and
  releases still reach their sessions through the recorded bot.
- A subscription that runs out mid-turn (a turn that fails with the
  provider's usage limit) is not a refusal to start: it fails the Step as any
  failed turn does, and no other Agent takes it.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
