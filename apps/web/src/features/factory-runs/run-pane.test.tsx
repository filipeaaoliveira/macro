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

function mount(viewerId: Accessor<string | undefined>) {
  const context: RunsContext = {
    viewerId,
    taskRuns: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [], starting: [] }),
      retry: vi.fn(),
    }),
    run: () => ({ state: () => 'ready', data: () => run, retry: vi.fn() }),
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
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={context}>
      <RunPageView runId={run.id} />
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

  it('shows no owner’s control while the viewer is not known yet', () => {
    const [viewer, setViewer] = createSignal<string>();
    mount(viewer);

    for (const control of ownerControls()) expect(control).toBeNull();
    setViewer(OWNER);
    for (const control of ownerControls()) expect(control).toBeTruthy();
  });
});
