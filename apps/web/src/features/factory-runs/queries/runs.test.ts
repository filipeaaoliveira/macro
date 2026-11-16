import { factoryClient } from '@service-factory/client';
import type { RunDetail } from '@service-factory/generated/schemas';
import { QueryClient } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Run } from '../core/run';
import { factoryRunKeys } from './keys';
import { createAppRunsContext } from './runs';

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

const started: RunDetail = {
  run: {
    id: 'run-2',
    request: 'Add a due date column',
    createdAt: '2026-11-16T09:00:00Z',
    taskId: 'task-1',
    end: null,
  },
  steps: [
    {
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:01Z',
      end: null,
      gate: null,
    },
  ],
};

const older: Run = {
  id: 'run-1',
  request: 'Rename the column',
  createdAt: '2026-11-15T09:00:00Z',
  taskId: 'task-1',
  end: { outcome: 'success', endedAt: '2026-11-15T10:00:00Z' },
  steps: [],
};

afterEach(() => {
  vi.clearAllMocks();
});

describe('createAppRunsContext', () => {
  it('puts a started Run first in its task, without the wire nulls', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.task('task-1').queryKey, [older]);
    vi.mocked(factoryClient.startRun).mockResolvedValue(ok(started));

    const result = await createAppRunsContext(client).startRun({
      taskId: 'task-1',
      flow: 'plan',
      request: 'Add a due date column',
    });

    const run: Run = {
      id: 'run-2',
      request: 'Add a due date column',
      createdAt: '2026-11-16T09:00:00Z',
      taskId: 'task-1',
      end: undefined,
      steps: [
        {
          entry: 'plan',
          visit: 1,
          startedAt: '2026-11-16T09:00:01Z',
          end: undefined,
          gate: undefined,
        },
      ],
    };
    expect(result).toEqual({ ok: true, value: run });
    expect(client.getQueryData(factoryRunKeys.task('task-1').queryKey)).toEqual(
      [run, older]
    );
    expect(client.getQueryData(factoryRunKeys.run('run-2').queryKey)).toEqual(
      run
    );
  });

  it('tells a refused answer apart from a failed one', async () => {
    const context = createAppRunsContext(new QueryClient());
    const input = {
      runId: 'run-2',
      step: { entry: 'plan', visit: 1 },
      answer: 'approve',
    };

    vi.mocked(factoryClient.answerGate).mockResolvedValueOnce(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );
    expect(await context.answerGate(input)).toEqual({
      ok: false,
      reason: 'conflict',
    });

    vi.mocked(factoryClient.answerGate).mockResolvedValueOnce(
      err([{ code: 'SERVER_ERROR', message: 'Internal server error' }])
    );
    expect(await context.answerGate(input)).toEqual({
      ok: false,
      reason: 'failed',
    });
  });

  it('reads the Run and its task again after a cancel the server refused', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.run('run-2').queryKey, {
      ...older,
      id: 'run-2',
      end: undefined,
    });
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    vi.mocked(factoryClient.cancelRun).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );

    expect(await createAppRunsContext(client).cancelRun('run-2')).toEqual({
      ok: false,
      reason: 'conflict',
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.run('run-2').queryKey,
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.task('task-1').queryKey,
    });
  });
});
