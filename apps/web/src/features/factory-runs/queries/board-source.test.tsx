/** @vitest-environment jsdom */
import { factoryClient } from '@service-factory/client';
import type { BoardPage } from '@service-factory/generated/schemas';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { For } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { BoardSource } from '../context/board-context';
import { createAppBoardContext } from './board';

vi.mock('@service-factory/client', () => ({
  factoryClient: { listBoard: vi.fn() },
}));
vi.mock('@core/context/user', () => ({
  useUserId: () => () => 'macro|julia@example.com',
}));

function page(steps: number): BoardPage {
  return {
    stoppedEarly: false,
    runs: [
      {
        state: 'running',
        detail: {
          run: {
            id: 'run-1',
            request: 'Rename the column',
            createdAt: '2026-11-18T09:00:00Z',
            userId: 'macro|julia@example.com',
            createdInPerson: true,
          },
          steps: Array.from({ length: steps }, (_, index) => ({
            seq: index + 1,
            entry: 'write',
            visit: 1,
            startedAt: '2026-11-18T09:00:00Z',
            checks: [],
            passedOver: [],
            terminal: { betweenTurns: false, steers: [] },
          })),
          reviewRounds: [],
        },
      },
    ],
  };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('the board source', () => {
  it('keeps a row’s node when a read again changes its Run', async () => {
    vi.mocked(factoryClient.listBoard)
      .mockResolvedValueOnce(ok(page(1)))
      .mockResolvedValue(ok(page(2)));
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    let source!: BoardSource;
    function Probe() {
      source = createAppBoardContext(client).board(() => ({ scope: 'mine' }));
      return (
        <ul>
          <For each={source.data()}>
            {(listed) => (
              <li data-run={listed.id}>{listed.run.steps.length} steps</li>
            )}
          </For>
        </ul>
      );
    }
    render(() => (
      <QueryClientProvider client={client}>
        <Probe />
      </QueryClientProvider>
    ));

    const row = await screen.findByText('1 steps');
    source.retry();
    await waitFor(() => expect(screen.getByText('2 steps')).toBeTruthy());

    expect(screen.getByText('2 steps')).toBe(row);
  });
});
