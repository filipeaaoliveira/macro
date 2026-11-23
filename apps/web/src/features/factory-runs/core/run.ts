/** How a Run ended, in the factory's words. */
export type RunOutcome =
  | 'success'
  | 'manual'
  | 'blocked'
  | 'failed'
  | 'cancelled'
  | 'infrastructure'
  | 'flawed';

/**
 * Why a Run that succeeded was found wrong afterwards: who marked it (its
 * owner, or a Scorer by its name), why, and when.
 */
export type RunFlaw = {
  by: { kind: 'person'; userId: string } | { kind: 'scorer'; name: string };
  reason: string;
  markedAt: string;
};

/** A Standing Rule: an answer the factory gives a kind of Gate on its own. */
export type StandingRule =
  | 'approve_plan'
  | 'fix_blockers'
  | 'merge_when_green'
  | 'rerun_flaky_ci';

/**
 * The answer to a Gate: a person's, a Standing Rule's (a Rule Answer), or
 * the stand-in's for the Run's owner.
 */
export type GateAnswer = {
  answer: string;
  note?: string;
  /** The Standing Rule that answered, for a Rule Answer. */
  rule?: StandingRule;
  /** The Run's Tampered files the answer approved. */
  approvedControlFiles?: string[];
  /**
   * For an answer the factory read from the person's words (its note), how
   * sure it was, in percent.
   */
  replyConfidence?: number;
  /** Whether the stand-in gave it, and the owner let it stand. */
  byStandIn?: boolean;
  /** Whether the Orchestrator, the owner's agent in chat, gave it. */
  byOrchestrator?: boolean;
  /**
   * The Agent that gave it and the Standing Role that let it, under a
   * Factory Definition with Standing Roles.
   */
  standing?: { agent: string; standingRole: string };
  answeredAt: string;
};

/**
 * The stand-in's answer to a Gate for the Run's owner: why, how sure it was,
 * and until when the owner may override it.
 */
export type StandInAnswer = {
  answer: string;
  reason: string;
  /** In percent. */
  confidence: number;
  until: string;
};

/**
 * How the Gate model read a Gate when what it said decided nothing: the
 * answer it picked (none when it couldn't tell), how sure it was, in
 * percent, and why. Without a confidence the model didn't answer.
 */
export type GateModelReading = {
  pick?: string;
  confidence?: number;
  reason: string;
};

/**
 * Who asks a Gate's question: the Flow, once its Step is done, offering
 * answers to pick from, or the Step's agent, which the Run's owner answers
 * in their own words (the note of its one answer, `answer`).
 */
export type GateKind = 'flow' | 'question';

/** A question a Step's end put to a person. */
export type Gate = {
  kind: GateKind;
  question: string;
  /** The answers it offers, in the Flow's order. */
  answers: string[];
  openedAt: string;
  answer?: GateAnswer;
  /**
   * The person's latest words the factory couldn't read as an answer, how
   * the Gate model read them, and whether they approved a Tampered Run,
   * which only the Approve button does.
   */
  askedBack?: {
    reply: string;
    at: string;
    reading?: GateModelReading;
    refusedTampered: boolean;
  };
  /** The stand-in's answer, while the owner may override it and after. */
  standIn?: StandInAnswer;
  /** How the Gate model read the Gate when the stand-in left it to the owner. */
  standInDeclined?: GateModelReading;
};

/**
 * One round of the repository's checks a Step ran on its agent's work:
 * which checks, who ran them, and how they went (`passed` or `failed`, with
 * a report).
 */
export type CheckRound = {
  /** Which round, from 1. */
  round: number;
  /** The checks' names, in the repository's order. */
  checks: string[];
  startedAt: string;
  /**
   * Whether the factory ran the checks itself, on the pushed commit, and
   * read their exit codes; otherwise the agent ran them and reported how
   * they went.
   */
  verified: boolean;
  /** The exit code of each check a verified round ran so far. */
  results: CheckResult[];
  /** The commit a verified round's checks ran on: the pushed branch's head. */
  checkedCommit?: string;
  end?: {
    outcome: 'passed' | 'failed';
    /**
     * What each check did, as markdown: the agent's report, or the checks
     * that failed, with the end of their output.
     */
    report: string;
    endedAt: string;
  };
};

/** How one check of a verified round ran. */
export type CheckResult = {
  name: string;
  exitCode: number;
};

/** What the Run's owner did to a running Step from its Step Terminal. */
export type SteerAction = 'note' | 'pause' | 'resume' | 'stop';

/** One thing the Run's owner did to a running Step. */
export type Steer = {
  action: SteerAction;
  /** Their note to the agent, for a note or a resume that has one. */
  note?: string;
  /** The agent's session refused the note or the resume: the agent never read it. */
  undelivered: boolean;
  at: string;
};

/**
 * A running agent Step as its owner watches and steers it: the agent
 * session to watch, whether it is paused or asked to stop, and what the
 * owner did, oldest first.
 */
export type StepTerminal = {
  /** The session the Step works in, while it runs in one. */
  sessionId?: string;
  pausedAt?: string;
  stopAskedAt?: string;
  /**
   * Its agent works on no turn the Step waits on, as while the factory runs
   * the repository's checks: it takes no steering until its next turn.
   */
  betweenTurns?: boolean;
  /** When a paused Step ends, failed, unless it is resumed first. */
  endsBy?: string;
  /** When it ended because it stayed paused past its time. */
  pauseRanOutAt?: string;
  steers: Steer[];
};

/** One visit of one of a Flow's entries. */
export type RunStep = {
  /** The Step's place among the Run's Steps, from 1: what keys it. */
  seq: number;
  entry: string;
  /** Which visit of the entry it is, from 1. */
  visit: number;
  startedAt: string;
  end?: {
    /** The Flow's word for how it ended (`done`, `blockers`, …). */
    outcome: string;
    /** What the Step produced, as markdown. */
    output: string;
    endedAt: string;
  };
  gate?: Gate;
  /** The Role an agent filled for the Step, as the Flow names it. */
  role?: string;
  /** The Agent that took the Step, as the agent definition names it. */
  agent?: string;
  /** The Agents before it that couldn’t start the Step, in order. */
  passedOver?: PassedOver[];
  /** The rounds of checks it ran, in order; none for most Steps. */
  checks?: CheckRound[];
  /** Its Step Terminal; none for a Step read before Steps had one. */
  terminal?: StepTerminal;
};

/**
 * What a reviewer found: where, what is wrong and why. Once its round ends
 * it is a `blocker`, in the diff the round reviewed, which the fix gets, a
 * `follow_up`, outside it, which became a task of its own, or `dismissed`,
 * a blocker its reviewer dropped earlier and raised again, which blocks
 * nothing.
 */
export type ReviewFinding = {
  /** The reviewer's Step. */
  seq: number;
  place: number;
  /** The reviewer's Role, as the Flow names it. */
  role?: string;
  path: string;
  lines?: { start: number; end: number };
  problem: string;
  why: string;
  /** A blocker of the round before found again, or an answer that never
   * read: it blocks wherever it is. */
  blocksAnywhere: boolean;
  /** Unset while its round reviews. */
  kind?: 'blocker' | 'follow_up' | 'dismissed';
  /** The task a follow-up became, when it was made. */
  followUpTaskId?: string;
  /** What the fix said of a blocker it was given. */
  fixStatus?: 'fixed' | 'disputed';
  /** Why the fix disputes it, as the fixer wrote it. */
  dispute?: string;
  /** How the next round ruled on the dispute, once a reviewer answered. */
  ruling?: 'upheld' | 'dropped';
};

/**
 * What became of a Run's blockers: fixed as the fixer says, or disputed and
 * then upheld or dropped, and its findings dismissed as repeats of a
 * dropped blocker. A dispute no round ruled on yet counts as upheld.
 */
export type BlockerResolutions = {
  fixed: number;
  disputedUpheld: number;
  disputedDropped: number;
  dismissed: number;
};

/**
 * One round of a review panel: its reviewers review at once, each in a
 * Step of its own from `seq`, the Run's whole change in the first round and
 * the fix diff, what the fix changed since, in a later one.
 */
export type ReviewRound = {
  /** Which round of the Run, from 1, across every round it recorded. */
  round: number;
  /**
   * Which round of its entry since the entry was last reset, from 1: a
   * round after a reset (as when the change is sent back) is 1 again, and
   * reviews the whole change afresh.
   */
  visit: number;
  /** The place of its first Step. */
  seq: number;
  reviewers: number;
  /** Unset for a round that could not open. */
  headSha?: string;
  /** Where the diff under review starts: a branch, or a commit. */
  diffBase?: string;
  fixDiff: boolean;
  /** Why the round could not open, when it could not. */
  failure?: string;
  startedAt: string;
  end?: { outcome: 'pass' | 'changes' | 'failed'; endedAt: string };
  findings: ReviewFinding[];
};

/** How the factory's run of a reported break went. */
export type BreakStatus =
  | 'verified'
  | 'not_reproduced'
  | 'hung'
  | 'not_run'
  | 'not_applied'
  | 'unreadable'
  | 'dropped'
  | 'unchecked'
  | 'interrupted';

/**
 * A break a write or fix Step's agent reported for a test it added: an
 * edit that should make the test fail, and how the factory's run of it
 * went on the commit whose checks passed.
 */
export type BreakRecord = {
  /** The Step whose agent reported it. */
  seq: number;
  /** The test; the whole line for one that does not read as a break. */
  test: string;
  /** The file the break edits, from the repository's root. */
  file: string;
  /** The text the break replaces. */
  oldText: string;
  /** What it puts in its place. */
  newText: string;
  status: BreakStatus;
  /** The commit it ran on; none when it did not run. */
  checkedCommit?: string;
  /** The end of what the test printed, or why it did not run. */
  output: string;
};

/**
 * The statuses that leave a new test unproven, so the tests reviewer
 * blocks on them: the test passed with the break, hung, failed without
 * it, or the break could not be applied or read.
 */
const BLOCKING_BREAKS: ReadonlySet<BreakStatus> = new Set<BreakStatus>([
  'not_reproduced',
  'hung',
  'not_run',
  'not_applied',
  'unreadable',
]);

/** Whether a break's status leaves its test unproven. */
export function isBlockingBreak(status: BreakStatus): boolean {
  return BLOCKING_BREAKS.has(status);
}

/** The breaks the Step at `seq` reported, in the order they were recorded. */
export function breakRecordsOf(
  records: BreakRecord[],
  seq: number
): BreakRecord[] {
  return records.filter((record) => record.seq === seq);
}

/** An Agent that couldn’t start a Step, so the next one took it. */
export type PassedOver = {
  agent: string;
  /** Why, as a code such as `ai_allowance_exhausted`. */
  reason: string;
};

/**
 * Why an Agent was passed over when it started and its runtime's own usage
 * limit ended its first turn before it did any work.
 */
export const USAGE_LIMIT_REASON = 'usage_limit';

/** The GitHub repository a Run works on. */
export type RunRepository = {
  /** Its `https://github.com/owner/name` URL. */
  url: string;
  /** The branch the Run's work starts from and proposes its change to. */
  baseBranch: string;
};

/**
 * The Control Files a Run's branch changed: files that decide whether its
 * checks pass. `tampered` are the ones its request doesn't ask for, `named`
 * the ones it names. `unlisted` when GitHub couldn't list the branch's
 * changes, so which ones is unknown.
 */
export type ControlFileChanges = {
  tampered: string[];
  named: string[];
  unlisted: boolean;
};

/** Whether the Run merges only once a person approves its Control Files. */
export function isTampered(changes: ControlFileChanges): boolean {
  return changes.unlisted || changes.tampered.length > 0;
}

/**
 * The base branch a Run merges to is red: its required checks fail, so no
 * Run merges to it until they pass again.
 */
export type MainHold = {
  /** When the base was first seen red. */
  since: string;
  /** The head commit the checks fail on. */
  headSha: string;
  /** The failing checks, one line each. */
  failed: string;
  /** The task that names the merges since the base was last green. */
  taskId?: string;
};

/** Why a base branch's required checks hold no merge. */
export type BaseRequiredChecks = 'unread' | 'none';

/** One Run of a Flow, with the Steps it took in the order they started. */
/** What kind of trigger started an Automation's Run. */
export type AutomationTriggerKind =
  | 'schedule'
  | 'repository_event'
  | 'webhook'
  | 'task_event';

/**
 * The Factory Definition a Run started under: its repository, and the
 * commit of its default branch the Run read.
 */
export type RunDefinition = {
  repoUrl: string;
  commit: string;
};

/** An act a Standing Role allows an agent to do for a Run's owner. */
export type StandingAct =
  | 'start'
  | 'cancel'
  | 'link'
  | 'answer_plan'
  | 'answer_blockers';

/**
 * What an agent did to a Run under a Standing Role of its Factory
 * Definition: the act, the Gate's Step for an answer, the Agent and the
 * Standing Role, and when.
 */
export type RunStandingAct = {
  act: StandingAct;
  seq?: number;
  agent: string;
  standingRole: string;
  at: string;
};

/**
 * An Agent Rule of a Run's Factory Definition that chose the Agent its
 * Steps of a Role start with, from its task's tags or its repository, as
 * the Run started.
 */
export type RunAgentRule = {
  role: string;
  rule: string;
  agent: string;
};

/**
 * Why a Run's token budget ended it: its agents used the budget or more
 * (`used`, counted as Insights counts tokens), or its usage could not be
 * read for too many agent Steps in a row.
 */
export type BudgetStop =
  | { reason: 'spent'; used: number }
  | { reason: 'unread' };

/**
 * A Run's token budget: the most tokens its agents may use, what they used
 * so far (unknown when the usage couldn't be read, or the Run was listed),
 * and why the budget ended the Run, when it did.
 */
export type RunTokenBudget = {
  budget: number;
  used?: number;
  /**
   * False when the Run's agent sessions record no token use at all (an
   * agent whose runtime is not measured): its budget cannot apply.
   */
  measured?: boolean;
  stop?: BudgetStop;
};

/** The Automation that started a Run, as it was then. */
export type RunAutomation = {
  id: string;
  name: string;
  trigger: AutomationTriggerKind;
};

/**
 * What a Run that retries another keeps of it: the Run it retries, the Run
 * whose branch it works on (the first of a chain of retries), and the place
 * of its own first Step: the Steps before it are the retried Run's, kept.
 */
export type RunRetry = {
  retriedFrom: string;
  branchOf: string;
  firstSeq: number;
};

export type Run = {
  id: string;
  request: string;
  createdAt: string;
  /** Its owner, by user id: the person its agent Steps run as. */
  ownerId?: string;
  /** The built-in Flow it walks, by name. */
  flow?: string;
  /** Whether its owner started it in their own signed-in session. */
  createdInPerson?: boolean;
  taskId?: string;
  /** The Project its task was in when it started. */
  projectId?: string;
  /** The page of the pull request its branch proposes its change in. */
  pullRequestUrl?: string;
  repository?: RunRepository;
  /** The Automation that started it; none for a Run a person started. */
  automation?: RunAutomation;
  /** The Factory Definition it started under; none for the built-in Flows. */
  definition?: RunDefinition;
  /** The Control Files its branch changed, if any. */
  controlFiles?: ControlFileChanges;
  /** How and when it ended, and for a flawed Run, why. */
  end?: { outcome: RunOutcome; endedAt: string; flaw?: RunFlaw };
  steps: RunStep[];
  /** While the Run is at its merge and its base branch is red. */
  mainHold?: MainHold;
  /**
   * While the Run is at its merge, why its base branch's required checks
   * hold nothing: they couldn't be read, or the branch requires none.
   */
  baseRequiredChecks?: BaseRequiredChecks;
  /** Its review rounds, in order; none for a Run with no review panel. */
  reviewRounds?: ReviewRound[];
  /** What agents did to it under its Definition's Standing Roles, in order. */
  standingActs?: RunStandingAct[];
  /** The Agent Rules that chose its Roles' first Agents, by Role. */
  agentRules?: RunAgentRule[];
  /**
   * Its task's tags couldn't be read as it started, so no Agent Rule on a
   * tag applied: not the same as a task no rule matched.
   */
  taskTagsUnread?: boolean;
  /** Its token budget; none for a Run that keeps none. */
  tokenBudget?: RunTokenBudget;
  /** What became of its blockers; none from a service older than disputes. */
  blockerResolutions?: BlockerResolutions;
  /**
   * The breaks its write and fix Steps reported, by Step; only a Run's own
   * read carries them, a list of Runs none.
   */
  breakRecords?: BreakRecord[];
  /** The Run it retries, for a retry; none for a Run started afresh. */
  retry?: RunRetry;
};

/** A Flow a person can start a Run of. */
export type Flow = {
  name: string;
  description: string;
  /** A Run of it works on a GitHub repository, which starting it names. */
  repository: boolean;
};

/** Where a Run is now. */
export type RunStatus =
  /** A Step is running, or the next one is about to start. */
  | { kind: 'running'; step?: RunStep }
  /** The Run waits for a person to answer `step`'s Gate. */
  | { kind: 'waiting'; step: RunStep; gate: Gate }
  | { kind: 'ended'; outcome: RunOutcome; endedAt: string };

/**
 * The Run's status. A Run waits only on the Gate of its latest Step: an
 * earlier Gate was answered before the Run went on.
 */
export function runStatus(run: Run): RunStatus {
  if (run.end) {
    return {
      kind: 'ended',
      outcome: run.end.outcome,
      endedAt: run.end.endedAt,
    };
  }

  const step = run.steps.at(-1);
  if (step?.gate && !step.gate.answer) {
    return { kind: 'waiting', step, gate: step.gate };
  }

  return { kind: 'running', step };
}

/** The answer that agrees to a Gate; on a Tampered Run it needs approval. */
export const APPROVE_ANSWER = 'approve';

/** The one answer an agent's question offers: the owner's words go with it. */
export const QUESTION_ANSWER = 'answer';

/** Whether an agent asked `gate`'s question, so the owner writes the answer. */
export function isQuestion(gate: Pick<Gate, 'kind'>): boolean {
  return gate.kind === 'question';
}

/**
 * The Role of the agent that asks `gate`'s question, the Gate of `step`:
 * none for a question the Flow asks.
 */
export function gateAsker(
  step: Pick<RunStep, 'role'>,
  gate: Pick<Gate, 'kind'>
): string | undefined {
  return isQuestion(gate) ? (step.role ?? 'agent') : undefined;
}

/** The first line of the request, which names the Run in lists. */
export function runTitle(run: Run): string {
  return run.request.trim().split('\n', 1)[0] ?? '';
}

/** What the request says after the first line, which is the Run's title. */
export function runRequestBody(run: Run): string {
  const [, ...rest] = run.request.trim().split('\n');
  return rest.join('\n').trim();
}

/** The path of a finding about the whole change rather than one file. */
export const WHOLE_CHANGE = '.';

/** The review round whose first Step is at `seq`, if one starts there. */
export function roundStartingAt(
  rounds: ReviewRound[],
  seq: number
): ReviewRound | undefined {
  return rounds.find((round) => round.seq === seq);
}

/**
 * Whether `round` reviews afresh after its entry was reset (as when the
 * change is sent back): the Run's round before it of the same entry, the
 * entry its first Step in `steps` is of, came at the same visit or a later
 * one. A round of a second panel entry is not a fresh review of the first.
 */
export function reviewsAfresh(
  rounds: ReviewRound[],
  steps: Pick<RunStep, 'seq' | 'entry'>[],
  round: ReviewRound
): boolean {
  const entryOf = (seq: number) =>
    steps.find((step) => step.seq === seq)?.entry;
  const entry = entryOf(round.seq);
  if (entry === undefined) return false;
  const before = rounds
    .filter((other) => other.seq < round.seq && entryOf(other.seq) === entry)
    .at(-1);
  return before !== undefined && before.visit >= round.visit;
}

/** What the reviewer of the Step at `seq` found, in its order. */
export function findingsOf(
  rounds: ReviewRound[],
  seq: number
): ReviewFinding[] {
  return rounds.flatMap((round) =>
    round.findings.filter((finding) => finding.seq === seq)
  );
}

/** How many of a round's findings block, and how many became follow-ups. */
/** Whether any of `resolutions`' blockers was fixed or disputed. */
export function anyResolved(resolutions: BlockerResolutions | undefined) {
  return (
    resolutions !== undefined &&
    resolutions.fixed +
      resolutions.disputedUpheld +
      resolutions.disputedDropped +
      resolutions.dismissed >
      0
  );
}

export function findingCounts(round: ReviewRound): {
  blockers: number;
  followUps: number;
} {
  let blockers = 0;
  let followUps = 0;
  for (const finding of round.findings) {
    if (finding.kind === 'blocker') blockers += 1;
    if (finding.kind === 'follow_up') followUps += 1;
  }
  return { blockers, followUps };
}

/**
 * The agent session a person can watch and steer `step` in: a running Step
 * whose agent works in a session.
 */
export function steerableSession(step: RunStep): string | undefined {
  return step.end ? undefined : step.terminal?.sessionId;
}

/** Where a running Step stands for its Step Terminal. */
export function terminalState(
  step: RunStep
): 'working' | 'paused' | 'stopping' | 'between' {
  if (step.terminal?.stopAskedAt) return 'stopping';
  if (step.terminal?.pausedAt) return 'paused';
  return step.terminal?.betweenTurns ? 'between' : 'working';
}

/** How a Run ends that its owner can retry. */
const RETRIABLE_OUTCOMES: ReadonlySet<RunOutcome> = new Set([
  'failed',
  'infrastructure',
  'blocked',
]);

/**
 * The Flows whose Runs the factory never retries: a Benchmark trial and a
 * self-improvement Run belong to the suite or improvement that started
 * them.
 */
const UNRETRIED_FLOWS: ReadonlySet<string> = new Set(['benchmark', 'improve']);

/**
 * The retry of `run` among `runs` (its task's), if one is listed: the Run
 * whose `retry.retriedFrom` names it. A Run is retried at most once.
 */
export function retryOf(run: Pick<Run, 'id'>, runs: Run[]): Run | undefined {
  return runs.find((other) => other.retry?.retriedFrom === run.id);
}

/**
 * Whether `run` can be retried: it ended `failed`, `infrastructure` or
 * `blocked`, is no Benchmark trial or self-improvement Run, and no retry of
 * it is known.
 */
export function isRetriable(run: Run, retried: boolean): boolean {
  if (!run.end || !RETRIABLE_OUTCOMES.has(run.end.outcome)) return false;
  if (run.flow !== undefined && UNRETRIED_FLOWS.has(run.flow)) return false;
  return !retried;
}

/** The entry of the code Flow that files an oversized plan's child tasks. */
export const SPLIT_ENTRY = 'split';

/** A task a Run filed, by its id and name. */
export type FiledTask = { id: string; name: string };

/**
 * What ends the header the engine's split writes above the list of the
 * child tasks it filed (`split_task` in `built_in_steps/plan_review.rs`).
 */
const FILED_HEADER = "child tasks, filed as the run's owner.";

/** One line of the filed list: `- ` and a task's document mention. */
const FILED_LINE = /^- <m-document-mention>(.*)<\/m-document-mention>$/;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The task a filed line's mention names, if it reads as one. */
function filedTask(json: string): FiledTask | undefined {
  try {
    const mention: unknown = JSON.parse(json);
    if (
      mention &&
      typeof mention === 'object' &&
      'documentId' in mention &&
      typeof mention.documentId === 'string' &&
      UUID.test(mention.documentId) &&
      'documentName' in mention &&
      typeof mention.documentName === 'string'
    ) {
      return { id: mention.documentId, name: mention.documentName };
    }
  } catch {
    // A mention that doesn't read links no task.
  }
  return undefined;
}

/**
 * The child tasks a `split` Step filed: the list under the header its
 * output opens with, one task mention a line, in order. A task the plan's
 * reasons mention, in the header, is not one it filed. None for another
 * Step, or one that filed none.
 */
export function splitChildTasks(
  step: Pick<RunStep, 'entry' | 'end'>
): FiledTask[] {
  if (step.entry !== SPLIT_ENTRY || !step.end) return [];
  const output = step.end.output;
  const header = output.indexOf(FILED_HEADER);
  if (header < 0) return [];
  const lines = output
    .slice(output.indexOf('\n', header) + 1 || output.length)
    .split('\n');
  const tasks: FiledTask[] = [];
  for (const line of lines) {
    const text = line.trim();
    // Blank lines lead to the list; the list ends at its first other line.
    if (text === '') {
      if (tasks.length > 0) break;
      continue;
    }
    const match = FILED_LINE.exec(text);
    if (!match) break;
    const task = filedTask(match[1] ?? '');
    if (task && !tasks.some((filed) => filed.id === task.id)) tasks.push(task);
  }
  return tasks;
}
