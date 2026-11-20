import {
  SplitPanelContext,
  type SplitPanelContextType,
} from '@components/app/split-layout/context';
import { ToastRegion } from '@core/component/Toast/ToastRegion';
import { UserContextProvider } from '@core/context/user';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import { queryClient } from '@queries/client';
import { QueryClientProvider } from '@tanstack/solid-query';
import { ImperativeDialogHost } from '@ui';
import { createEffect, createSignal } from 'solid-js';
import { render } from 'solid-js/web';
import '@fontsource-variable/inter';
import '../../../index.css';
import {
  type InsightsContext,
  InsightsProvider,
} from '../context/insights-context';
import { type RunsContext, RunsProvider } from '../context/runs-context';
import type {
  EntryInsight,
  InsightsParams,
  OutcomeCounts,
  RunInsights,
  RunLink,
} from '../core/insights';
import type { QualityInsights, QualityParams } from '../core/quality';
import type { Run } from '../core/run';
import { RunInsightsView } from '../insights-view';

// `touch:` styles key on this attribute, as the app shell sets it.
document.documentElement.dataset.touchDevice = String(isTouchDevice());

// `?error` fails the first read; `?empty` has no Runs in any period.

/** The person the fixture shows, who owns its Runs. */
const FIXTURE_OWNER = 'macro|owner@example.com';
const search = new URLSearchParams(location.search);
const FAIL_FIRST = search.has('error');
const EMPTY = search.has('empty');
const NOW = new Date('2026-11-16T15:00:00Z');
const DAY = 86_400_000;

const counts = (changes: Partial<OutcomeCounts> = {}): OutcomeCounts => ({
  running: 0,
  success: 0,
  manual: 0,
  blocked: 0,
  failed: 0,
  cancelled: 0,
  infrastructure: 0,
  flawed: 0,
  ...changes,
});

const add = (a: OutcomeCounts, b: OutcomeCounts): OutcomeCounts => ({
  running: a.running + b.running,
  success: a.success + b.success,
  manual: a.manual + b.manual,
  blocked: a.blocked + b.blocked,
  failed: a.failed + b.failed,
  cancelled: a.cancelled + b.cancelled,
  infrastructure: a.infrastructure + b.infrastructure,
  flawed: a.flawed + b.flawed,
});

const RUNS: Run[] = [
  {
    id: 'run-ci',
    request: 'Add a due date column to the task list',
    ownerId: FIXTURE_OWNER,
    createdAt: new Date(+NOW - 2 * DAY).toISOString(),
    end: {
      outcome: 'manual',
      endedAt: new Date(+NOW - 2 * DAY + 3_600_000).toISOString(),
    },
    steps: [
      {
        seq: 1,
        entry: 'ci',
        visit: 1,
        startedAt: new Date(+NOW - 2 * DAY).toISOString(),
        end: {
          outcome: 'red',
          output: 'Two tests failed in the task list.',
          endedAt: new Date(+NOW - 2 * DAY + 600_000).toISOString(),
        },
      },
    ],
  },
  {
    id: 'run-review',
    request: 'Fix the empty state of the board',
    ownerId: FIXTURE_OWNER,
    createdAt: new Date(+NOW - 5 * DAY).toISOString(),
    end: {
      outcome: 'success',
      endedAt: new Date(+NOW - 5 * DAY + 5_400_000).toISOString(),
    },
    steps: [],
  },
  {
    id: 'run-failed',
    request: 'Plan the CSV import',
    ownerId: FIXTURE_OWNER,
    createdAt: new Date(+NOW - 9 * DAY).toISOString(),
    end: {
      outcome: 'failed',
      endedAt: new Date(+NOW - 9 * DAY + 600_000).toISOString(),
    },
    steps: [],
  },
];

const link = (run: Run): RunLink => ({
  id: run.id,
  title: run.request,
  createdAt: run.createdAt,
  outcome: run.end?.outcome,
});

/** A day's Runs, varied but fixed, so screenshots compare. */
function dayCounts(index: number, flow: string): OutcomeCounts {
  const seed = (index * 7 + (flow === 'code' ? 3 : 0)) % 11;
  if (flow === 'reply') return counts({ success: seed % 3 === 0 ? 1 : 0 });
  return counts({
    success: seed % 4,
    manual: flow === 'code' && seed % 5 === 0 ? 1 : 0,
    blocked: seed === 7 ? 1 : 0,
    failed: seed % 6 === 1 ? 1 : 0,
    cancelled: seed === 9 ? 1 : 0,
    infrastructure: seed === 10 ? 1 : 0,
    flawed: flow === 'code' && seed === 2 ? 1 : 0,
    running: index > 27 && seed % 2 === 0 ? 1 : 0,
  });
}

const PLAN: EntryInsight = {
  entry: 'plan',
  steps: 58,
  runs: 44,
  revisitedRuns: 11,
  failedRuns: 3,
  troubledRuns: 13,
  duration: { count: 57, medianSeconds: 212, p90Seconds: 540 },
  outcomes: [
    { name: 'done', count: 55 },
    { name: 'failed', count: 3 },
  ],
  gate: {
    opened: 55,
    answered: 53,
    wait: { count: 53, medianSeconds: 1_140, p90Seconds: 14_400 },
    answers: [
      { name: 'approve', count: 42 },
      { name: 'revise', count: 11 },
    ],
  },
  tokens: {
    input: 210_000,
    output: 64_000,
    cacheRead: 1_900_000,
    cacheWrite: 120_000,
  },
  trouble: [
    { run: link(RUNS[2]), failed: true, visits: 1 },
    { run: link(RUNS[1]), failed: false, visits: 3 },
  ],
};

const CODE_ENTRIES: EntryInsight[] = [
  {
    entry: 'write',
    steps: 40,
    runs: 40,
    revisitedRuns: 0,
    failedRuns: 1,
    troubledRuns: 1,
    duration: { count: 39, medianSeconds: 1_260, p90Seconds: 3_300 },
    outcomes: [
      { name: 'done', count: 36 },
      { name: 'unchanged', count: 3 },
      { name: 'failed', count: 1 },
    ],
    tokens: {
      input: 820_000,
      output: 240_000,
      cacheRead: 9_600_000,
      cacheWrite: 410_000,
    },
    trouble: [],
  },
  {
    entry: 'review',
    steps: 52,
    runs: 34,
    revisitedRuns: 14,
    failedRuns: 0,
    troubledRuns: 14,
    duration: { count: 52, medianSeconds: 420, p90Seconds: 900 },
    outcomes: [
      { name: 'clean', count: 31 },
      { name: 'blockers', count: 21 },
    ],
    tokens: {
      input: 300_000,
      output: 52_000,
      cacheRead: 2_800_000,
      cacheWrite: 90_000,
    },
    trouble: [{ run: link(RUNS[1]), failed: false, visits: 2 }],
  },
  {
    entry: 'fix',
    steps: 21,
    runs: 14,
    revisitedRuns: 5,
    failedRuns: 0,
    troubledRuns: 5,
    duration: { count: 21, medianSeconds: 640, p90Seconds: 1_500 },
    outcomes: [{ name: 'done', count: 21 }],
    tokens: {
      input: 260_000,
      output: 70_000,
      cacheRead: 2_100_000,
      cacheWrite: 80_000,
    },
    trouble: [{ run: link(RUNS[1]), failed: false, visits: 2 }],
  },
  {
    entry: 'ci',
    steps: 38,
    runs: 30,
    revisitedRuns: 6,
    failedRuns: 1,
    troubledRuns: 6,
    duration: { count: 37, medianSeconds: 780, p90Seconds: 1_800 },
    outcomes: [
      { name: 'green', count: 29 },
      { name: 'red', count: 8 },
      { name: 'failed', count: 1 },
    ],
    trouble: [{ run: link(RUNS[0]), failed: false, visits: 2 }],
  },
  {
    entry: 'ci-fix',
    steps: 8,
    runs: 6,
    revisitedRuns: 2,
    failedRuns: 0,
    troubledRuns: 2,
    duration: { count: 8, medianSeconds: 900, p90Seconds: 2_000 },
    outcomes: [{ name: 'done', count: 8 }],
    tokens: {
      input: 90_000,
      output: 20_000,
      cacheRead: 640_000,
      cacheWrite: 30_000,
    },
    trouble: [{ run: link(RUNS[0]), failed: false, visits: 2 }],
  },
];

/** Insights as the service would answer `params`, from fixed numbers. */
function insightsFor(params: InsightsParams): RunInsights {
  const flows = params.flow ? [params.flow] : ['code', 'plan', 'reply'];
  const length =
    Math.round((Date.parse(params.to) - Date.parse(params.from)) / DAY) + 1;
  const days = Array.from({ length }, (_, index) => {
    const date = new Date(Date.parse(params.from) + index * DAY)
      .toISOString()
      .slice(0, 10);
    const total = EMPTY
      ? counts()
      : flows.reduce(
          (sum, flow) => add(sum, dayCounts(index + 30 - length, flow)),
          counts()
        );
    return { date, counts: total };
  });
  const flowCounts = EMPTY
    ? []
    : flows.map((flow) => ({
        flow,
        counts: days.reduce(
          (sum, _, index) => add(sum, dayCounts(index + 30 - length, flow)),
          counts()
        ),
      }));
  const entries = EMPTY
    ? []
    : [
        ...(flows.includes('code') || flows.includes('plan') ? [PLAN] : []),
        ...(flows.includes('code') ? CODE_ENTRIES : []),
      ];
  return {
    from: params.from,
    to: params.to,
    timeZone: params.timeZone,
    flow: params.flow,
    runCap: 1000,
    capped: false,
    totals: days.reduce((sum, day) => add(sum, day.counts), counts()),
    days,
    flows: flowCounts,
    runDuration: EMPTY
      ? undefined
      : { count: 61, medianSeconds: 3_000, p90Seconds: 19_800 },
    entries,
    usage: {
      agentSteps: EMPTY ? 0 : 160,
      measuredSteps: EMPTY ? 0 : 131,
      measuredRuns: EMPTY ? 0 : 39,
      tokens: EMPTY
        ? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }
        : {
            input: 1_680_000,
            output: 446_000,
            cacheRead: 17_040_000,
            cacheWrite: 730_000,
          },
      heaviest: EMPTY
        ? []
        : [
            {
              run: link(RUNS[1]),
              tokens: {
                input: 160_000,
                output: 41_000,
                cacheRead: 1_500_000,
                cacheWrite: 60_000,
              },
            },
            {
              run: link(RUNS[0]),
              tokens: {
                input: 120_000,
                output: 30_000,
                cacheRead: 990_000,
                cacheWrite: 40_000,
              },
            },
          ],
    },
  };
}

/** The factory in memory: a read takes a moment, as over the network. */
function createFixtureInsights(): InsightsContext {
  let failNext = FAIL_FIRST;
  return {
    insights: (params) => {
      const [state, setState] = createSignal<'pending' | 'error' | 'ready'>(
        'pending'
      );
      const [data, setData] = createSignal<RunInsights>();
      const [updating, setUpdating] = createSignal(false);
      let reads = 0;
      const read = (shown: InsightsParams) => {
        const current = ++reads;
        if (data()) setUpdating(true);
        setTimeout(() => {
          if (current !== reads) return;
          setUpdating(false);
          if (failNext) {
            failNext = false;
            setState('error');
            return;
          }
          setData(insightsFor(shown));
          setState('ready');
        }, 400);
      };
      const source = {
        state,
        data,
        updating,
        retry: () => {
          setState('pending');
          read(params());
        },
      };
      // Read again whenever the period or the Flow changes.
      createEffect(() => read(params()));
      return source;
    },
    // No Scorer, Experiment or Benchmark is set up in the fixture.
    evaluation: (params) => ({
      state: () => 'ready',
      data: () => ({
        from: params().from,
        to: params().to,
        timeZone: params().timeZone,
        scorers: [],
        experiments: [],
        agentRules: [],
        benchmarks: [],
        suites: [],
      }),
      updating: () => false,
      retry: () => {},
    }),
    // Two repositories' CI waits, review rounds and merges.
    quality: (params) => ({
      state: () => 'ready',
      data: () => qualityFixture(params()),
      updating: () => false,
      retry: () => {},
    }),
    flowNames: () => ({
      state: () => 'ready',
      data: () => ['plan', 'reply'],
      retry: () => {},
    }),
  };
}

function createFixtureRuns(): RunsContext {
  const ready = <T,>(data: () => T) => ({
    state: () => 'ready' as const,
    data,
    retry: () => {},
  });
  return {
    viewerId: () => FIXTURE_OWNER,
    taskRuns: () => ready(() => RUNS),
    waitingRuns: () => ready(() => ({ waiting: [], starting: [] })),
    run: (runId) => ready(() => RUNS.find((run) => run.id === runId())),
    flows: () => ready(() => []),
    repositories: () => ready(() => []),
    startRun: async () => ({ ok: false, reason: 'failed' }),
    answerGate: async () => ({ ok: false, reason: 'failed' }),
    replyToGate: async () => ({ ok: false, reason: 'failed' }),
    cancelRun: async () => ({ ok: false, reason: 'failed' }),
    markRunFlawed: async () => ({ ok: false, reason: 'failed' }),
    steerStep: async () => ({ ok: false, reason: 'failed' }),
  };
}

/** The pane's header slots, where a mobile view's header portals to. */
const headerLeft = document.createElement('div');
headerLeft.className = 'flex min-w-0 flex-1 items-center gap-2';
const headerRight = document.createElement('div');
headerRight.className = 'flex shrink-0 items-center gap-2';

function Fixture() {
  const [displayName, setDisplayName] = createSignal('');
  const panel = {
    handle: {
      displayName,
      setDisplayName,
      isPopover: () => false,
    },
    layoutRefs: { headerLeft, headerRight },
    mobileHeaderOwner: createSignal<symbol>(),
    titleFileMenuTrigger: () => undefined,
  } as unknown as SplitPanelContextType;

  return (
    <SplitPanelContext.Provider value={panel}>
      <main
        class="flex h-dvh flex-col bg-panel text-ink"
        style={{
          '--mobile-content-inset-top': isTouchDevice() ? '64px' : '0px',
          '--mobile-content-inset-bottom': '24px',
        }}
      >
        <header
          class="absolute inset-x-0 top-0 z-10 flex h-14 items-center gap-2 px-3 touch:flex not-touch:hidden"
          ref={(element) => element.append(headerLeft, headerRight)}
        />
        <RunsProvider value={createFixtureRuns()}>
          <InsightsProvider value={createFixtureInsights()}>
            <RunInsightsView timeZone="Europe/Lisbon" now={() => NOW} />
          </InsightsProvider>
        </RunsProvider>
      </main>
      <ImperativeDialogHost />
      <ToastRegion />
    </SplitPanelContext.Provider>
  );
}

render(
  () => (
    <QueryClientProvider client={queryClient}>
      <UserContextProvider>
        <Fixture />
      </UserContextProvider>
    </QueryClientProvider>
  ),
  document.getElementById('root')!
);

const APP = 'https://github.com/macro/app';

/** CI health, review quality, cost and autonomy for the fixture's period. */
function qualityFixture(params: QualityParams): QualityInsights {
  const run = RUNS[0];
  const link = {
    id: run?.id ?? '00000000-0000-7000-8000-000000000001',
    title: 'Collapse repeated dashes',
    createdAt: '2026-11-10T09:00:00Z',
    outcome: 'success' as const,
  };
  return {
    from: params.from,
    to: params.to,
    timeZone: params.timeZone,
    flow: params.flow ?? null,
    repository: params.repository ?? null,
    repositories: [APP, 'https://github.com/macro/site'],
    runCap: 1000,
    capped: false,
    ci: {
      waits: 14,
      redWaits: 5,
      runs: 9,
      causes: { own: 2, setup: 2, flaky: 2, cancelled: 1 },
      jobs: [
        {
          repository: APP,
          name: 'test',
          finished: 13,
          red: 4,
          redRate: 4 / 13,
          causes: { own: 2, setup: 0, flaky: 2, cancelled: 1 },
          pullRequestsFailed: 3,
        },
        {
          repository: APP,
          name: 'build',
          finished: 14,
          red: 2,
          redRate: 2 / 14,
          causes: { own: 0, setup: 2, flaky: 0, cancelled: 0 },
          pullRequestsFailed: 2,
        },
      ],
      firstPushToGreen: { count: 8, medianSeconds: 1260, p90Seconds: 5400 },
      ciFixRounds: 5,
      runsWithCiFixes: 4,
      reviewFixRounds: 7,
      recurring: [
        {
          repository: APP,
          job: 'build',
          step: 'Set up job',
          cause: 'setup',
          pullRequests: 2,
          failures: 2,
          lastFailedAt: '2026-11-12T10:20:00Z',
          runs: [link],
        },
        {
          repository: APP,
          job: 'test',
          step: 'Run tests',
          cause: 'flaky',
          pullRequests: 2,
          failures: 2,
          lastFailedAt: '2026-11-11T10:20:00Z',
          runs: [link],
        },
      ],
    },
    review: {
      reviewedRuns: 9,
      rounds: 16,
      roundsPerRun: 16 / 9,
      firstRounds: 9,
      cleanFirstRounds: 3,
      cleanFirstRate: 3 / 9,
      blockers: 14,
      newBlockers: 11,
      repeatedBlockers: 3,
      byReviewer: [
        { name: 'correctness-reviewer', reviews: 16, blockers: 9 },
        { name: 'scope-reviewer', reviews: 16, blockers: 3 },
        { name: 'tests-reviewer', reviews: 16, blockers: 2 },
      ],
      byAgent: [
        { name: 'primary', reviews: 40, blockers: 12 },
        { name: 'backup', reviews: 8, blockers: 2 },
      ],
      recurring: [
        {
          text: 'The new code path has no test that fails without the change.',
          runs: 3,
          blockers: 4,
          reviewers: ['tests-reviewer'],
          lastFoundAt: '2026-11-12T09:40:00Z',
          runLinks: [link],
        },
      ],
    },
    delivery: {
      endedRuns: 10,
      merged: 6,
      autonomous: 3,
      autonomyRate: 0.3,
      personAnswered: 3,
      steered: 1,
      manual: 2,
      measuredRuns: 10,
      tokens: {
        input: 820000,
        output: 96000,
        cacheRead: 5400000,
        cacheWrite: 310000,
      },
      costUsd: 18.6,
      unpricedCalls: 0,
      costPerMergedUsd: 3.1,
      tokensPerMerged: 1104333,
    },
  };
}
