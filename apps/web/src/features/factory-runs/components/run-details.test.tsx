/** @vitest-environment jsdom */

import { cleanup, render, screen } from '@solidjs/testing-library';
import { afterEach, describe, expect, it } from 'vitest';
import type { Run } from '../core/run';
import { RunSummary } from './run-details';

const COMMIT = '0123456789abcdef0123456789abcdef01234567';

const RUN: Run = {
  id: '00000000-0000-7000-8000-000000000001',
  request: 'Note it',
  createdAt: '2026-11-20T10:00:00Z',
  flow: 'notes',
  steps: [],
};

function mount(run: Run) {
  render(() => (
    <RunSummary run={run} status={{ kind: 'running' }} time={(at) => at} />
  ));
}

afterEach(() => {
  cleanup();
});

describe('RunSummary', () => {
  it('names the Definition commit the Run read, linked to its files', () => {
    mount({
      ...RUN,
      definition: {
        repoUrl: 'https://github.com/macro/factory-definition',
        commit: COMMIT,
      },
    });

    expect(screen.getByText('Factory definition')).toBeTruthy();
    const link = screen.getByRole('link', {
      name: 'macro/factory-definition at 0123456',
    });
    expect(link.getAttribute('href')).toBe(
      `https://github.com/macro/factory-definition/tree/${COMMIT}`
    );
  });

  it('names no Definition for a Run of the built-in Flows', () => {
    mount(RUN);

    expect(screen.queryByText('Factory definition')).toBeNull();
  });
});
