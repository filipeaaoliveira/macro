/** @vitest-environment jsdom */

import { cleanup, render, screen } from '@solidjs/testing-library';
import { afterEach, describe, expect, it } from 'vitest';
import { KpiStrip } from './insights-overview';

afterEach(() => {
  cleanup();
});

describe('KpiStrip', () => {
  it('keeps the median run time beside time to merge', () => {
    render(() => (
      <KpiStrip
        kpis={{
          runs: { value: 4 },
          medianRunSeconds: { value: 600, trend: [600, 300] },
          timeToMergeSeconds: { value: 5400 },
        }}
      />
    ));

    expect(screen.getByText('Median run time')).toBeTruthy();
    expect(screen.getByText('Time to merge')).toBeTruthy();
  });

  it('shows no time to merge while nothing merged', () => {
    render(() => (
      <KpiStrip
        kpis={{
          runs: { value: 4 },
          medianRunSeconds: { value: 600 },
        }}
      />
    ));

    expect(screen.getByText('Median run time')).toBeTruthy();
    expect(screen.queryByText('Time to merge')).toBeNull();
  });
});
