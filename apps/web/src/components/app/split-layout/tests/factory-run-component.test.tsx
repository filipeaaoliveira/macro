/** @vitest-environment jsdom */
import { cleanup, render, screen } from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { resolveComponent } from '../componentRegistry';

vi.mock('@app/features/factory-runs/factory-runs', () => ({
  FactoryRun: (props: { runId: string }) => <p>Run {props.runId}</p>,
  FactoryInsights: () => <p>Run insights</p>,
}));

afterEach(() => {
  cleanup();
});

describe('the factory-run component', () => {
  it('shows the Run its notification names', async () => {
    const resolved = resolveComponent('factory-run', { runId: 'run-1' });
    render(() => resolved.element());

    expect(await screen.findByText('Run run-1')).toBeTruthy();
    expect(screen.queryByText('Run insights')).toBeNull();
  });
});
