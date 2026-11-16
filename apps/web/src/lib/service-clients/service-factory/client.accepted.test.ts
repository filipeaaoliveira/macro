import { afterEach, describe, expect, it, vi } from 'vitest';
import { factoryClient } from './client';

vi.mock('@core/constant/servers', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@core/constant/servers')>()),
  SERVER_HOSTS: { factory: 'https://factory.example.com' },
}));

afterEach(() => {
  vi.unstubAllGlobals();
});

// The service accepts a Gate's answer and a cancel with 202 and no body.
describe('factoryClient on a 202 with no body', () => {
  it('takes an accepted answer and cancel as done', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(null, { status: 202 }))
    );

    const answered = await factoryClient.answerGate('run-1', 3, {
      answer: 'approve',
    });
    const cancelled = await factoryClient.cancelRun('run-1');

    expect(answered.isOk()).toBe(true);
    expect(cancelled.isOk()).toBe(true);
  });
});
