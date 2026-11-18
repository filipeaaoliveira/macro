import { afterEach, expect, test } from 'bun:test';
import { Macro } from '../src/macro';

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

test('a run answers the gate it waits on, by its step', async () => {
  const requests = serve(run([step(2, answered), step(4, open)]));

  await macro().factory.byId(RUN).answer({ answer: 'approve', note: 'Go' });

  const answer = requests.at(-1);
  expect(answer?.method).toBe('POST');
  expect(answer?.url).toBe(`https://factory.test/runs/${RUN}/steps/4/answer`);
  expect(await answer?.json()).toEqual({ answer: 'approve', note: 'Go' });
});

test('a run that waits on no gate answers nothing', async () => {
  const requests = serve(run([step(2, answered)]));

  await expect(
    macro().factory.byId(RUN).answer({ answer: 'approve' }),
  ).rejects.toThrow('waits on no gate');
  expect(requests.every((request) => request.method === 'GET')).toBe(true);
});

test('an answer the factory keeps for the person is refused', async () => {
  serve(run([step(4, open)]), 403);

  await expect(
    macro().factory.byId(RUN).answer({ answer: 'approve' }),
  ).rejects.toThrow();
});
