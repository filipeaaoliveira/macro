/** @vitest-environment jsdom */
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from '@solidjs/testing-library';
import { type Accessor, createSignal } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  type InsightsContext,
  InsightsProvider,
} from './context/insights-context';
import {
  type RunsContext,
  RunsProvider,
  type RunsQueue,
} from './context/runs-context';
import type { EvaluationInsights } from './core/evaluation';
import type { InsightsParams, RunInsights } from './core/insights';
import type { QualityInsights, QualityParams } from './core/quality';
import type { Run } from './core/run';
import { RunInsightsView } from './insights-view';

vi.mock('@core/mobile/isTouchDevice', () => ({
  isTouchDevice: () => false,
}));

// The chart draws with Observable Plot, which jsdom cannot lay out; the
// figures and tables beside it carry the same numbers.
vi.mock('@app/features/scheduling/components/insight-plot', () => ({
  InsightPlot: (props: { label: string }) => (
    <div role="img" aria-label={props.label} />
  ),
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
);

const failedImport: Run = {
  id: 'run-import',
  request: 'Plan the import',
  createdAt: '2026-11-11T10:00:00Z',
  end: { outcome: 'failed', endedAt: '2026-11-11T10:10:00Z' },
  steps: [],
};

const gateRun: Run = {
  id: 'run-gate',
  request: 'Plan the export',
  createdAt: '2026-11-11T11:00:00Z',
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-11T11:00:00Z',
      gate: {
        kind: 'flow',
        question: 'Approve the plan?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-11T11:05:00Z',
      },
    },
  ],
};

const startingRun: Run = {
  id: 'run-starting',
  request: 'Plan the archive',
  createdAt: '2026-11-11T11:30:00Z',
  steps: [],
};

const outcomes = (
  changes: Partial<RunInsights['totals']>
): RunInsights['totals'] => ({
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

const november: RunInsights = {
  from: '2026-11-10',
  to: '2026-11-11',
  timeZone: 'Europe/Lisbon',
  runCap: 1000,
  capped: false,
  totals: outcomes({ success: 1, failed: 1, running: 1 }),
  days: [
    { date: '2026-11-10', counts: outcomes({ success: 1 }) },
    { date: '2026-11-11', counts: outcomes({ failed: 1, running: 1 }) },
  ],
  flows: [{ flow: 'plan', counts: outcomes({ success: 1, failed: 1 }) }],
  runDuration: { count: 2, medianSeconds: 1200, p90Seconds: 1680 },
  entries: [
    {
      entry: 'plan',
      steps: 3,
      runs: 2,
      revisitedRuns: 1,
      failedRuns: 1,
      troubledRuns: 1,
      duration: { count: 3, medianSeconds: 300, p90Seconds: 540 },
      outcomes: [
        { name: 'done', count: 2 },
        { name: 'failed', count: 1 },
      ],
      gate: {
        opened: 2,
        answered: 2,
        wait: { count: 2, medianSeconds: 120, p90Seconds: 600 },
        answers: [
          { name: 'approve', count: 1 },
          { name: 'revise', count: 1 },
        ],
      },
      trouble: [
        {
          run: {
            id: failedImport.id,
            title: 'Plan the import',
            createdAt: failedImport.createdAt,
            outcome: 'failed',
          },
          failed: true,
          visits: 1,
        },
      ],
    },
  ],
  usage: {
    agentSteps: 2,
    measuredSteps: 1,
    measuredRuns: 1,
    tokens: { input: 1200, output: 300, cacheRead: 0, cacheWrite: 0 },
    heaviest: [],
  },
};

const judged: EvaluationInsights = {
  from: '2026-10-13',
  to: '2026-11-11',
  timeZone: 'Europe/Lisbon',
  scorers: [
    {
      name: 'correctness',
      instructions: 'Does the change do what the request asks?',
      flows: [],
      samplePercent: 20,
      judged: 4,
      passed: 3,
      meanScore: 0.7,
    },
  ],
  experiments: [
    {
      name: 'writer-agent',
      flow: 'code',
      running: true,
      arms: [
        {
          name: 'control',
          agents: {},
          runs: 3,
          ended: 2,
          succeeded: 1,
          fellBack: 0,
          scores: [],
        },
        {
          name: 'backup',
          agents: { writer: 'backup' },
          runs: 2,
          ended: 2,
          succeeded: 2,
          fellBack: 1,
          scores: [{ scorer: 'correctness', judged: 2, meanScore: 0.9 }],
        },
      ],
    },
  ],
  benchmarks: [
    {
      name: 'core',
      tasks: ['typo'],
      scorers: ['correctness'],
    },
  ],
  suites: [
    {
      id: 'suite-1',
      benchmark: 'core',
      agents: { writer: 'backup' },
      createdAt: '2026-11-10T09:00:00Z',
      scored: 1,
      infrastructure: 0,
      meanScore: 0.5,
      tokens: { input: 1200, output: 300, cacheRead: 0, cacheWrite: 0 },
      trials: [
        {
          task: 'typo',
          runId: failedImport.id,
          runOutcome: 'manual',
          score: 0.5,
          tokens: { input: 1200, output: 300, cacheRead: 0, cacheWrite: 0 },
          scores: [{ scorer: 'correctness', score: 0.25 }],
        },
      ],
    },
  ],
};

function mount(options: {
  state?: Accessor<'pending' | 'error' | 'ready'>;
  insights?: RunInsights;
  evaluation?: EvaluationInsights;
  evaluationState?: Accessor<'pending' | 'error' | 'ready'>;
  evaluationRetry?: () => void;
  quality?: (params: QualityParams) => QualityInsights;
  retry?: () => void;
  queue?: RunsQueue;
}) {
  const asked: InsightsParams[] = [];
  const insights: InsightsContext = {
    insights: (params) => {
      asked.push(params());
      const state = options.state ?? (() => 'ready' as const);
      return {
        state,
        data: () => (state() === 'ready' ? options.insights : undefined),
        retry: options.retry ?? vi.fn(),
        updating: () => false,
      };
    },
    evaluation: () => ({
      state:
        options.evaluationState ??
        (() => (options.evaluation ? 'ready' : 'pending')),
      data: () => options.evaluation,
      retry: options.evaluationRetry ?? vi.fn(),
      updating: () => false,
    }),
    quality: (params) => ({
      state: () => (options.quality ? 'ready' : 'pending'),
      data: () => options.quality?.(params()),
      retry: vi.fn(),
      updating: () => false,
    }),
    flowNames: () => ({
      state: () => 'ready',
      data: () => ['plan', 'reply'],
      retry: vi.fn(),
    }),
  };
  const runs: RunsContext = {
    taskRuns: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => options.queue ?? { waiting: [], starting: [] },
      retry: vi.fn(),
    }),
    run: (runId) => ({
      state: () => 'ready',
      data: () =>
        [failedImport, gateRun, startingRun].find((run) => run.id === runId()),
      retry: vi.fn(),
    }),
    flows: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    repositories: () => ({
      state: () => 'ready',
      data: () => [],
      retry: vi.fn(),
    }),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    replyToGate: vi.fn(),
    cancelRun: vi.fn(),
    markRunFlawed: vi.fn(),
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={runs}>
      <InsightsProvider value={insights}>
        <RunInsightsView
          timeZone="Europe/Lisbon"
          now={() => new Date('2026-11-11T12:00:00Z')}
        />
      </InsightsProvider>
    </RunsProvider>
  ));
  return { asked };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('RunInsightsView', () => {
  it('reads the last 30 days of every flow in the person’s time zone', () => {
    const { asked } = mount({ insights: november });

    expect(asked).toEqual([
      {
        from: '2026-10-13',
        to: '2026-11-11',
        timeZone: 'Europe/Lisbon',
        flow: undefined,
      },
    ]);
  });

  it('holds a placeholder while the first read loads', () => {
    mount({ state: () => 'pending' });

    expect(screen.getByLabelText('Loading run insights…')).toBeTruthy();
    expect(screen.queryByText('Total runs')).toBeNull();
  });

  it('says when insights could not be read, and reads them again on Retry', () => {
    const retry = vi.fn();
    mount({ state: () => 'error', retry });

    const alert = screen.getByRole('alert');
    fireEvent.click(within(alert).getByRole('button', { name: 'Retry' }));

    expect(alert.textContent).toContain('Run insights couldn’t be loaded');
    expect(retry).toHaveBeenCalledOnce();
  });

  it('shows how the runs went, the success rate leaving the running one out', () => {
    mount({ insights: november });

    const outcomesList = screen.getByRole('list', { name: 'Run outcomes' });
    expect(
      within(outcomesList).getByText('Failed').nextSibling?.textContent
    ).toBe('1');
    // One done and one failed run ended on their own: 50%.
    expect(
      screen.getByText('Success rate').closest('div')?.parentElement
        ?.textContent
    ).toContain('50');
    expect(screen.queryByText(/No runs match this period/)).toBeNull();
  });

  it('splits failures of the work from infrastructure, and flawed runs from done', () => {
    mount({
      insights: {
        ...november,
        totals: outcomes({
          success: 3,
          flawed: 1,
          failed: 2,
          infrastructure: 2,
        }),
      },
    });

    const outcomesList = screen.getByRole('list', { name: 'Run outcomes' });
    expect(
      within(outcomesList).getByText('Infrastructure failure').nextSibling
        ?.textContent
    ).toBe('2');
    expect(
      within(outcomesList).getByText('Flawed').nextSibling?.textContent
    ).toBe('1');
    const kinds = screen
      .getByRole('heading', { name: 'Quality and infrastructure' })
      .closest('section');
    if (!kinds) throw new Error('No quality and infrastructure section');
    // One flawed of the four that reached their end: 25%. Two of the eight
    // that ended on their own failed for infrastructure: 25%.
    expect(
      within(kinds).getByText('Failed for the work').nextSibling?.textContent
    ).toBe('2');
    expect(
      within(kinds).getByText('Failed for infrastructure').nextSibling
        ?.textContent
    ).toBe('2');
    const figure = (label: string) =>
      within(kinds)
        .getByText(label)
        .closest('div')
        ?.parentElement?.querySelector('p')?.textContent;
    expect(figure('Flawed')).toBe('25%');
    expect(figure('Infrastructure failures')).toBe('25%');
    // The success rate leaves the infrastructure failures out: 3 of 6.
    expect(
      screen
        .getByText('Success rate')
        .closest('div')
        ?.parentElement?.querySelector('p')?.textContent
    ).toBe('50%');
  });

  it('says there is nothing to show for a period with no runs', () => {
    const [state] = createSignal<'ready'>('ready');
    mount({
      state,
      insights: {
        ...november,
        totals: outcomes({}),
        days: [],
        flows: [],
        entries: [],
        runDuration: undefined,
      },
    });

    expect(screen.getByText(/No runs match this period/)).toBeTruthy();
    // With no run ended, there is no rate or time to show, not a zero.
    const figure = (label: string) =>
      screen.getByText(label).closest('div')?.parentElement?.querySelector('p')
        ?.textContent;
    expect(figure('Success rate')).toBe('–');
    expect(figure('Median run time')).toBe('–');
    expect(
      screen.getByText('No step failed or went again in this period.')
    ).toBeTruthy();
  });

  it('opens a run named where runs stop', async () => {
    mount({ insights: november });

    const stops = screen.getByRole('list', { name: 'Runs at Plan' });
    fireEvent.click(
      within(stops).getByRole('button', { name: /Plan the import/ })
    );

    // The Run opens as from its task: its Steps, which this one has none of.
    expect(await screen.findByText('No steps yet')).toBeTruthy();
  });

  it('lists the runs holding a place before their first gate apart from those at a gate', async () => {
    mount({
      insights: november,
      queue: { waiting: [gateRun], starting: [startingRun] },
    });

    expect(screen.getByText('Waiting on you')).toBeTruthy();
    const holding = screen.getByRole('region', {
      name: 'Yet to ask you anything',
    });
    expect(within(holding).getByText('Plan the archive')).toBeTruthy();
    expect(within(holding).queryByText('Plan the export')).toBeNull();
    expect(screen.getByText('Plan the export')).toBeTruthy();

    fireEvent.click(
      within(holding).getByRole('button', { name: /Plan the archive/ })
    );
    expect(await screen.findByText('No steps yet')).toBeTruthy();
  });

  it('shows Waiting on you while only runs yet to ask anything hold a place', () => {
    mount({
      insights: november,
      queue: { waiting: [], starting: [startingRun] },
    });

    expect(screen.getByText('Waiting on you')).toBeTruthy();
    expect(screen.getByText('Plan the archive')).toBeTruthy();
  });

  it('shows each scorer, each experiment’s arms and each benchmark suite’s trials', async () => {
    mount({ insights: november, evaluation: judged });

    const scorers = screen.getByRole('heading', { name: 'Scorers' })
      .parentElement?.parentElement?.parentElement as HTMLElement;
    expect(within(scorers).getByText('correctness')).toBeTruthy();
    expect(within(scorers).getByText('0.70')).toBeTruthy();
    expect(within(scorers).getByText('75%')).toBeTruthy();

    const backup = screen.getByRole('rowheader', { name: /backup/ });
    expect(backup.parentElement?.textContent).toContain('0.90');
    expect(backup.textContent).toContain('writer: backup');
    expect(backup.textContent).toContain('1 run fell back to another agent');

    const trials = screen.getByRole('list', { name: 'Trial' });
    const trial = within(trials).getByRole('button', { name: /typo/ });
    expect(trial.textContent).toContain('0.25');
    expect(trial.textContent).toContain('0.50');

    fireEvent.click(trial);
    expect(await screen.findByText('No steps yet')).toBeTruthy();
  });

  it('says how many of a suite’s trials were lost to infrastructure, with no score', () => {
    const [suite] = judged.suites;
    if (!suite) throw new Error('No suite');
    mount({
      insights: november,
      evaluation: {
        ...judged,
        suites: [
          {
            ...suite,
            infrastructure: 1,
            trials: [
              ...suite.trials,
              {
                task: 'export',
                runId: 'run-lost',
                runOutcome: 'infrastructure',
                score: null,
                tokens: null,
                scores: [],
              },
            ],
          },
        ],
      },
    });

    expect(screen.getByText('1 of 2 trials scored')).toBeTruthy();
    expect(
      screen.getByText('1 lost to infrastructure, left out of the score')
    ).toBeTruthy();
    const trials = screen.getByRole('list', { name: 'Trial' });
    const lost = within(trials).getByRole('button', { name: /export/ });
    expect(lost.textContent).not.toMatch(/\d\.\d\d/);
  });

  it('shows the team’s pending failures and its last self-improvement run, and opens it', async () => {
    mount({
      insights: november,
      evaluation: {
        ...judged,
        selfImprovement: {
          repoUrl: 'https://github.com/macro/factory-definition',
          enabled: true,
          failuresThreshold: 25,
          oldestDays: 7,
          namer: 'admin',
          pending: 4,
          oldestPendingAt: '2026-11-05T09:00:00Z',
          pendingBySource: [
            { source: 'correctness', count: 3 },
            { source: 'flawed', count: 1 },
          ],
          last: {
            runId: failedImport.id,
            state: 'closed',
            failures: 12,
            createdAt: '2026-11-01T09:00:00Z',
          },
        },
      },
    });

    const section = screen.getByRole('heading', { name: 'Self-improvement' })
      .parentElement?.parentElement?.parentElement as HTMLElement;
    expect(within(section).getByText('4')).toBeTruthy();
    expect(within(section).getByText('25 failures or 7 days')).toBeTruthy();
    expect(section.textContent).toContain('correctness 3');
    expect(section.textContent).toContain('Marked flawed 1');
    expect(within(section).getByText('Closed unmerged')).toBeTruthy();
    expect(section.textContent).toContain('12 failures');

    fireEvent.click(within(section).getByRole('button', { name: 'Open run' }));
    expect(await screen.findByText('No steps yet')).toBeTruthy();
  });

  it('says no self-improvement run starts once the definition’s namer left the team', () => {
    mount({
      insights: november,
      evaluation: {
        ...judged,
        selfImprovement: {
          repoUrl: 'https://github.com/macro/factory-definition',
          enabled: true,
          failuresThreshold: 25,
          oldestDays: 7,
          namer: 'left_team',
          pending: 30,
          pendingBySource: [{ source: 'correctness', count: 30 }],
        },
      },
    });

    expect(
      screen.getByText(
        'The admin who named the factory definition left the team, so no run starts. An admin names it again in team settings.'
      )
    ).toBeTruthy();
  });

  it('shows no self-improvement for a team with no factory definition', () => {
    mount({ insights: november, evaluation: judged });
    expect(
      screen.queryByRole('heading', { name: 'Self-improvement' })
    ).toBeNull();
  });

  it('holds a placeholder while the evaluation loads, and reads it again on Retry', () => {
    mount({ insights: november });
    expect(
      screen.getByLabelText('Loading scorers, experiments and benchmarks…')
    ).toBeTruthy();
    cleanup();

    const retry = vi.fn();
    mount({
      insights: november,
      evaluationState: () => 'error',
      evaluationRetry: retry,
    });
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain(
      'Scorers, experiments and benchmarks couldn’t be loaded'
    );
    fireEvent.click(within(alert).getByRole('button', { name: 'Retry' }));
    expect(retry).toHaveBeenCalledOnce();
  });

  it('lists a setup failure on two pull requests once, as setup, beside a flaky test and a pull request’s own failure', () => {
    mount({
      insights: november,
      quality: () => ({ ...quality(), repository: APP }),
    });

    const recurring = screen.getByRole('list', {
      name: 'Failed on two or more pull requests',
    });
    const rows = within(recurring).getAllByRole('heading');
    expect(rows.map((row) => row.textContent)).toEqual([
      'build · Set up job',
      'test · Run tests',
    ]);
    const setup = rows[0]?.parentElement as HTMLElement;
    expect(within(setup).getByText('Setup')).toBeTruthy();
    expect(setup.parentElement?.textContent).toContain(
      '2 failures on 2 pull requests'
    );
    const flaky = rows[1]?.parentElement as HTMLElement;
    expect(within(flaky).getByText('Flaky')).toBeTruthy();
    // A flaky failure names the test that failed on both pull requests.
    expect(flaky.parentElement?.textContent).toContain(
      'src/net.rs: net::test::retry'
    );
    expect(setup.parentElement?.textContent).not.toContain('src/');

    const e2e = screen.getByRole('rowheader', { name: 'e2e' });
    expect(e2e.parentElement?.textContent).toContain('100%');
    expect(e2e.parentElement?.textContent).toContain('The pull request’s 1');
    expect(figure('Red waits')).toBe('50%');
    expect(figure('First push to green')).toBe('2hr');
    expect(figure('CI fix rounds')).toBe('2');
    expect(figure('Review fix rounds')).toBe('1');
  });

  it('shows review rounds, blockers by reviewer and agent, and recurring blocker texts', () => {
    mount({ insights: november, quality: () => quality() });

    const reviewers = screen.getByRole('table', {
      name: 'Blockers by reviewer',
    });
    const correctness = within(reviewers).getByRole('rowheader', {
      name: 'correctness-reviewer',
    });
    expect(
      [...(correctness.parentElement?.querySelectorAll('td') ?? [])].map(
        (cell) => cell.textContent
      )
    ).toEqual(['4', '3', '0.8']);
    // Written for the interface's locale, with its grouping.
    const style = within(reviewers).getByRole('rowheader', {
      name: 'style-reviewer',
    });
    expect(
      style.parentElement?.querySelector('td:last-child')?.textContent
    ).toBe('1,234.5');
    const agents = screen.getByRole('table', { name: 'Blockers by agent' });
    expect(
      within(agents).getByRole('rowheader', { name: 'backup' })
    ).toBeTruthy();
    const recurring = screen.getByRole('list', {
      name: 'Blockers found in two or more runs',
    });
    expect(recurring.textContent).toContain('The empty string panics');
    expect(recurring.textContent).toContain('2 runs · correctness-reviewer');
    expect(figure('Rounds per run')).toBe('1.3');
    expect(figure('Clean after the first round')).toBe('33%');
    expect(figure('Repeated blockers')).toBe('1');
    expect(figure('Fixed, as the fixer says')).toBe('2');
    expect(figure('Disputed, then upheld')).toBe('1');
    expect(figure('Disputed, then dropped')).toBe('3');
    expect(figure('Dismissed as repeats')).toBe('4');
  });

  it('reads cost per merged pull request in money when priced and in tokens otherwise, beside the autonomy rate', () => {
    mount({ insights: november, quality: () => quality() });
    expect(screen.getByTestId('cost-per-merged').textContent).toBe('$2.50');
    expect(figure('Autonomy rate')).toBe('25%');
    // The rate is of the ended runs, not of the merged ones.
    expect(
      screen.getByText(
        'What merging cost, and the share of ended runs that merged with no person stepping in'
      )
    ).toBeTruthy();
    expect(figure('Merged pull requests')).toBe('1');
    expect(
      screen.getByText('1 run with a gate a person answered')
    ).toBeTruthy();
    cleanup();

    const read = quality();
    mount({
      insights: november,
      quality: () => ({
        ...read,
        delivery: {
          ...read.delivery,
          costPerMergedUsd: null,
          unpricedCalls: 3,
        },
      }),
    });
    expect(screen.getByText('Tokens per merged pull request')).toBeTruthy();
    expect(screen.getByTestId('cost-per-merged').textContent).toBe('1.5K');
    expect(
      screen.getByText('3 model calls have no price, so cost reads in tokens.')
    ).toBeTruthy();
  });

  it('reads one repository once it is picked', async () => {
    const asked: (string | undefined)[] = [];
    mount({
      insights: november,
      quality: (params) => {
        asked.push(params.repository);
        return quality();
      },
    });

    const picker = screen.getByRole('button', { name: /Repository/ });
    fireEvent.keyDown(picker, { key: 'Enter' });
    fireEvent.keyDown(
      await screen.findByRole('option', { name: 'macro/site' }),
      { key: 'Enter' }
    );

    expect(asked.at(-1)).toBe('https://github.com/macro/site');
  });
});

const APP = 'https://github.com/macro/app';

/** The figure a Metric shows beside its label. */
function figure(label: string) {
  return screen
    .getByText(label)
    .closest('div')
    ?.parentElement?.querySelector('p')?.textContent;
}

function quality(): QualityInsights {
  const link = {
    id: 'run-quality',
    title: 'Collapse repeated dashes',
    createdAt: '2026-11-10T09:00:00Z',
    outcome: 'success' as const,
  };
  const causes = (own: number, setup: number, flaky: number) => ({
    own,
    setup,
    flaky,
    cancelled: 0,
  });
  return {
    from: '2026-10-13',
    to: '2026-11-11',
    timeZone: 'Europe/Lisbon',
    flow: null,
    repository: null,
    repositories: [APP, 'https://github.com/macro/site'],
    runCap: 1000,
    capped: false,
    ci: {
      waits: 4,
      redWaits: 2,
      runs: 2,
      causes: { own: 1, setup: 2, flaky: 2, cancelled: 1 },
      jobs: [
        {
          repository: APP,
          name: 'e2e',
          finished: 1,
          red: 1,
          redRate: 1,
          causes: causes(1, 0, 0),
          pullRequestsFailed: 1,
        },
        {
          repository: APP,
          name: 'build',
          finished: 2,
          red: 2,
          redRate: 1,
          causes: causes(0, 2, 0),
          pullRequestsFailed: 2,
        },
      ],
      firstPushToGreen: { count: 2, medianSeconds: 7200, p90Seconds: 10800 },
      ciFixRounds: 2,
      runsWithCiFixes: 2,
      reviewFixRounds: 1,
      recurring: [
        {
          repository: APP,
          job: 'build',
          step: 'Set up job',
          cause: 'setup',
          pullRequests: 2,
          failures: 2,
          lastFailedAt: '2026-11-02T10:00:00Z',
          runs: [link],
        },
        {
          repository: APP,
          job: 'test',
          step: 'Run tests',
          failure: 'src/net.rs: net::test::retry',
          cause: 'flaky',
          pullRequests: 2,
          failures: 2,
          lastFailedAt: '2026-11-02T10:00:00Z',
          runs: [link],
        },
      ],
    },
    review: {
      reviewedRuns: 3,
      rounds: 4,
      roundsPerRun: 4 / 3,
      firstRounds: 3,
      cleanFirstRounds: 1,
      cleanFirstRate: 1 / 3,
      blockers: 4,
      newBlockers: 3,
      repeatedBlockers: 1,
      resolutions: { fixed: 2, disputedUpheld: 1, disputedDropped: 3, dismissed: 4 },
      byReviewer: [
        { name: 'correctness-reviewer', reviews: 4, blockers: 3 },
        { name: 'scope-reviewer', reviews: 1, blockers: 1 },
        { name: 'style-reviewer', reviews: 10, blockers: 12345 },
      ],
      byAgent: [
        { name: 'backup', reviews: 2, blockers: 2 },
        { name: 'primary', reviews: 3, blockers: 2 },
      ],
      recurring: [
        {
          text: 'The empty string panics',
          runs: 2,
          blockers: 3,
          reviewers: ['correctness-reviewer'],
          lastFoundAt: '2026-11-02T02:00:00Z',
          runLinks: [link],
        },
      ],
    },
    delivery: {
      endedRuns: 4,
      merged: 1,
      autonomous: 1,
      autonomyRate: 0.25,
      personAnswered: 1,
      steered: 1,
      manual: 1,
      measuredRuns: 2,
      tokens: { input: 1500, output: 0, cacheRead: 0, cacheWrite: 0 },
      costUsd: 2.5,
      unpricedCalls: 0,
      costPerMergedUsd: 2.5,
      tokensPerMerged: 1500,
    },
  };
}
