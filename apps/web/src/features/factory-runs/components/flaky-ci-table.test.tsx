/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { QualityInsights } from '../core/quality';
import { FlakyCiTable } from './insights-overview';

const APP = 'https://github.com/macro/app';

const causes = { own: 0, setup: 0, flaky: 1, cancelled: 0 };

const CI: QualityInsights['ci'] = {
  waits: 4,
  redWaits: 2,
  runs: 2,
  causes,
  jobs: [
    {
      repository: APP,
      name: 'lint',
      finished: 4,
      red: 1,
      redRate: 0.25,
      causes,
      pullRequestsFailed: 1,
    },
  ],
  ciFixRounds: 0,
  runsWithCiFixes: 0,
  reviewFixRounds: 0,
  recurring: [],
  retries: { passed: 0, failed: 0, refused: 0 },
  quarantined: [
    {
      repository: APP,
      job: 'e2e',
      passedRetries: 3,
      quarantinedAt: '2026-11-20T10:00:00Z',
      lastPassedAt: '2026-11-21T10:00:00Z',
    },
  ],
};

afterEach(() => {
  cleanup();
});

describe('FlakyCiTable', () => {
  it('quarantines an active job and releases a quarantined one', async () => {
    const onQuarantine = vi.fn(async () => {});
    render(() => <FlakyCiTable ci={CI} onQuarantine={onQuarantine} />);

    fireEvent.click(screen.getByRole('button', { name: 'Quarantine' }));
    await waitFor(() =>
      expect(onQuarantine).toHaveBeenCalledWith(
        { repository: APP, job: 'lint' },
        true
      )
    );
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Release' }).hasAttribute('disabled')
      ).toBe(false)
    );
    fireEvent.click(screen.getByRole('button', { name: 'Release' }));

    await waitFor(() =>
      expect(onQuarantine).toHaveBeenCalledWith(
        { repository: APP, job: 'e2e' },
        false
      )
    );
  });

  it('offers neither while the jobs cannot be changed', () => {
    render(() => <FlakyCiTable ci={CI} />);

    expect(screen.queryByRole('button', { name: 'Quarantine' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Release' })).toBeNull();
  });
});
