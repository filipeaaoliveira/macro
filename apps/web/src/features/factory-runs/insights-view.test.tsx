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
import { type RunsContext, RunsProvider } from './context/runs-context';
import type { InsightsParams, RunInsights } from './core/insights';
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

const outcomes = (
  changes: Partial<RunInsights['totals']>
): RunInsights['totals'] => ({
  running: 0,
  success: 0,
  manual: 0,
  blocked: 0,
  failed: 0,
  cancelled: 0,
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

function mount(options: {
  state?: Accessor<'pending' | 'error' | 'ready'>;
  insights?: RunInsights;
  retry?: () => void;
  openRunId?: Accessor<string | undefined>;
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
      data: () => [],
      retry: vi.fn(),
    }),
    run: (runId) => ({
      state: () => 'ready',
      data: () => (runId() === failedImport.id ? failedImport : undefined),
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
  };
  render(() => (
    <RunsProvider value={runs}>
      <InsightsProvider value={insights}>
        <RunInsightsView
          timeZone="Europe/Lisbon"
          now={() => new Date('2026-11-11T12:00:00Z')}
          openRunId={options.openRunId?.()}
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

  it('opens the run a link names, and another link’s run in the same view', async () => {
    const [runId, setRunId] = createSignal<string | undefined>();
    mount({ insights: november, openRunId: runId });
    expect(screen.queryByText('No steps yet')).toBeNull();

    setRunId(failedImport.id);

    expect(await screen.findByText('No steps yet')).toBeTruthy();
  });
});
