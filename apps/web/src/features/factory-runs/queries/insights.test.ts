import { factoryClient } from '@service-factory/client';
import { QueryClient } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createAppInsightsContext } from './insights';
import { factoryRunKeys } from './keys';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    undoImprovementDismissal: vi.fn(),
  },
}));

const params = {
  from: '2026-11-01',
  to: '2026-11-30',
  timeZone: 'UTC',
  flow: undefined,
};

/** A client holding one evaluation read, and whether it was invalidated. */
function seeded() {
  const client = new QueryClient();
  const key = factoryRunKeys.evaluation(params).queryKey;
  client.setQueryData(key, { scorers: [] });
  const stale = () => client.getQueryState(key)?.isInvalidated ?? false;
  return { client, stale };
}

afterEach(() => {
  vi.clearAllMocks();
});

describe('undoDismissal', () => {
  it('undoes the dismissal of the named improvement and reads the evaluation again', async () => {
    vi.mocked(factoryClient.undoImprovementDismissal).mockResolvedValue(
      ok({ restored: 3, undoneAt: '2026-11-20T09:00:00Z' })
    );
    const { client, stale } = seeded();

    const result =
      await createAppInsightsContext(client).undoDismissal('improvement-1');

    expect(result).toEqual({ ok: true, value: undefined });
    expect(factoryClient.undoImprovementDismissal).toHaveBeenCalledWith(
      'improvement-1'
    );
    expect(stale()).toBe(true);
  });

  it('says conflict when the improvement dismissed nothing, and reads again', async () => {
    vi.mocked(factoryClient.undoImprovementDismissal).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'nothing dismissed' }])
    );
    const { client, stale } = seeded();

    const result =
      await createAppInsightsContext(client).undoDismissal('improvement-1');

    expect(result).toEqual({ ok: false, reason: 'conflict' });
    expect(stale()).toBe(true);
  });

  it('says forbidden when the person may no longer undo it, and reads again', async () => {
    vi.mocked(factoryClient.undoImprovementDismissal).mockResolvedValue(
      err([{ code: 'FORBIDDEN', message: 'not yours' }])
    );
    const { client, stale } = seeded();

    const result =
      await createAppInsightsContext(client).undoDismissal('improvement-1');

    expect(result).toEqual({ ok: false, reason: 'forbidden' });
    expect(stale()).toBe(true);
  });

  it('says failed when the undo fails otherwise', async () => {
    vi.mocked(factoryClient.undoImprovementDismissal).mockResolvedValue(
      err([{ code: 'INTERNAL_SERVER_ERROR', message: 'down' }])
    );
    const { client } = seeded();

    const result =
      await createAppInsightsContext(client).undoDismissal('improvement-1');

    expect(result).toEqual({ ok: false, reason: 'failed' });
  });
});
