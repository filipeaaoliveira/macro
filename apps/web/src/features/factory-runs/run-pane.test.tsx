/** @vitest-environment jsdom */
import { cleanup, render, screen } from '@solidjs/testing-library';
import { type Accessor, createSignal } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { type RunsContext, RunsProvider } from './context/runs-context';
import type { Run } from './core/run';
import { RunPageView } from './run-view';

vi.mock('@core/mobile/isTouchDevice', () => ({
  isTouchDevice: () => false,
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

// The shared collapse animation measures with ResizeObserver, which jsdom lacks.
vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
);

const OWNER = 'macro|teo@example.com';

/** A Run with a Step its agent works on and a Gate waiting after it. */
const run: Run = {
  id: 'run-1',
  request: 'Plan the export',
  createdAt: '2026-11-18T09:00:00Z',
  ownerId: OWNER,
  steps: [
    {
      seq: 1,
      entry: 'write',
      visit: 1,
      startedAt: '2026-11-18T09:00:00Z',
      role: 'writer',
      terminal: { sessionId: 'session-1', steers: [] },
    },
    {
      seq: 2,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-18T09:01:00Z',
      gate: {
        kind: 'flow',
        question: 'Approve the plan?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-18T09:05:00Z',
      },
    },
  ],
};

function mount(viewerId: Accessor<string | undefined>, shown: Run = run) {
  const context: RunsContext = {
    viewerId,
    taskRuns: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [], starting: [] }),
      retry: vi.fn(),
    }),
    run: () => ({ state: () => 'ready', data: () => shown, retry: vi.fn() }),
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
    retryRun: vi.fn(),
    markRunFlawed: vi.fn(),
    runScores: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    labelRunScore: vi.fn(),
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={context}>
      <RunPageView runId={shown.id} />
    </RunsProvider>
  ));
}

/** The owner's controls on the Run: its Gate's answers, Cancel run, Watch. */
function ownerControls() {
  return [
    screen.queryByRole('button', { name: 'Approve' }),
    screen.queryByRole('button', { name: 'Cancel run' }),
    screen.queryByRole('button', { name: /^Watch/ }),
  ];
}

afterEach(() => {
  cleanup();
});

describe('RunPageView', () => {
  it('gives its owner the Gate’s answers, Cancel run and the Step Terminal', () => {
    mount(() => OWNER);

    for (const control of ownerControls()) expect(control).toBeTruthy();
  });

  it('shows a team member the Run without its owner’s controls', () => {
    mount(() => 'macro|julia@example.com');

    expect(screen.getByText('Plan the export')).toBeTruthy();
    for (const control of ownerControls()) expect(control).toBeNull();
  });

  it('shows where each Step ran, in Working now and in the Steps, once known', () => {
    const [write, plan] = run.steps;
    if (!write || !plan) throw new Error('No Steps');
    mount(() => OWNER, {
      ...run,
      steps: [
        {
          ...plan,
          seq: 1,
          gate: undefined,
          role: 'planner',
          agent: 'primary',
          end: {
            outcome: 'done',
            output: 'The plan',
            endedAt: '2026-11-18T09:00:30Z',
          },
          runtime: {
            model: 'claude-opus-4-5',
            machine: 'm4',
            agentName: 'Planner bot',
          },
        },
        {
          ...write,
          seq: 2,
          agent: 'primary',
          runtime: {
            model: 'claude-sonnet-4-5',
            machine: 'm3',
            agentName: 'Writer bot',
          },
        },
        // A Step from before Steps kept where they ran: no chips.
        { ...write, seq: 3, agent: 'backup', terminal: undefined },
      ],
    });

    // The working Step shows in Working now and in the Steps.
    expect(screen.getAllByTitle('Model claude-sonnet-4-5')).toHaveLength(2);
    expect(screen.getAllByTitle('Machine m3')).toHaveLength(2);
    expect(screen.getAllByTitle('Agent Writer bot')).toHaveLength(2);
    // The ended one only in the Steps.
    expect(screen.getAllByTitle('Model claude-opus-4-5')).toHaveLength(1);
    expect(screen.getAllByTitle('Machine m4')).toHaveLength(1);
    expect(screen.getAllByTitle('Agent Planner bot')).toHaveLength(1);
    expect(screen.queryAllByTitle(/^Model /)).toHaveLength(3);
  });

  it('shows no owner’s control while the viewer is not known yet', () => {
    const [viewer, setViewer] = createSignal<string>();
    mount(viewer);

    for (const control of ownerControls()) expect(control).toBeNull();
    setViewer(OWNER);
    for (const control of ownerControls()) expect(control).toBeTruthy();
  });
});
