import type { FetchWithTokenErrorCode } from '@core/util/fetchWithToken';
import { fireAndForget } from '@core/util/fireAndForget';
import { type ResultError, throwOnErr } from '@core/util/result';
import { useAgentRepositoriesQuery } from '@queries/agent-repositories/repositories';
import {
  factoryClient,
  type RetryRunErrorCode,
  type StartRunErrorCode,
} from '@service-factory/client';
import type {
  GateModelReading as ApiGateModelReading,
  RunDetail,
} from '@service-factory/generated/schemas';
import {
  type QueryClient,
  queryOptions,
  type UseQueryResult,
  useQuery,
} from '@tanstack/solid-query';
import type { Accessor } from 'solid-js';
import type {
  RetryRunFailure,
  RunActionResult,
  RunsContext,
  RunsQueue,
  RunsSource,
  StartRunFailure,
} from '../context/runs-context';
import { type RunScore, scoresRefetchInterval } from '../core/evaluation';
import { type GateModelReading, type Run, runStatus } from '../core/run';
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

/** The Gate model's reading of a Gate, as the view keeps it. */
function modelReading(
  reading: ApiGateModelReading | null | undefined
): GateModelReading | undefined {
  if (!reading) return undefined;
  return {
    pick: reading.pick ?? undefined,
    confidence: reading.confidence ?? undefined,
    reason: reading.reason,
  };
}

/** The agent that answered `seq`'s Gate under a Standing Role, if one did. */
function standingAnswer(
  detail: RunDetail,
  seq: number
): { agent: string; standingRole: string } | undefined {
  const act = (detail.standingActs ?? []).find(
    (standing) =>
      standing.seq === seq &&
      (standing.act === 'answer_plan' || standing.act === 'answer_blockers')
  );
  return act ? { agent: act.agent, standingRole: act.standingRole } : undefined;
}

export function toRun(detail: RunDetail): Run {
  const { run } = detail;
  return {
    id: run.id,
    request: run.request,
    createdAt: run.createdAt,
    ownerId: run.userId,
    flow: run.flow ?? undefined,
    createdInPerson: run.createdInPerson,
    taskId: run.taskId ?? undefined,
    projectId: run.projectId ?? undefined,
    pullRequestUrl: run.pullRequestUrl ?? undefined,
    repository: run.repository ?? undefined,
    automation: run.automation ?? undefined,
    definition: run.definition ?? undefined,
    controlFiles: run.controlFiles ?? undefined,
    end: run.end
      ? {
          outcome: run.end.outcome,
          endedAt: run.end.endedAt,
          flaw: run.end.flaw ?? undefined,
        }
      : undefined,
    steps: detail.steps.map((step) => ({
      seq: step.seq,
      entry: step.entry,
      visit: step.visit,
      startedAt: step.startedAt,
      end: step.end ?? undefined,
      gate: step.gate
        ? {
            kind: step.gate.kind,
            question: step.gate.question,
            answers: step.gate.answers,
            openedAt: step.gate.openedAt,
            answer: step.gate.answer
              ? {
                  answer: step.gate.answer.answer,
                  note: step.gate.answer.note ?? undefined,
                  rule: step.gate.answer.rule ?? undefined,
                  approvedControlFiles:
                    step.gate.answer.approvedControlFiles ?? undefined,
                  replyConfidence:
                    step.gate.answer.replyConfidence ?? undefined,
                  byStandIn: step.gate.answer.byStandIn,
                  byOrchestrator: step.gate.answer.agentSessionId != null,
                  standing: standingAnswer(detail, step.seq),
                  answeredAt: step.gate.answer.answeredAt,
                }
              : undefined,
            askedBack: step.gate.askedBack
              ? {
                  reply: step.gate.askedBack.reply,
                  at: step.gate.askedBack.at,
                  reading: modelReading(step.gate.askedBack.reading),
                  refusedTampered: step.gate.askedBack.refusedTampered,
                }
              : undefined,
            standIn: step.gate.standIn ?? undefined,
            standInDeclined: modelReading(step.gate.standInDeclined),
          }
        : undefined,
      role: step.role ?? undefined,
      agent: step.agent ?? undefined,
      passedOver: step.passedOver,
      // A service older than the Step Terminal sends none.
      terminal: step.terminal
        ? {
            sessionId: step.terminal.sessionId ?? undefined,
            pausedAt: step.terminal.pausedAt ?? undefined,
            stopAskedAt: step.terminal.stopAskedAt ?? undefined,
            betweenTurns: step.terminal.betweenTurns,
            endsBy: step.terminal.endsBy ?? undefined,
            pauseRanOutAt: step.terminal.pauseRanOutAt ?? undefined,
            steers: step.terminal.steers.map((steer) => ({
              action: steer.action,
              note: steer.note ?? undefined,
              undelivered: steer.undelivered,
              at: steer.at,
            })),
          }
        : undefined,
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
    // A service older than disputes sends none.
    blockerResolutions: detail.blockerResolutions ?? undefined,
    // A Run with no budget, and a service older than budgets, send none.
    tokenBudget: detail.tokenBudget
      ? {
          budget: detail.tokenBudget.budget,
          used: detail.tokenBudget.used ?? undefined,
          measured: detail.tokenBudget.measured,
          stop: detail.tokenBudget.stop ?? undefined,
        }
      : undefined,
    // A service older than Standing Roles sends none.
    standingActs: (detail.standingActs ?? []).map((standing) => ({
      act: standing.act,
      seq: standing.seq ?? undefined,
      agent: standing.agent,
      standingRole: standing.standingRole,
      at: standing.at,
    })),
    // A service older than Agent Rules sends none.
    agentRules: (detail.agentRules ?? []).map((chosen) => ({
      role: chosen.role,
      rule: chosen.rule,
      agent: chosen.agent,
    })),
    taskTagsUnread: detail.taskTagsUnread ?? false,
    // A service older than retries sends none.
    retry: run.retry
      ? {
          retriedFrom: run.retry.retriedFrom,
          branchOf: run.retry.branchOf,
          firstSeq: run.retry.firstSeq,
        }
      : undefined,
    reviewRounds: detail.reviewRounds.map((round) => ({
      round: round.round,
      visit: round.visit,
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
        fixStatus: finding.fixStatus ?? undefined,
        dispute: finding.dispute ?? undefined,
        ruling: finding.ruling ?? undefined,
      })),
    })),
  };
}

/**
 * Seed the Run's own read with what a list read of it. A list reads no
 * Run's token use, so the use its own read found is kept, rather than
 * shown unknown until the next read.
 */
export function seedRun(client: QueryClient, run: Run) {
  const key = factoryRunKeys.run(run.id).queryKey;
  const known = client.getQueryData<Run>(key)?.tokenBudget;
  const listed = run.tokenBudget;
  const tokenBudget =
    listed && listed.used === undefined && known?.used !== undefined
      ? { ...listed, used: known.used, measured: known.measured }
      : listed;
  client.setQueryData(key, { ...run, tokenBudget });
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

/**
 * The person's Runs that hold a place in their Queue; read again slowly, as
 * a waiting task's are.
 */
export function waitingRunsOptions(client: QueryClient) {
  return queryOptions({
    queryKey: factoryRunKeys.waiting.queryKey,
    queryFn: async ({ signal }): Promise<RunsQueue> => {
      const queue = await throwOnErr(() =>
        factoryClient.listWaitingRuns(signal)
      );
      const waiting = queue.runs.map(toRun);
      const starting = queue.starting.map(toRun);
      for (const run of [...waiting, ...starting]) seedRun(client, run);
      return { waiting, starting };
    },
    refetchInterval: WAITING_POLL_MS,
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

/** How the live Scorers' judgments of one of the person's Runs are read. */
function runScoresOptions(runId: string) {
  return queryOptions({
    queryKey: factoryRunKeys.runScores(runId).queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getRunScores(runId, signal)),
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

/**
 * Why a Run didn't start: the repository, Queue or Definition cause the
 * service named, if any.
 */
function startFailure(
  errors: ResultError<FetchWithTokenErrorCode | StartRunErrorCode>[]
): StartRunFailure {
  const codes = errors.map((error) => error.code);
  if (codes.includes('NO_PUSH_ACCESS')) return 'noPushAccess';
  if (codes.includes('UNREACHABLE_REPOSITORY')) return 'unreachableRepository';
  if (codes.includes('QUEUE_FULL')) return 'queueFull';
  if (codes.includes('DEFINITION_UNREADABLE')) return 'definitionUnreadable';
  return 'failed';
}

/** Why a retry didn't start: a full Queue, a refusal, or a failure. */
function retryFailure(
  errors: ResultError<FetchWithTokenErrorCode | RetryRunErrorCode>[]
): RetryRunFailure {
  const codes = errors.map((error) => error.code);
  if (codes.includes('QUEUE_FULL')) return 'queueFull';
  if (codes.includes('CONFLICT')) return 'conflict';
  return 'failed';
}

/**
 * Reads the Run, its task's list, the Queue, the board and the Runs of pull
 * requests again; resolves once they are read.
 */
function refreshRun(
  client: QueryClient,
  runId: string,
  taskId?: string
): Promise<unknown> {
  const reads = [
    client.invalidateQueries({ queryKey: factoryRunKeys.run(runId).queryKey }),
    client.invalidateQueries({ queryKey: factoryRunKeys.waiting.queryKey }),
    client.invalidateQueries({ queryKey: factoryRunKeys.board._def }),
    client.invalidateQueries({ queryKey: factoryRunKeys.pullRequests._def }),
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

/**
 * The Runs context on the factory service, cached in `client`; the app's
 * entry points add the signed-in person's id.
 */
export function createAppRunsContext(client: QueryClient): RunsContext {
  return {
    taskRuns: (taskId: Accessor<string>) =>
      toSource(useQuery(() => taskRunsOptions(client, taskId()))),

    run: (runId: Accessor<string>) =>
      toSource(useQuery(() => runOptions(runId()))),

    waitingRuns: () => toSource(useQuery(() => waitingRunsOptions(client))),

    runScores: (
      runId: Accessor<string>,
      enabled: Accessor<boolean>,
      endedAt: Accessor<string | undefined>
    ) =>
      toSource(
        useQuery(() => ({
          ...runScoresOptions(runId()),
          enabled: enabled() && Boolean(runId()),
          refetchInterval: () => scoresRefetchInterval(endedAt(), Date.now()),
        }))
      ),

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

    async replyToGate(input) {
      const result = await factoryClient.replyToGate(
        input.runId,
        input.step.seq,
        { reply: input.reply }
      );
      // Read the Run again, whether the words answered the Gate or were
      // asked back.
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
      if (refused.reason === 'conflict') {
        await reads.catch(() => undefined);
      } else {
        fireAndForget(reads, 'factory-runs');
      }
      return refused;
    },

    async steerStep(input) {
      const runId = input.runId;
      const seq = input.step.seq;
      const result = await (() => {
        switch (input.action) {
          case 'note':
            return factoryClient.noteStep(runId, seq, { note: input.note });
          case 'pause':
            return factoryClient.pauseStep(runId, seq);
          case 'resume':
            return factoryClient.resumeStep(runId, seq, { note: input.note });
          case 'stop':
            return factoryClient.stopStep(runId, seq);
        }
      })();
      // Read the Run again either way: it shows what the owner did, or what
      // the Step did instead.
      fireAndForget(
        refreshRun(client, runId, taskOfRun(client, runId)),
        'factory-runs'
      );
      if (result.isErr()) return failure(result.error);
      return { ok: true, value: undefined };
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

    async retryRun(runId) {
      const result = await factoryClient.retryRun(runId);
      // Retried or refused, the Run's lists and its insights read again: a
      // retry shows in them, and a refusal means the Run moved on.
      const reads = () =>
        Promise.all([
          refreshRun(client, runId, taskOfRun(client, runId)),
          client.invalidateQueries({ queryKey: factoryRunKeys.insights._def }),
        ]);
      if (result.isErr()) {
        fireAndForget(reads(), 'factory-runs');
        return { ok: false, reason: retryFailure(result.error) };
      }
      const run = toRun(result.value);
      seedRun(client, run);
      const taskId = run.taskId;
      if (taskId) {
        client.setQueryData<Run[]>(
          factoryRunKeys.task(taskId).queryKey,
          (runs) => [run, ...(runs ?? []).filter((old) => old.id !== run.id)]
        );
      }
      fireAndForget(reads(), 'factory-runs');
      return { ok: true, value: run };
    },

    async markRunFlawed({ runId, reason }) {
      const result = await factoryClient.flawRun(runId, { reason });
      if (result.isErr()) {
        const refused = failure(result.error);
        // A Run that did not end in success has moved on from what this
        // person saw: it resolves once the Run is read again.
        const reads = refreshRun(client, runId, taskOfRun(client, runId));
        if (refused.reason === 'conflict') await reads.catch(() => undefined);
        else fireAndForget(reads, 'factory-runs');
        return refused;
      }
      seedRun(client, toRun(result.value));
      // A flawed Run counts apart in insights.
      fireAndForget(
        Promise.all([
          refreshRun(client, runId, taskOfRun(client, runId)),
          client.invalidateQueries({ queryKey: factoryRunKeys.insights._def }),
        ]),
        'factory-runs'
      );
      return { ok: true, value: undefined };
    },

    async labelRunScore({ runId, scorer, passed }) {
      const result = await factoryClient.labelRunScore(runId, scorer, passed);
      if (result.isErr()) return failure(result.error);
      const label = result.value;
      client.setQueryData<RunScore[]>(
        factoryRunKeys.runScores(runId).queryKey,
        (scores) =>
          scores?.map((score) =>
            score.scorer === scorer ? { ...score, label } : score
          )
      );
      // The label moves the Scorer's calibration, here and in insights.
      fireAndForget(
        Promise.all([
          client.invalidateQueries({
            queryKey: factoryRunKeys.runScores(runId).queryKey,
          }),
          client.invalidateQueries({
            queryKey: factoryRunKeys.evaluation._def,
          }),
        ]),
        'factory-runs'
      );
      return { ok: true, value: undefined };
    },
  };
}
