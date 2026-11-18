/** @vitest-environment jsdom */
import { factoryClient } from '@service-factory/client';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StepSessionComposerGate } from './step-session-gate';

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({ enabled: true }),
}));
vi.mock('@service-factory/client', () => ({
  factoryClient: { getSessionStep: vi.fn() },
}));

function mount(sessionId: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(() => (
    <QueryClientProvider client={client}>
      <StepSessionComposerGate sessionId={sessionId}>
        <textarea aria-label="Composer" />
      </StepSessionComposerGate>
    </QueryClientProvider>
  ));
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('StepSessionComposerGate', () => {
  it('sends the person to the Run of a factory Step’s session instead of its composer', async () => {
    vi.mocked(factoryClient.getSessionStep).mockResolvedValue(
      ok({ runId: 'run-1', seq: 2 })
    );
    mount('session-2');

    const open = await screen.findByRole('link', { name: 'Open the run' });
    expect(open.getAttribute('href')).toBe('/app/factory/runs/run-1');
    expect(screen.queryByRole('textbox', { name: 'Composer' })).toBeNull();
    expect(factoryClient.getSessionStep).toHaveBeenCalledWith(
      'session-2',
      expect.anything()
    );
  });

  it('keeps the composer of any other session', async () => {
    vi.mocked(factoryClient.getSessionStep).mockResolvedValue(
      err([{ code: 'NOT_FOUND', message: 'not found' }])
    );
    mount('session-9');

    await waitFor(() =>
      expect(factoryClient.getSessionStep).toHaveBeenCalled()
    );
    expect(screen.getByRole('textbox', { name: 'Composer' })).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Open the run' })).toBeNull();
  });
});
