import { afterEach, expect, test } from 'bun:test';
import { Macro } from '../src/macro';
import { MacroApiError } from '../src/utils/unwrap';

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
});

const RUN = '01960000-0000-7000-8000-000000000001';

function run(steps: unknown[], end: unknown = null) {
  return {
    run: {
      id: RUN,
      request: 'Plan the export',
      createdAt: '2026-11-18T09:00:00Z',
      createdInPerson: false,
      userId: 'macro|alice@example.com',
      end,
    },
    steps,
    reviewRounds: [],
  };
}

function step(seq: number, gate: unknown) {
  return {
    seq,
    entry: 'plan',
    visit: 1,
    startedAt: '2026-11-18T09:00:00Z',
    gate,
  };
}

const answered = {
  kind: 'flow',
  question: 'Approve the plan?',
  answers: ['approve', 'revise'],
  openedAt: '2026-11-18T09:01:00Z',
  answer: { answer: 'revise', answeredAt: '2026-11-18T09:02:00Z' },
};
const open = {
  kind: 'flow',
  question: 'Approve the plan?',
  answers: ['approve', 'revise'],
  openedAt: '2026-11-18T09:03:00Z',
};

function serve(detail: unknown, status = 202) {
  const requests: Request[] = [];
  globalThis.fetch = Object.assign(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      requests.push(request);
      if (request.method === 'GET') return Response.json(detail);
      return new Response(
        status === 202
          ? null
          : JSON.stringify({ message: 'refused', code: 'reservedForAPerson' }),
        { status },
      );
    },
    { preconnect: originalFetch.preconnect },
  );
  return requests;
}

const macro = () =>
  new Macro({
    auth: { type: 'user', apiKey: 'mak_test' },
    hosts: { factory: 'https://factory.test' },
  });

test('a run answers the gate the agent decided on, by its step', async () => {
  const requests = serve(run([step(2, answered), step(4, open)]));
  const factoryRun = macro().factory.byId(RUN);

  const waiting = await factoryRun.waitingGate();
  if (!waiting) throw new Error('the run waits on a gate');
  await factoryRun.answer(waiting, { answer: 'revise', note: 'Go' });

  const answer = requests.at(-1);
  expect(answer?.method).toBe('POST');
  expect(answer?.url).toBe(`https://factory.test/runs/${RUN}/steps/4/answer`);
  expect(await answer?.json()).toEqual({ answer: 'revise', note: 'Go' });
});

test('a run that waits on no gate has none to answer', async () => {
  serve(run([step(2, answered)]));

  expect(await macro().factory.byId(RUN).waitingGate()).toBeUndefined();
});

test('a gate answered since the agent read it is refused', async () => {
  const detail = run([step(4, open)]);
  serve(detail, 409);
  const factoryRun = macro().factory.byId(RUN);
  const waiting = await factoryRun.waitingGate();
  if (!waiting) throw new Error('the run waits on a gate');

  // The person answered meanwhile: the factory refuses the stale answer.
  await expect(
    factoryRun.answer(waiting, { answer: 'approve' }),
  ).rejects.toThrow();
});

test('an answer the factory keeps for the person is refused', async () => {
  serve(run([step(4, open)]), 403);
  const factoryRun = macro().factory.byId(RUN);
  const waiting = await factoryRun.waitingGate();
  if (!waiting) throw new Error('the run waits on a gate');

  await expect(
    factoryRun.answer(waiting, { answer: 'approve' }),
  ).rejects.toThrow();
});

test('the queue and the runs yet to ask anything are read apart', async () => {
  const STARTING = '01960000-0000-7000-8000-000000000002';
  const requests = serve({
    runs: [run([step(1, open)])],
    starting: [{ ...run([]), run: { ...run([]).run, id: STARTING } }],
  });

  const queue = await macro().factory.queue();
  const starting = await macro().factory.starting();

  expect(queue.map((waiting) => waiting.id)).toEqual([RUN]);
  expect(starting.map((held) => held.id)).toEqual([STARTING]);
  expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
    '/runs/waiting',
    '/runs/waiting',
  ]);
});

test('a retry starts a new run and hands it back', async () => {
  const RETRY = '01960000-0000-7000-8000-000000000003';
  const requests: Request[] = [];
  globalThis.fetch = Object.assign(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      requests.push(request);
      const retried = run([], { outcome: 'infrastructure' });
      return Response.json(
        {
          ...retried,
          run: {
            ...retried.run,
            id: RETRY,
            end: null,
            retry: { retriedFrom: RUN, branchOf: RUN, firstSeq: 3 },
          },
        },
        { status: 201 },
      );
    },
    { preconnect: originalFetch.preconnect },
  );

  const retry = await macro().factory.byId(RUN).retry();

  expect(retry.id).toBe(RETRY);
  expect(requests.map((request) => request.method)).toEqual(['POST']);
  expect(requests[0]?.url).toBe(`https://factory.test/runs/${RUN}/retry`);
});

test('a run retried already is refused', async () => {
  globalThis.fetch = Object.assign(
    async () =>
      Response.json(
        { message: 'the run was retried already' },
        { status: 409 },
      ),
    { preconnect: originalFetch.preconnect },
  );

  await expect(macro().factory.byId(RUN).retry()).rejects.toThrow();
});

test('a retry past the queue limit is refused with its code', async () => {
  globalThis.fetch = Object.assign(
    async () =>
      Response.json(
        { message: 'too many runs wait on you', code: 'queueFull' },
        { status: 409 },
      ),
    { preconnect: originalFetch.preconnect },
  );

  const refused = await macro()
    .factory.byId(RUN)
    .retry()
    .then(
      () => undefined,
      (error: unknown) => error,
    );

  expect(refused).toBeInstanceOf(MacroApiError);
  if (!(refused instanceof MacroApiError)) throw new Error('not refused');
  expect(refused.status).toBe(409);
  expect(refused.data).toEqual({
    message: 'too many runs wait on you',
    code: 'queueFull',
  });
});
