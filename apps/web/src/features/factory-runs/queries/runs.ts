import type { FetchWithTokenErrorCode } from '@core/util/fetchWithToken';
import { fireAndForget } from '@core/util/fireAndForget';
import { type ResultError, throwOnErr } from '@core/util/result';
import { useAgentRepositoriesQuery } from '@queries/agent-repositories/repositories';
import { factoryClient, type StartRunErrorCode } from '@service-factory/client';
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
  StartRunFailure,
} from '../context/runs-context';
import { type Run, runStatus } from '../core/run';
import { factoryRunKeys } from './keys';

/** How often an open Run, or a task with a Run going, is read again. */
const LIVE_POLL_MS = 3_000;

/**
 * How often a task whose live Runs all wait on a person is read again. A
 * Gate can wait for days, and every open task polls; this still catches an
 * answer given somewhere else.
 */
const WAITING_POLL_MS = 60_000;

/** A task's Runs are read fast while a Step runs, slowly while they wait. */
function taskRunsPollMs(runs: Run[] | undefined): number | false {
  const live = (runs ?? []).filter((run) => !run.end);
  if (live.length === 0) return false;
  const running = live.some((run) => runStatus(run).kind === 'running');
  return running ? LIVE_POLL_MS : WAITING_POLL_MS;
}

function toRun(detail: RunDetail): Run {
  const { run } = detail;
  return {
    id: run.id,
    request: run.request,
    createdAt: run.createdAt,
    taskId: run.taskId ?? undefined,
    repository: run.repository ?? undefined,
    controlFiles: run.controlFiles ?? undefined,
    end: run.end ?? undefined,
    steps: detail.steps.map((step) => ({
      seq: step.seq,
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
                  approvedControlFiles:
                    step.gate.answer.approvedControlFiles ?? undefined,
                  answeredAt: step.gate.answer.answeredAt,
                }
              : undefined,
          }
        : undefined,
      role: step.role ?? undefined,
      agent: step.agent ?? undefined,
      passedOver: step.passedOver,
      checks: step.checks.map((round) => ({
        round: round.round,
        checks: round.checks,
        startedAt: round.startedAt,
        verified: round.verified,
        checkedCommit: round.checkedCommit ?? undefined,
        results: round.results.map((result) => ({
          name: result.name,
          exitCode: result.exitCode,
        })),
        end: round.end ?? undefined,
      })),
    })),
    mainHold: detail.mainHold
      ? {
          since: detail.mainHold.since,
          headSha: detail.mainHold.headSha,
          failed: detail.mainHold.failed,
          taskId: detail.mainHold.taskId ?? undefined,
        }
      : undefined,
    baseRequiredChecks: detail.baseRequiredChecks ?? undefined,
    reviewRounds: detail.reviewRounds.map((round) => ({
      round: round.round,
      seq: round.seq,
      reviewers: round.reviewers,
      headSha: round.headSha ?? undefined,
      diffBase: round.diffBase ?? undefined,
      fixDiff: round.fixDiff,
      failure: round.failure ?? undefined,
      startedAt: round.startedAt,
      end: round.end ?? undefined,
      findings: round.findings.map((finding) => ({
        seq: finding.seq,
        place: finding.place,
        role: finding.role ?? undefined,
        path: finding.path,
        lines: finding.lines ?? undefined,
        problem: finding.problem,
        why: finding.why,
        blocksAnywhere: finding.blocksAnywhere,
        kind: finding.kind ?? undefined,
        followUpTaskId: finding.followUpTaskId ?? undefined,
      })),
    })),
  };
}

function seedRun(client: QueryClient, run: Run) {
  client.setQueryData(factoryRunKeys.run(run.id).queryKey, run);
}

export function taskRunsOptions(client: QueryClient, taskId: string) {
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
    refetchInterval: (query) => taskRunsPollMs(query.state.data),
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

export function flowsOptions() {
  return queryOptions({
    queryKey: factoryRunKeys.flows.queryKey,
    queryFn: ({ signal }) => throwOnErr(() => factoryClient.listFlows(signal)),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export function toSource<T>(query: UseQueryResult<T>): RunsSource<T> {
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

function failure(
  errors: ResultError[]
): Extract<RunActionResult, { ok: false }> {
  const conflict = errors.some((error) => error.code === 'CONFLICT');
  return { ok: false, reason: conflict ? 'conflict' : 'failed' };
}

/** Why a Run didn't start: the repository cause the service named, if any. */
function startFailure(
  errors: ResultError<FetchWithTokenErrorCode | StartRunErrorCode>[]
): StartRunFailure {
  const codes = errors.map((error) => error.code);
  if (codes.includes('NO_PUSH_ACCESS')) return 'noPushAccess';
  if (codes.includes('UNREACHABLE_REPOSITORY')) return 'unreachableRepository';
  return 'failed';
}

/** Reads the Run, and its task's list, again; resolves once they are read. */
function refreshRun(
  client: QueryClient,
  runId: string,
  taskId?: string
): Promise<unknown> {
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
  return Promise.all(reads);
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

    repositories: (enabled: Accessor<boolean>) => {
      // The repositories a coding session can be given: each agent Step of
      // a Run that works on a repository is one.
      const source = toSource(useAgentRepositoriesQuery(enabled));
      return {
        ...source,
        data: () =>
          source.data()?.repositories.map((repository) => repository.url),
      };
    },

    async startRun(input) {
      const result = await factoryClient.startRun(input);
      if (result.isErr()) {
        return { ok: false, reason: startFailure(result.error) };
      }

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
        input.step.seq,
        {
          answer: input.answer,
          note: input.note,
          approvedControlFiles: input.approvedControlFiles,
        }
      );
      // Refused or not, the Run has moved on from what this person saw.
      const reads = refreshRun(
        client,
        input.runId,
        taskOfRun(client, input.runId)
      );
      if (result.isOk()) {
        fireAndForget(reads, 'factory-runs');
        return { ok: true, value: undefined };
      }
      const refused = failure(result.error);
      // A refused answer resolves once the Run is read again, so the caller
      // sees whether the Gate it answered is still the one the Run waits on.
      if (refused.reason === 'conflict') {
        await reads.catch(() => undefined);
      } else {
        fireAndForget(reads, 'factory-runs');
      }
      return refused;
    },

    async cancelRun(runId) {
      const result = await factoryClient.cancelRun(runId);
      fireAndForget(
        refreshRun(client, runId, taskOfRun(client, runId)),
        'factory-runs'
      );
      if (result.isErr()) return failure(result.error);
      return { ok: true, value: undefined };
    },
  };
}
