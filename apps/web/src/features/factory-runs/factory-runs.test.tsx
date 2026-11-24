/** @vitest-environment jsdom */
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { cleanup, render, screen } from '@solidjs/testing-library';
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from '@tanstack/solid-query';
import { createSignal, type JSX, Show, Suspense } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FactoryRun, TaskRuns } from './factory-runs';
import { createAppRunsContext } from './queries/runs';

const flags = vi.hoisted(() => ({ enabled: (): boolean => false }));

// The flag read itself is PostHog's; this checks which flag guards what.
vi.mock('@app/lib/analytics/posthog', () => ({
  ShowFeatureFlag: (props: { flag: unknown; children: JSX.Element }) => (
    <Show when={props.flag === enableFactoryRuns && flags.enabled()}>
      {props.children}
    </Show>
  ),
}));
vi.mock('@queries/client', () => ({ queryClient: {} }));
vi.mock('@core/constant/SettingsState', () => ({
  useSettingsState: () => ({ openSettings: vi.fn() }),
}));
vi.mock('@core/context/user', () => ({
  useUserId: () => () => 'macro|julia@example.com',
}));
vi.mock('./queries/runs', () => ({ createAppRunsContext: vi.fn(() => ({})) }));
// The Definition is read as the app reads it, but never answers.
vi.mock('./queries/definition', () => ({
  useDefinitionQuery: () =>
    useQuery(() => ({
      queryKey: ['definition'],
      queryFn: () => new Promise<never>(() => {}),
    })),
}));
vi.mock('./run-view', () => ({
  RunPageView: (props: {
    runId: string;
    flowEntries?: (flow: string) => readonly string[] | undefined;
  }) => (
    <p>
      Run {props.runId}, {props.flowEntries?.('code')?.join(' ') ?? 'built-in'}
    </p>
  ),
}));
vi.mock('./views/task-runs-section', () => ({
  TaskRunsSection: (props: { taskId: string }) => (
    <section>Runs of {props.taskId}</section>
  ),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('TaskRuns', () => {
  it('mounts nothing, and reads nothing, while the flag is off', () => {
    const [enabled, setEnabled] = createSignal(false);
    flags.enabled = enabled;
    render(() => <TaskRuns taskId="task-1" order={36} />);

    expect(screen.queryByText('Runs of task-1')).toBeNull();
    expect(createAppRunsContext).not.toHaveBeenCalled();

    setEnabled(true);
    expect(screen.getByText('Runs of task-1')).toBeTruthy();
    expect(createAppRunsContext).toHaveBeenCalledOnce();
  });
});

describe('FactoryRun', () => {
  it('shows the Run while the Definition is still being read', () => {
    flags.enabled = () => true;
    render(() => (
      <QueryClientProvider client={new QueryClient()}>
        <Suspense fallback={<p>Loading</p>}>
          <FactoryRun runId="run-1" />
        </Suspense>
      </QueryClientProvider>
    ));

    expect(screen.queryByText('Loading')).toBeNull();
    expect(screen.getByText('Run run-1, built-in')).toBeTruthy();
  });
});
