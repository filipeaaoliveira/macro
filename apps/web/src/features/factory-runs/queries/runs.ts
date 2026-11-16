import { fireAndForget } from '@core/util/fireAndForget';
import { type ResultError, throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { RunDetail } from '@service-factory/generated/schemas';
import {
  type QueryClient,
  queryOptions,
  type UseQueryResult,
  useQuery,
} from '@tanstack/solid-query';
import type { Accessor } from 'solid-js';
import type {
  RunActionResult,
  RunsContext,
  RunsSource,
} from '../context/runs-context';
import type { Run } from '../core/run';
import { factoryRunKeys } from './keys';

/** How often an open Run, or a task with a Run going, is read again. */
const LIVE_POLL_MS = 3_000;

function toRun(detail: RunDetail): Run {
  const { run } = detail;
  return {
    id: run.id,
    request: run.request,
    createdAt: run.createdAt,
    taskId: run.taskId ?? undefined,
    end: run.end ?? undefined,
    steps: detail.steps.map((step) => ({
      entry: step.entry,
      visit: step.visit,
      startedAt: step.startedAt,
      end: step.end ?? undefined,
      gate: step.gate
        ? {
            question: step.gate.question,
            answers: step.gate.answers,
            openedAt: step.gate.openedAt,
            answer: step.gate.answer
              ? {
                  answer: step.gate.answer.answer,
                  note: step.gate.answer.note ?? undefined,
                  answeredAt: step.gate.answer.answeredAt,
                }
              : undefined,
          }
        : undefined,
    })),
  };
}

function seedRun(client: QueryClient, run: Run) {
  client.setQueryData(factoryRunKeys.run(run.id).queryKey, run);
}

function taskRunsOptions(client: QueryClient, taskId: string) {
  return queryOptions({
    queryKey: factoryRunKeys.task(taskId).queryKey,
    enabled: Boolean(taskId),
    queryFn: async ({ signal }) => {
      const details = await throwOnErr(() =>
        factoryClient.listTaskRuns(taskId, signal)
      );
      const runs = details.map(toRun);
      // An opened Run shows what the list already read while it loads.
      for (const run of runs) seedRun(client, run);
      return runs;
    },
    refetchInterval: (query) =>
      query.state.data?.some((run) => !run.end) ? LIVE_POLL_MS : false,
    reconcile: 'id',
  });
}

export function runOptions(runId: string) {
  return queryOptions({
    queryKey: factoryRunKeys.run(runId).queryKey,
    enabled: Boolean(runId),
    queryFn: async ({ signal }) =>
      toRun(await throwOnErr(() => factoryClient.getRun(runId, signal))),
    refetchInterval: (query) =>
      query.state.data && !query.state.data.end ? LIVE_POLL_MS : false,
    // Polled under an open dialog: keep its Steps' nodes, and their open
    // outputs, across reads.
    reconcile: 'id',
  });
}

function flowsOptions() {
  return queryOptions({
    queryKey: factoryRunKeys.flows.queryKey,
    queryFn: ({ signal }) => throwOnErr(() => factoryClient.listFlows(signal)),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

function toSource<T>(query: UseQueryResult<T>): RunsSource<T> {
  // Reading `data` while pending would suspend the enclosing section.
  const data = () => (query.isPending ? undefined : query.data);
  return {
    state: () => {
      if (query.isPending) return 'pending';
      return data() === undefined ? 'error' : 'ready';
    },
    data,
    retry: () => fireAndForget(query.refetch(), 'factory-runs'),
  };
}

function failure(errors: ResultError[]): RunActionResult<never> {
  const conflict = errors.some((error) => error.code === 'CONFLICT');
  return { ok: false, reason: conflict ? 'conflict' : 'failed' };
}

function refreshRun(client: QueryClient, runId: string, taskId?: string) {
  const reads = [
    client.invalidateQueries({ queryKey: factoryRunKeys.run(runId).queryKey }),
  ];
  if (taskId) {
    reads.push(
      client.invalidateQueries({
        queryKey: factoryRunKeys.task(taskId).queryKey,
      })
    );
  }
  fireAndForget(Promise.all(reads), 'factory-runs');
}

function taskOfRun(client: QueryClient, runId: string) {
  return client.getQueryData<Run>(factoryRunKeys.run(runId).queryKey)?.taskId;
}

/** The Runs context on the factory service, cached in `client`. */
export function createAppRunsContext(client: QueryClient): RunsContext {
  return {
    taskRuns: (taskId: Accessor<string>) =>
      toSource(useQuery(() => taskRunsOptions(client, taskId()))),

    run: (runId: Accessor<string>) =>
      toSource(useQuery(() => runOptions(runId()))),

    flows: () => toSource(useQuery(() => flowsOptions())),

    async startRun(input) {
      const result = await factoryClient.startRun(input);
      if (result.isErr()) return failure(result.error);

      const run = toRun(result.value);
      seedRun(client, run);
      client.setQueryData<Run[]>(
        factoryRunKeys.task(input.taskId).queryKey,
        (runs) => [run, ...(runs ?? []).filter((old) => old.id !== run.id)]
      );
      return { ok: true, value: run };
    },

    async answerGate(input) {
      const result = await factoryClient.answerGate(
        input.runId,
        input.step.entry,
        input.step.visit,
        { answer: input.answer, note: input.note }
      );
      // Refused or not, the Run has moved on from what this person saw.
      refreshRun(client, input.runId, taskOfRun(client, input.runId));
      if (result.isErr()) return failure(result.error);
      return { ok: true, value: undefined };
    },

    async cancelRun(runId) {
      const result = await factoryClient.cancelRun(runId);
      refreshRun(client, runId, taskOfRun(client, runId));
      if (result.isErr()) return failure(result.error);
      return { ok: true, value: undefined };
    },
  };
}
