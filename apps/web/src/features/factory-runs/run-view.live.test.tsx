/** @vitest-environment jsdom */
import { factoryClient } from '@service-factory/client';
import type { RunDetail } from '@service-factory/generated/schemas';
import { cleanup, render, screen } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { RunsProvider } from './context/runs-context';
import { createAppRunsContext } from './queries/runs';
import { RunView } from './run-view';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    listTaskRuns: vi.fn(),
    getRun: vi.fn(),
    listFlows: vi.fn(),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    cancelRun: vi.fn(),
  },
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
  );
});

const planning: RunDetail = {
  run: {
    id: 'run-1',
    request: 'Add a due date column',
    createdAt: '2026-11-16T09:00:00Z',
    taskId: 'task-1',
    userId: 'macro|owner@example.com',
    end: null,
  },
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:00Z',
      passedOver: [],
      end: null,
      gate: null,
      checks: [],
    },
  ],
};

const waiting: RunDetail = {
  ...planning,
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:00Z',
      passedOver: [],
      end: {
        outcome: 'done',
        output: 'Sort the list by its due date.',
        endedAt: '2026-11-16T09:01:00Z',
      },
      gate: {
        question: 'Approve this plan?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-16T09:01:00Z',
        answer: null,
      },
      checks: [],
    },
  ],
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe('RunView on the factory', () => {
  it('opens the plan a Gate asks about when its Step ends while open', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.mocked(factoryClient.getRun)
      .mockResolvedValueOnce(ok(planning))
      .mockResolvedValue(ok(waiting));
    const client = new QueryClient();
    render(() => (
      <QueryClientProvider client={client}>
        <RunsProvider value={createAppRunsContext(client)}>
          <RunView runId="run-1" onClose={() => {}} />
        </RunsProvider>
      </QueryClientProvider>
    ));

    expect(await screen.findByText('Running…')).toBeTruthy();
    await vi.advanceTimersByTimeAsync(3_000);

    expect(await screen.findByText('Approve this plan?')).toBeTruthy();
    expect(screen.getByText('Sort the list by its due date.')).toBeTruthy();
  });
});
