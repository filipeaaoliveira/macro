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

// The person routes the web app calls on the factory service, built from
// its generated client's URLs.
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

  it('reads a Run’s scores and labels one by its Scorer', async () => {
    await factoryClient.getRunScores('run 1');
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run%201/scores',
      method: 'GET',
      body: undefined,
    });

    await factoryClient.labelRunScore('run-1', 'clear docs', false);
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/scores/clear%20docs/label',
      method: 'PUT',
      body: { passed: false },
    });
  });

  it('proposes a Definition change and names the cause of a refusal', async () => {
    const request = {
      changeId: '00000000-0000-4000-8000-000000000001',
      title: 'Add a docs flow',
      description: null,
      files: { 'agents.json': '{}' },
    };
    await factoryClient.proposeDefinitionChange(request);
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/settings/definition/changes',
      method: 'POST',
      body: request,
    });

    const handler = vi
      .mocked(fetchWithToken)
      .mock.calls.at(-1)?.[1]?.errorResponseHandler;
    if (!handler) throw new Error('No error response handler');
    const refused = (status: number, body: unknown) =>
      handler(new Response(JSON.stringify(body), { status }));

    expect(
      await refused(400, {
        code: 'invalidDefinitionChange',
        message: 'flows/docs.json: unknown entry',
      })
    ).toEqual({
      code: 'INVALID_DEFINITION_CHANGE',
      message: 'flows/docs.json: unknown entry',
    });
    expect(
      await refused(409, { code: 'definitionChangeClosed', message: 'closed' })
    ).toEqual({ code: 'DEFINITION_CHANGE_CLOSED', message: 'closed' });
    expect(
      await refused(409, { code: 'noDefinition', message: 'none' })
    ).toEqual({ code: 'NO_DEFINITION', message: 'none' });
    expect(
      await refused(409, {
        code: 'definitionChangeNotOpened',
        message: 'a rule',
      })
    ).toEqual({ code: 'DEFINITION_CHANGE_NOT_OPENED', message: 'a rule' });
    expect(
      await refused(400, {
        code: 'unreachableDefinition',
        message: 'no access',
      })
    ).toEqual({ code: 'DEFINITION_UNREACHABLE', message: 'no access' });
    expect(
      await refused(400, { code: 'invalidDefinition', message: 'moved' })
    ).toEqual({ code: 'DEFINITION_INVALID', message: 'moved' });
    // A refusal with no code the change names keeps the status's, and the
    // service's message.
    expect(await refused(403, { message: 'admins only' })).toEqual({
      code: 'FORBIDDEN',
      message: 'admins only',
    });
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

  it('names the repository cause of a refused start, else the status', async () => {
    await factoryClient.startRun({
      flow: 'code',
      request: 'Add a due date column',
      repoUrl: 'https://github.com/macro-inc/app',
    });
    const handler = vi
      .mocked(fetchWithToken)
      .mock.calls.at(-1)?.[1]?.errorResponseHandler;
    if (!handler) throw new Error('No error response handler');
    const refused = (body: unknown) =>
      handler(new Response(JSON.stringify(body), { status: 400 }));

    expect(
      await refused({ message: 'cannot push', code: 'noPushAccess' })
    ).toEqual({ code: 'NO_PUSH_ACCESS', message: 'cannot push' });
    expect(
      await refused({ message: 'cannot reach', code: 'unreachableRepository' })
    ).toEqual({ code: 'UNREACHABLE_REPOSITORY', message: 'cannot reach' });
    expect(
      await handler(
        new Response(
          JSON.stringify({ message: 'too many wait', code: 'queueFull' }),
          { status: 409 }
        )
      )
    ).toEqual({ code: 'QUEUE_FULL', message: 'too many wait' });
    expect(await refused({ message: 'the request must not be empty' })).toEqual(
      { code: 'HTTP_ERROR', message: 'HTTP error! status: 400' }
    );
    expect(
      await handler(new Response('Bad gateway', { status: 502 }))
    ).toMatchObject({ code: 'HTTP_ERROR' });
  });

  it('answers a Gate without naming the person, and cancels', async () => {
    await factoryClient.answerGate('run-1', 3, {
      answer: 'approve',
      note: 'Keep it small',
    });
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/steps/3/answer',
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

  it('steers a running Step from its Step Terminal', async () => {
    await factoryClient.noteStep('run-1', 2, { note: 'Use the tokens' });
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/steps/2/note',
      method: 'POST',
      body: { note: 'Use the tokens' },
    });

    await factoryClient.pauseStep('run-1', 2);
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/steps/2/pause',
      method: 'POST',
      body: undefined,
    });

    await factoryClient.resumeStep('run-1', 2, {});
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/steps/2/resume',
      method: 'POST',
      body: {},
    });

    await factoryClient.stopStep('run-1', 2);
    expect(lastCall()).toEqual({
      url: 'https://factory.example.com/runs/run-1/steps/2/stop',
      method: 'POST',
      body: undefined,
    });
  });
});
