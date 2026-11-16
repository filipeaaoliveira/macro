import { factoryClient } from '@service-factory/client';
import type { RunDetail } from '@service-factory/generated/schemas';
import { QueryClient, QueryObserver } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Run } from '../core/run';
import { factoryRunKeys } from './keys';
import { createAppRunsContext, runOptions, taskRunsOptions } from './runs';

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

  it('stops reading a Run again once it has ended', async () => {
    vi.useFakeTimers();
    try {
      const running: RunDetail = { ...started };
      const ended: RunDetail = {
        ...started,
        run: {
          ...started.run,
          end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
        },
      };
      vi.mocked(factoryClient.getRun)
        .mockResolvedValueOnce(ok(running))
        .mockResolvedValue(ok(ended));
      const client = new QueryClient();
      const observer = new QueryObserver(client, runOptions('run-2'));
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(3_000);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops reading a task once its Runs end, and starts again for a new one', async () => {
    vi.useFakeTimers();
    try {
      const ended: RunDetail = {
        ...started,
        run: {
          ...started.run,
          end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
        },
      };
      vi.mocked(factoryClient.listTaskRuns).mockResolvedValue(ok([ended]));
      const client = new QueryClient();
      const observer = new QueryObserver(
        client,
        taskRunsOptions(client, 'task-1')
      );
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(120_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(1);

      vi.mocked(factoryClient.startRun).mockResolvedValue(ok(started));
      await createAppRunsContext(client).startRun({
        taskId: 'task-1',
        flow: 'plan',
        request: 'Add a due date column',
      });
      await vi.advanceTimersByTimeAsync(3_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reads a task whose live Runs all wait on a Gate only slowly', async () => {
    vi.useFakeTimers();
    try {
      const waiting: RunDetail = {
        ...started,
        steps: [
          {
            entry: 'plan',
            visit: 1,
            startedAt: '2026-11-16T09:00:01Z',
            end: {
              outcome: 'done',
              output: 'A plan.',
              endedAt: '2026-11-16T09:01:00Z',
            },
            gate: {
              question: 'Approve this plan?',
              answers: ['approve'],
              openedAt: '2026-11-16T09:01:00Z',
              answer: null,
            },
          },
        ],
      };
      vi.mocked(factoryClient.listTaskRuns).mockResolvedValue(ok([waiting]));
      const client = new QueryClient();
      const observer = new QueryObserver(
        client,
        taskRunsOptions(client, 'task-1')
      );
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reads the Run again before reporting a refused answer', async () => {
    const client = new QueryClient();
    const observer = new QueryObserver(client, runOptions('run-2'));
    const unsubscribe = observer.subscribe(() => {});
    vi.mocked(factoryClient.getRun).mockResolvedValue(ok(started));
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().isSuccess).toBe(true)
    );
    const readsBefore = vi.mocked(factoryClient.getRun).mock.calls.length;
    vi.mocked(factoryClient.answerGate).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );

    const result = await createAppRunsContext(client).answerGate({
      runId: 'run-2',
      step: { entry: 'plan', visit: 1 },
      answer: 'approve',
    });

    expect(result).toEqual({ ok: false, reason: 'conflict' });
    expect(vi.mocked(factoryClient.getRun).mock.calls.length).toBe(
      readsBefore + 1
    );
    expect(observer.getCurrentResult().isFetching).toBe(false);
    unsubscribe();
  });
});
