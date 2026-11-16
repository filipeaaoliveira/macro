import { fetchWithToken } from '@core/util/fetchWithToken';
import { ok } from 'neverthrow';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { factoryClient } from './client';

vi.mock('@core/constant/servers', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@core/constant/servers')>()),
  SERVER_HOSTS: { factory: 'https://factory.example.com' },
}));
vi.mock('@core/util/fetchWithToken', () => ({ fetchWithToken: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(fetchWithToken).mockResolvedValue(ok({ runs: [], flows: [] }));
});

function lastCall() {
  const call = vi.mocked(fetchWithToken).mock.calls.at(-1);
  if (!call) throw new Error('No request was sent');
  const [url, init] = call;
  return {
    url,
    method: init?.method,
    body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
  };
}

// The routes the web app needs from the factory service. When the service
// publishes them, its generated types replace `pending-routes.ts`, and this
// keeps the switch honest.
describe('factoryClient routes', () => {
  it('reads a task’s Runs and the Flows', async () => {
    await factoryClient.listTaskRuns('task 1');
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/tasks/task%201/runs',
      method: 'GET',
      body: undefined,
    });

    await factoryClient.listFlows();
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/flows',
      method: 'GET',
      body: undefined,
    });

    await factoryClient.getRun('run-1');
    expect(lastCall().url).toBe('https://factory.example.com/runs/run-1');
  });

  it('starts a Run from a Flow’s name on a task', async () => {
    await factoryClient.startRun({
      flow: 'plan',
      request: 'Add a due date column',
      taskId: 'task-1',
    });
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs',
      method: 'POST',
      body: {
        flow: 'plan',
        request: 'Add a due date column',
        taskId: 'task-1',
      },
    });
  });

  it('answers a Gate without naming the person, and cancels', async () => {
    await factoryClient.answerGate('run-1', 'ci-fix', 2, {
      answer: 'approve',
      note: 'Keep it small',
    });
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/gates/ci-fix/2/answer',
      method: 'POST',
      body: { answer: 'approve', note: 'Keep it small' },
    });

    await factoryClient.cancelRun('run-1');
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/cancel',
      method: 'POST',
      body: undefined,
    });
  });
});
