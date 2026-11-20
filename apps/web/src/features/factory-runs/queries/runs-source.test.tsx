/** @vitest-environment jsdom */
import { factoryClient } from '@service-factory/client';
import type { RunDetail } from '@service-factory/generated/schemas';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RunsSource } from '../context/runs-context';
import type { Run } from '../core/run';
import { createAppRunsContext } from './runs';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    listTaskRuns: vi.fn(),
    getRun: vi.fn(),
    listFlows: vi.fn(),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    cancelRun: vi.fn(),
    markRunFlawed: vi.fn(),
    steerStep: vi.fn(),
  },
}));

const ended: RunDetail = {
  run: {
    id: 'run-1',
    request: 'Rename the column',
    createdAt: '2026-11-16T09:00:00Z',
    taskId: 'task-1',
    userId: 'macro|owner@example.com',
    createdInPerson: true,
    end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
  },
  steps: [],
  reviewRounds: [],
  standingActs: [],
  agentRules: [],
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('a Run source', () => {
  it('keeps the Run it read when a later read fails', async () => {
    vi.mocked(factoryClient.getRun)
      .mockResolvedValueOnce(ok(ended))
      .mockResolvedValue(
        err([{ code: 'SERVER_ERROR', message: 'Internal server error' }])
      );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    let source!: RunsSource<Run>;
    function Probe() {
      source = createAppRunsContext(client).run(() => 'run-1');
      return (
        <output>
          {source.state()}:{source.data()?.request}
        </output>
      );
    }
    render(() => (
      <QueryClientProvider client={client}>
        <Probe />
      </QueryClientProvider>
    ));

    expect(await screen.findByText('ready:Rename the column')).toBeTruthy();
    source.retry();
    await waitFor(() => expect(factoryClient.getRun).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        client.getQueryState(['factoryRuns', 'run', 'run-1'])?.status
      ).toBe('error')
    );

    expect(screen.getByText('ready:Rename the column')).toBeTruthy();
  });
});
