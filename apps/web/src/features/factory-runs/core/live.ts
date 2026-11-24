import type { BoardRun } from './board';
import { fellShort } from './flow-words';
import type { Gate, Run, RunStep } from './run';

/**
 * What the live dashboard and the Run page read of a Run: its Flow's stages
 * and where the Run is on them, the agents working now, and the numbers of
 * the header strip. Everything the API does not send yet comes from
 * `stepRuntime`, the one place to fill it in.
 */

/**
 * The code Flow's main stages, in order, for a Run whose Definition the
 * person can't read: the built-in Flows. A Definition's own Flow lists its
 * entries, which replace these.
 */
const CODE_STAGES = [
  'plan',
  'plan-review',
  'write',
  'review',
  'fix',
  'accept',
  'publish',
  'ci',
  'merge-gate',
  'merge',
];

/**
 * Entries that loop on a main stage rather than stand as one: a CI fix
 * runs CI again, an update brings the branch up to date before the merge
 * is asked again, a split ends a plan its review found too big.
 */
const SIDE_ENTRIES: Record<string, string> = {
  'ci-fix': 'ci',
  update: 'merge-gate',
  split: 'plan-review',
};

/** One stage of a Flow, with the side entries that loop on it. */
export type FlowStage = { entry: string; side: string[] };

/**
 * The stages a Run of `flow` walks: the Definition's entries when it lists
 * the Flow, the code Flow's for `code`, else the entries the Run's Steps
 * visited, in the order they first did.
 */
export function flowStages(
  flow: string | undefined,
  definitionEntries: readonly string[] | undefined,
  steps: readonly Pick<RunStep, 'entry'>[] = []
): FlowStage[] {
  const entries =
    definitionEntries ??
    (flow === 'code'
      ? CODE_STAGES
      : [...new Set(steps.map((step) => step.entry))]);
  const main = entries.filter(
    (entry) => !(SIDE_ENTRIES[entry] && entries.includes(SIDE_ENTRIES[entry]))
  );
  return main.map((entry) => ({
    entry,
    side: entries.filter((side) => SIDE_ENTRIES[side] === entry),
  }));
}

/**
 * Where a stage is: done, the one working now, waiting on a person at its
 * Gate, where the Run stopped or failed, passed over, or not reached yet.
 */
export type StageState =
  | 'done'
  | 'current'
  | 'waiting'
  | 'failed'
  | 'stopped'
  | 'skipped'
  | 'pending';

/** One stage as the pipeline shows it. */
export type PipelineStage = {
  entry: string;
  state: StageState;
  /** How many times the Run came to it: a review after a fix is a second. */
  visits: number;
  /** How many times each of its side entries ran, by entry. */
  loops: { entry: string; count: number }[];
  /** Its Steps, every visit, in order. */
  steps: RunStep[];
  /** The Steps of its latest visit: a review panel's reviewers at once. */
  latest: RunStep[];
  /** Whether a Gate asked a person at it, and whether one waits now. */
  gate?: { open: boolean; gate: Gate; step: RunStep };
  /** When its latest visit started and ended. */
  startedAt?: string;
  endedAt?: string;
};

/** The Steps of `entry` grouped into visits: a new one each time the Run comes back. */
function visitsOf(steps: readonly RunStep[], entry: string): RunStep[][] {
  const groups: RunStep[][] = [];
  let inVisit = false;
  for (const step of steps) {
    if (step.entry !== entry) {
      inVisit = false;
      continue;
    }
    if (!inVisit) groups.push([]);
    groups.at(-1)?.push(step);
    inVisit = true;
  }
  return groups;
}

/** How many separate runs of `entry` the Steps hold. */
function runsOf(steps: readonly RunStep[], entry: string): number {
  let count = 0;
  let previous: string | undefined;
  for (const step of steps) {
    if (step.entry === entry && previous !== entry) count += 1;
    previous = step.entry;
  }
  return count;
}

/** Where a Run is on its Flow's stages. */
export function runPipeline(run: Run, stages: FlowStage[]): PipelineStage[] {
  const steps = [...run.steps].sort((a, b) => a.seq - b.seq);
  const stageOf = (entry: string) =>
    stages.find((stage) => stage.entry === entry || stage.side.includes(entry))
      ?.entry;
  const last = steps.at(-1);
  const position = last
    ? stages.findIndex((stage) => stage.entry === stageOf(last.entry))
    : -1;
  const endedWell =
    run.end?.outcome === 'success' || run.end?.outcome === 'flawed';

  return stages.map((stage, index) => {
    const own = steps.filter((step) => stageOf(step.entry) === stage.entry);
    const visits = visitsOf(steps, stage.entry);
    const latestVisit = visits.at(-1) ?? [];
    const latestOwn =
      index === position && last && last.entry !== stage.entry
        ? own.filter((step) => step.entry === last.entry).slice(-1)
        : latestVisit;
    const gateStep = [...own].reverse().find((step) => step.gate);
    const open =
      !run.end &&
      gateStep !== undefined &&
      gateStep === last &&
      !gateStep.gate?.answer;
    const ends = latestOwn.flatMap((step) =>
      step.end ? [step.end.endedAt] : []
    );
    let state: StageState;
    if (index === position) {
      if (run.end) {
        if (endedWell) state = 'done';
        else if (run.end.outcome === 'cancelled') state = 'stopped';
        else state = 'failed';
      } else state = open ? 'waiting' : 'current';
    } else if (index < position || (run.end && endedWell)) {
      state = own.length > 0 ? 'done' : 'skipped';
    } else {
      state = 'pending';
    }
    return {
      entry: stage.entry,
      state,
      visits: visits.length,
      loops: stage.side
        .map((entry) => ({ entry, count: runsOf(steps, entry) }))
        .filter((loop) => loop.count > 0),
      steps: own,
      latest: latestOwn,
      gate:
        gateStep?.gate !== undefined
          ? { open, gate: gateStep.gate, step: gateStep }
          : undefined,
      startedAt: latestOwn[0]?.startedAt,
      endedAt:
        ends.length === latestOwn.length && ends.length > 0
          ? ends.sort().at(-1)
          : undefined,
    };
  });
}

/** The Steps working now: a review panel runs its reviewers at once. */
export function workingSteps(run: Run): RunStep[] {
  if (run.end) return [];
  return run.steps.filter((step) => !step.end);
}

/**
 * Where a Step runs and on what. The API does not send these yet: the model
 * its agent runs, and the Machine its session runs on. Fill them in here
 * once a Step carries them; every view reads them from this one function.
 */
export type StepRuntime = {
  /** The model the Step's agent runs, such as `claude-opus-4`. */
  model?: string;
  /** The Machine the Step's session runs on, by its name. */
  machine?: string;
};

export function stepRuntime(_step: RunStep): StepRuntime {
  return {};
}

/** What a Gate asks a person, as the dashboard sorts them. */
export type GateType =
  | 'plan'
  | 'blockers'
  | 'ci'
  | 'merge'
  | 'question'
  | 'decision';

export function gateType(step: Pick<RunStep, 'entry'>, gate: Gate): GateType {
  if (gate.kind === 'question') return 'question';
  if (step.entry === 'plan' || step.entry === 'plan-review') return 'plan';
  if (step.entry === 'review' || step.entry === 'fix') return 'blockers';
  if (step.entry === 'ci' || step.entry === 'ci-fix') return 'ci';
  if (step.entry.startsWith('merge') || step.entry === 'update') {
    return 'merge';
  }
  return 'decision';
}

/** Milliseconds from `from` to `to` (now when unset), never negative. */
export function elapsedMs(from: string, to?: string, now = Date.now()) {
  const end = to ? Date.parse(to) : now;
  return Math.max(0, end - Date.parse(from));
}

/** A duration, compact: `42s`, `3m 05s`, `1h 12m`, `2d 4h`. */
export function formatElapsed(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ${String(seconds % 60).padStart(2, '0')}s`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/** The numbers of the dashboard's header strip. */
export type LiveNumbers = {
  running: number;
  waiting: number;
  mergedToday: number;
  /** Of the ended Runs read, in percent; none while none ended. */
  successRate?: number;
  /** How many ended Runs the rate counts. */
  ended: number;
  /** The median time from start to merge of the merged Runs read. */
  medianToMerge?: number;
};

function sameDay(a: Date, b: Date) {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

export function liveNumbers(input: {
  active: readonly BoardRun[];
  waiting: number;
  recent: readonly BoardRun[];
  now: number;
}): LiveNumbers {
  const today = new Date(input.now);
  const ended = input.recent.filter(
    (listed) => listed.run.end && listed.run.end.outcome !== 'cancelled'
  );
  const merged = ended.filter(
    (listed) => listed.run.end?.outcome === 'success'
  );
  const durations = merged
    .map((listed) =>
      elapsedMs(listed.run.createdAt, listed.run.end?.endedAt, input.now)
    )
    .sort((a, b) => a - b);
  return {
    running: input.active.filter((listed) => listed.state !== 'waiting').length,
    waiting: input.waiting,
    mergedToday: merged.filter((listed) =>
      sameDay(new Date(listed.run.end?.endedAt ?? 0), today)
    ).length,
    successRate:
      ended.length > 0
        ? Math.round((merged.length / ended.length) * 100)
        : undefined,
    ended: ended.length,
    medianToMerge:
      durations.length > 0
        ? durations[Math.floor((durations.length - 1) / 2)]
        : undefined,
  };
}

/** Whether a Step's outcome fell short, its Gate's answer deciding. */
export function stepFellShort(step: RunStep): boolean {
  return step.end
    ? fellShort(step.end.outcome, step.gate?.answer?.answer)
    : false;
}
