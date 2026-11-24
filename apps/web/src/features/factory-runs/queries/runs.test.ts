import { factoryClient } from '@service-factory/client';
import type { RunDetail, RunScores } from '@service-factory/generated/schemas';
import { QueryClient, QueryObserver } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RunScore } from '../core/evaluation';
import type { Run } from '../core/run';
import { factoryRunKeys } from './keys';
import {
  createAppRunsContext,
  runOptions,
  seedRun,
  taskRunsOptions,
  waitingRunsOptions,
} from './runs';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    listTaskRuns: vi.fn(),
    listWaitingRuns: vi.fn(),
    getRun: vi.fn(),
    listFlows: vi.fn(),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    cancelRun: vi.fn(),
    retryRun: vi.fn(),
    flawRun: vi.fn(),
    steerStep: vi.fn(),
    labelRunScore: vi.fn(),
  },
}));

const started: RunDetail = {
  run: {
    id: 'run-2',
    request: 'Add a due date column',
    createdAt: '2026-11-16T09:00:00Z',
    taskId: 'task-1',
    userId: 'macro|owner@example.com',
    createdInPerson: true,
    end: null,
  },
  steps: [
    {
      terminal: { betweenTurns: false, steers: [] },
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:01Z',
      passedOver: [{ agent: 'backup', reason: 'ai_allowance_exhausted' }],
      role: 'planner',
      agent: 'primary',
      end: null,
      gate: null,
      checks: [
        {
          round: 1,
          checks: ['web'],
          startedAt: '2026-11-16T09:00:02Z',
          verified: true,
          checkedCommit: '0123456789abcdef0123456789abcdef01234567',
          results: [{ name: 'web', exitCode: 1, output: 'error: 1 failed' }],
          end: null,
        },
      ],
    },
  ],
  standingActs: [],
  agentRules: [],
  taskTagsUnread: false,
  blockerResolutions: {
    fixed: 0,
    disputedUpheld: 0,
    disputedDropped: 0,
    dismissed: 0,
  },
  breakRecords: [],
  ciReruns: [],
  ciQuarantines: [],
  splitTasks: [],
  reviewRounds: [
    {
      round: 1,
      visit: 1,
      seq: 2,
      reviewers: 1,
      headSha: 'abc123',
      diffBase: 'main',
      fixDiff: false,
      failure: null,
      startedAt: '2026-11-16T09:01:00Z',
      end: null,
      findings: [
        {
          seq: 2,
          place: 1,
          role: null,
          path: 'README.md',
          lines: null,
          problem: 'Names the old flag',
          why: 'People follow it',
          blocksAnywhere: false,
          kind: null,
          followUpTaskId: null,
        },
      ],
    },
  ],
};

const older: Run = {
  id: 'run-1',
  request: 'Rename the column',
  createdAt: '2026-11-15T09:00:00Z',
  taskId: 'task-1',
  end: { outcome: 'success', endedAt: '2026-11-15T10:00:00Z' },
  steps: [],
};

afterEach(() => {
  vi.clearAllMocks();
});

describe('createAppRunsContext', () => {
  it('puts a started Run first in its task, without the wire nulls', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.task('task-1').queryKey, [older]);
    vi.mocked(factoryClient.startRun).mockResolvedValue(ok(started));

    const result = await createAppRunsContext(client).startRun({
      taskId: 'task-1',
      flow: 'plan',
      request: 'Add a due date column',
    });

    const run: Run = {
      id: 'run-2',
      breakRecords: [],
      splitTasks: [],
      standingActs: [],
      agentRules: [],
      taskTagsUnread: false,
      blockerResolutions: {
        fixed: 0,
        disputedUpheld: 0,
        disputedDropped: 0,
        dismissed: 0,
      },
      request: 'Add a due date column',
      createdAt: '2026-11-16T09:00:00Z',
      ownerId: 'macro|owner@example.com',
      createdInPerson: true,
      taskId: 'task-1',
      end: undefined,
      steps: [
        {
          seq: 1,
          entry: 'plan',
          visit: 1,
          startedAt: '2026-11-16T09:00:01Z',
          role: 'planner',
          agent: 'primary',
          passedOver: [{ agent: 'backup', reason: 'ai_allowance_exhausted' }],
          end: undefined,
          gate: undefined,
          terminal: {
            sessionId: undefined,
            pausedAt: undefined,
            stopAskedAt: undefined,
            betweenTurns: false,
            endsBy: undefined,
            pauseRanOutAt: undefined,
            steers: [],
          },
          checks: [
            {
              round: 1,
              checks: ['web'],
              startedAt: '2026-11-16T09:00:02Z',
              verified: true,
              checkedCommit: '0123456789abcdef0123456789abcdef01234567',
              results: [{ name: 'web', exitCode: 1 }],
              end: undefined,
            },
          ],
        },
      ],
      reviewRounds: [
        {
          round: 1,
          visit: 1,
          seq: 2,
          reviewers: 1,
          headSha: 'abc123',
          diffBase: 'main',
          fixDiff: false,
          failure: undefined,
          startedAt: '2026-11-16T09:01:00Z',
          end: undefined,
          findings: [
            {
              seq: 2,
              place: 1,
              role: undefined,
              path: 'README.md',
              lines: undefined,
              problem: 'Names the old flag',
              why: 'People follow it',
              blocksAnywhere: false,
              kind: undefined,
              followUpTaskId: undefined,
            },
          ],
        },
      ],
    };
    expect(result).toEqual({ ok: true, value: run });
    expect(client.getQueryData(factoryRunKeys.task('task-1').queryKey)).toEqual(
      [run, older]
    );
    expect(client.getQueryData(factoryRunKeys.run('run-2').queryKey)).toEqual(
      run
    );
  });

  it('reads a Gate the Orchestrator answered as its answer', async () => {
    const answered = (agentSessionId: string | null): RunDetail => ({
      ...started,
      steps: [
        {
          terminal: { betweenTurns: false, steers: [] },
          seq: 1,
          entry: 'plan',
          visit: 1,
          startedAt: '2026-11-16T09:00:01Z',
          passedOver: [],
          checks: [],
          gate: {
            kind: 'flow',
            question: 'Approve the plan?',
            answers: ['approve', 'revise'],
            openedAt: '2026-11-16T09:01:00Z',
            answer: {
              answer: 'revise',
              answeredAt: '2026-11-16T09:02:00Z',
              byStandIn: false,
              agentSessionId,
            },
          },
        },
      ],
    });
    const context = createAppRunsContext(new QueryClient());
    const input = { taskId: 'task-1', flow: 'plan', request: 'Plan it' };

    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(
      ok(answered('01960000-0000-7000-8000-000000000009'))
    );
    const byAgent = await context.startRun(input);
    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(ok(answered(null)));
    const byPerson = await context.startRun(input);

    const answerOf = (result: typeof byAgent) =>
      result.ok ? result.value.steps[0]?.gate?.answer : undefined;
    expect(answerOf(byAgent)?.byOrchestrator).toBe(true);
    expect(answerOf(byPerson)?.byOrchestrator).toBe(false);
  });

  it('reads which agent answered a Gate and did each act under a Standing Role', async () => {
    const detail: RunDetail = {
      ...started,
      steps: [1, 2].map((seq) => ({
        terminal: { betweenTurns: false, steers: [] },
        seq,
        entry: 'plan',
        visit: seq,
        startedAt: '2026-11-16T09:00:01Z',
        passedOver: [],
        checks: [],
        gate: {
          kind: 'flow',
          question: 'Approve the plan?',
          answers: ['approve', 'revise'],
          openedAt: '2026-11-16T09:01:00Z',
          answer: {
            answer: 'revise',
            answeredAt: '2026-11-16T09:02:00Z',
            byStandIn: false,
            agentSessionId: '01960000-0000-7000-8000-000000000009',
          },
        },
      })),
      standingActs: [
        {
          act: 'start',
          seq: null,
          botId: '01960000-0000-7000-8000-0000000000b1',
          agent: 'backup',
          standingRole: 'intake',
          sessionId: '01960000-0000-7000-8000-000000000008',
          at: '2026-11-16T09:00:00Z',
        },
        {
          act: 'answer_plan',
          seq: 2,
          botId: '01960000-0000-7000-8000-0000000000b2',
          agent: 'primary',
          standingRole: 'gatekeeper',
          sessionId: '01960000-0000-7000-8000-000000000009',
          at: '2026-11-16T09:02:00Z',
        },
      ],
      agentRules: [{ role: 'writer', rule: 'docs', agent: 'docs-writer' }],
      taskTagsUnread: true,
    };
    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(ok(detail));

    const result = await createAppRunsContext(new QueryClient()).startRun({
      taskId: 'task-1',
      flow: 'plan',
      request: 'Plan it',
    });

    const run = result.ok ? result.value : undefined;
    expect(run?.steps[0]?.gate?.answer?.standing).toBeUndefined();
    expect(run?.steps[1]?.gate?.answer?.standing).toEqual({
      agent: 'primary',
      standingRole: 'gatekeeper',
    });
    expect(run?.standingActs?.map((standing) => standing.act)).toEqual([
      'start',
      'answer_plan',
    ]);
    expect(run?.standingActs?.[0]).toEqual({
      act: 'start',
      seq: undefined,
      agent: 'backup',
      standingRole: 'intake',
      at: '2026-11-16T09:00:00Z',
    });
    expect(run?.agentRules).toEqual([
      { role: 'writer', rule: 'docs', agent: 'docs-writer' },
    ]);
    expect(run?.taskTagsUnread).toBe(true);
  });

  it('starts a Run on the repository it names and keeps it on the Run', async () => {
    const repository = {
      url: 'https://github.com/macro-inc/app',
      baseBranch: 'main',
    };
    vi.mocked(factoryClient.startRun).mockResolvedValue(
      ok({ ...started, run: { ...started.run, repository } })
    );

    const result = await createAppRunsContext(new QueryClient()).startRun({
      taskId: 'task-1',
      flow: 'code',
      request: 'Add a due date column',
      repoUrl: 'https://github.com/macro-inc/app',
    });

    expect(factoryClient.startRun).toHaveBeenCalledWith({
      taskId: 'task-1',
      flow: 'code',
      request: 'Add a due date column',
      repoUrl: 'https://github.com/macro-inc/app',
    });
    expect(result.ok && result.value.repository).toEqual(repository);
  });

  it('tells a refused repository apart from a failed start', async () => {
    const context = createAppRunsContext(new QueryClient());
    const input = {
      taskId: 'task-1',
      flow: 'code',
      request: 'Add a due date column',
      repoUrl: 'https://github.com/macro-inc/app',
    };

    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(
      err([{ code: 'UNREACHABLE_REPOSITORY', message: 'unreachable' }])
    );
    expect(await context.startRun(input)).toEqual({
      ok: false,
      reason: 'unreachableRepository',
    });

    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(
      err([{ code: 'NO_PUSH_ACCESS', message: 'no push' }])
    );
    expect(await context.startRun(input)).toEqual({
      ok: false,
      reason: 'noPushAccess',
    });

    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(
      err([{ code: 'QUEUE_FULL', message: 'too many wait' }])
    );
    expect(await context.startRun(input)).toEqual({
      ok: false,
      reason: 'queueFull',
    });

    vi.mocked(factoryClient.startRun).mockResolvedValueOnce(
      err([{ code: 'HTTP_ERROR', message: 'HTTP error! status: 400' }])
    );
    expect(await context.startRun(input)).toEqual({
      ok: false,
      reason: 'failed',
    });
  });

  it('tells a refused answer apart from a failed one', async () => {
    const context = createAppRunsContext(new QueryClient());
    const input = {
      runId: 'run-2',
      step: { seq: 1 },
      answer: 'approve',
    };

    vi.mocked(factoryClient.answerGate).mockResolvedValueOnce(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );
    expect(await context.answerGate(input)).toEqual({
      ok: false,
      reason: 'conflict',
    });

    vi.mocked(factoryClient.answerGate).mockResolvedValueOnce(
      err([{ code: 'SERVER_ERROR', message: 'Internal server error' }])
    );
    expect(await context.answerGate(input)).toEqual({
      ok: false,
      reason: 'failed',
    });

    // An agent answering for its owner: only a person may answer a Gate.
    vi.mocked(factoryClient.answerGate).mockResolvedValueOnce(
      err([{ code: 'FORBIDDEN', message: 'Forbidden' }])
    );
    expect(await context.answerGate(input)).toEqual({
      ok: false,
      reason: 'failed',
    });
  });

  it('reads the Run and its task again after a cancel the server refused', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.run('run-2').queryKey, {
      ...older,
      id: 'run-2',
      end: undefined,
    });
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    vi.mocked(factoryClient.cancelRun).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );

    expect(await createAppRunsContext(client).cancelRun('run-2')).toEqual({
      ok: false,
      reason: 'conflict',
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.run('run-2').queryKey,
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.task('task-1').queryKey,
    });
  });

  it('puts a retry first in its task, and reads the retried Run and insights again', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.task('task-1').queryKey, [older]);
    client.setQueryData(factoryRunKeys.run('run-1').queryKey, older);
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    vi.mocked(factoryClient.retryRun).mockResolvedValue(
      ok({
        ...started,
        run: {
          ...started.run,
          retry: { retriedFrom: 'run-1', branchOf: 'run-1', firstSeq: 2 },
        },
      })
    );

    const result = await createAppRunsContext(client).retryRun('run-1');

    if (!result.ok) throw new Error('The retry was refused');
    expect(result.value.id).toBe('run-2');
    expect(result.value.retry).toEqual({
      retriedFrom: 'run-1',
      branchOf: 'run-1',
      firstSeq: 2,
    });
    expect(
      client
        .getQueryData<Run[]>(factoryRunKeys.task('task-1').queryKey)
        ?.map((run) => run.id)
    ).toEqual(['run-2', 'run-1']);
    expect(
      client.getQueryData<Run>(factoryRunKeys.run('run-2').queryKey)?.id
    ).toBe('run-2');
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.run('run-1').queryKey,
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.insights._def,
    });
  });

  it('tells a full Queue and a refused retry apart from a failed one', async () => {
    const context = createAppRunsContext(new QueryClient());
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'QUEUE_FULL', message: 'too many wait' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'queueFull',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'CONFLICT', message: 'the run was retried already' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'conflict',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'ALREADY_RETRIED', message: 'the run was retried already' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'conflict',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'NOT_RETRIABLE', message: 'no flow kept' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'conflict',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'NOT_IN_STANDING_ROLE', message: 'no role starts' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'notInStandingRole',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'DEFINITION_UNREADABLE', message: 'moved' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'definitionUnreadable',
    });
    vi.mocked(factoryClient.retryRun).mockResolvedValueOnce(
      err([{ code: 'NOT_FOUND', message: 'Resource not found' }])
    );
    expect(await context.retryRun('run-1')).toEqual({
      ok: false,
      reason: 'failed',
    });
  });

  it('reads the Run again before it answers a flaw mark the server refused', async () => {
    const client = new QueryClient();
    client.setQueryData(factoryRunKeys.run('run-2').queryKey, {
      ...older,
      id: 'run-2',
    });
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    vi.mocked(factoryClient.flawRun).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );

    expect(
      await createAppRunsContext(client).markRunFlawed({
        runId: 'run-2',
        reason: 'Wrong column',
      })
    ).toEqual({ ok: false, reason: 'conflict' });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.run('run-2').queryKey,
    });
  });

  it('keeps a Scorer label it gave, and reads the calibration again', async () => {
    const client = new QueryClient();
    const calibration = {
      agreed: 0,
      agreement: null,
      calibrated: false,
      labelled: 0,
      needed: 5,
      threshold: 0.8,
    };
    const score = (scorer: string): RunScore => ({
      scorer,
      score: 0.4,
      passed: false,
      reason: 'Too vague',
      model: 'judge-small',
      judgedAt: '2026-11-16T10:00:00Z',
      byCurrentJudge: true,
      calibration,
      label: null,
    });
    const key = factoryRunKeys.runScores('run-2').queryKey;
    client.setQueryData(key, {
      scores: [score('clear'), score('correct')],
      pending: [],
    });
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    const label = { passed: true, labelledAt: '2026-11-17T10:00:00Z' };
    vi.mocked(factoryClient.labelRunScore).mockResolvedValue(ok(label));

    expect(
      await createAppRunsContext(client).labelRunScore({
        runId: 'run-2',
        scorer: 'clear',
        passed: true,
      })
    ).toEqual({ ok: true, value: undefined });

    expect(factoryClient.labelRunScore).toHaveBeenCalledWith(
      'run-2',
      'clear',
      true
    );
    expect(
      client.getQueryData<RunScores>(key)?.scores.map((read) => read.label)
    ).toEqual([label, null]);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: key });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: factoryRunKeys.evaluation._def,
    });
  });

  it('reads the Runs at a Gate and those yet to reach one as the Queue', async () => {
    const atGate: RunDetail = {
      ...started,
      run: { ...started.run, id: 'run-3', request: 'Plan the export' },
    };
    vi.mocked(factoryClient.listWaitingRuns).mockResolvedValue(
      ok({ runs: [atGate], starting: [started] })
    );
    const client = new QueryClient();

    const queue = await client.fetchQuery(waitingRunsOptions(client));

    expect(queue.waiting.map((run) => run.id)).toEqual(['run-3']);
    expect(queue.starting.map((run) => run.id)).toEqual(['run-2']);
    // Each opens from the list with what it already read.
    expect(client.getQueryData(runOptions('run-2').queryKey)?.request).toBe(
      'Add a due date column'
    );
  });

  it('keeps a Run’s known token use when a list seeds it again', () => {
    const client = new QueryClient();
    const run: Run = {
      id: 'run-9',
      request: 'Plan it',
      createdAt: '2026-11-16T09:00:00Z',
      steps: [],
    };
    const key = factoryRunKeys.run('run-9').queryKey;
    client.setQueryData(key, {
      ...run,
      tokenBudget: { budget: 100_000, used: 42_000, measured: true },
    });

    // A list reads the budget, not the use.
    seedRun(client, { ...run, tokenBudget: { budget: 100_000 } });
    expect(client.getQueryData<Run>(key)?.tokenBudget).toEqual({
      budget: 100_000,
      used: 42_000,
      measured: true,
    });

    // The Run's own read replaces it, and a Run with no budget has none.
    seedRun(client, { ...run, tokenBudget: { budget: 100_000, used: 50_000 } });
    expect(client.getQueryData<Run>(key)?.tokenBudget?.used).toBe(50_000);
    seedRun(client, run);
    expect(client.getQueryData<Run>(key)?.tokenBudget).toBeUndefined();
  });

  it('reads a Run’s break records, and keeps them when a list seeds it again', async () => {
    const client = new QueryClient();
    vi.mocked(factoryClient.getRun).mockResolvedValue(
      ok({
        ...started,
        breakRecords: [
          {
            seq: 1,
            test: 'parses a due date',
            file: 'src/due.ts',
            oldText: 'return date;',
            newText: 'return null;',
            command: 'bun test src/due.test.ts',
            status: 'verified',
            checkedCommit: null,
            output: '1 failed',
            recordedAt: '2026-11-16T09:05:00Z',
          },
        ],
      })
    );

    const run = await client.fetchQuery(runOptions('run-2'));
    const record = {
      seq: 1,
      test: 'parses a due date',
      file: 'src/due.ts',
      oldText: 'return date;',
      newText: 'return null;',
      status: 'verified',
      checkedCommit: undefined,
      output: '1 failed',
    };
    expect(run.breakRecords).toEqual([record]);

    // A list reads no break records: the Run's own read keeps its.
    seedRun(client, { ...run, breakRecords: [] });
    expect(
      client.getQueryData<Run>(runOptions('run-2').queryKey)?.breakRecords
    ).toEqual([record]);
  });

  it('reads where each Step ran and when the Run merged, none when not sent', async () => {
    const client = new QueryClient();
    const [step] = started.steps;
    if (!step) throw new Error('No Step');
    // Hand-written until the client is regenerated (pending-api-fields.ts).
    const detail = {
      ...started,
      run: { ...started.run, mergedAt: '2026-11-16T10:00:00Z' },
      steps: [
        {
          ...step,
          runtime: { model: 'claude-opus-4-5', machine: 'm4', agentName: null },
        },
        { ...step, seq: 2, runtime: null },
        { ...step, seq: 3 },
      ],
    } as RunDetail;
    vi.mocked(factoryClient.getRun).mockResolvedValue(ok(detail));

    const run = await client.fetchQuery(runOptions('run-2'));

    expect(run.mergedAt).toBe('2026-11-16T10:00:00Z');
    expect(run.steps.map((read) => read.runtime)).toEqual([
      { model: 'claude-opus-4-5', machine: 'm4', agentName: undefined },
      undefined,
      undefined,
    ]);

    vi.mocked(factoryClient.getRun).mockResolvedValue(ok(started));
    const unmerged = await client.fetchQuery(runOptions('run-3'));
    expect(unmerged.mergedAt).toBeUndefined();
  });

  it('stops reading a Run again once it has ended', async () => {
    vi.useFakeTimers();
    try {
      const running: RunDetail = { ...started };
      const ended: RunDetail = {
        ...started,
        run: {
          ...started.run,
          end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
        },
      };
      vi.mocked(factoryClient.getRun)
        .mockResolvedValueOnce(ok(running))
        .mockResolvedValue(ok(ended));
      const client = new QueryClient();
      const observer = new QueryObserver(client, runOptions('run-2'));
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(3_000);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.getRun).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops reading a task once its Runs end, and starts again for a new one', async () => {
    vi.useFakeTimers();
    try {
      const ended: RunDetail = {
        ...started,
        run: {
          ...started.run,
          end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
        },
      };
      vi.mocked(factoryClient.listTaskRuns).mockResolvedValue(ok([ended]));
      const client = new QueryClient();
      const observer = new QueryObserver(
        client,
        taskRunsOptions(client, 'task-1')
      );
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(120_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(1);

      vi.mocked(factoryClient.startRun).mockResolvedValue(ok(started));
      await createAppRunsContext(client).startRun({
        taskId: 'task-1',
        flow: 'plan',
        request: 'Add a due date column',
      });
      await vi.advanceTimersByTimeAsync(3_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reads a task whose live Runs all wait on a Gate only slowly', async () => {
    vi.useFakeTimers();
    try {
      const waiting: RunDetail = {
        ...started,
        steps: [
          {
            terminal: { betweenTurns: false, steers: [] },
            seq: 1,
            entry: 'plan',
            visit: 1,
            startedAt: '2026-11-16T09:00:01Z',
            passedOver: [],
            end: {
              outcome: 'done',
              output: 'A plan.',
              endedAt: '2026-11-16T09:01:00Z',
            },
            gate: {
              kind: 'flow',
              question: 'Approve this plan?',
              answers: ['approve'],
              openedAt: '2026-11-16T09:01:00Z',
              answer: null,
            },
            checks: [],
          },
        ],
      };
      vi.mocked(factoryClient.listTaskRuns).mockResolvedValue(ok([waiting]));
      const client = new QueryClient();
      const observer = new QueryObserver(
        client,
        taskRunsOptions(client, 'task-1')
      );
      const unsubscribe = observer.subscribe(() => {});

      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(factoryClient.listTaskRuns).toHaveBeenCalledTimes(2);
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reads the Run again before reporting a refused answer', async () => {
    const client = new QueryClient();
    const observer = new QueryObserver(client, runOptions('run-2'));
    const unsubscribe = observer.subscribe(() => {});
    vi.mocked(factoryClient.getRun).mockResolvedValue(ok(started));
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().isSuccess).toBe(true)
    );
    const readsBefore = vi.mocked(factoryClient.getRun).mock.calls.length;
    vi.mocked(factoryClient.answerGate).mockResolvedValue(
      err([{ code: 'CONFLICT', message: 'Resource conflict' }])
    );

    const result = await createAppRunsContext(client).answerGate({
      runId: 'run-2',
      step: { seq: 1 },
      answer: 'approve',
    });

    expect(result).toEqual({ ok: false, reason: 'conflict' });
    expect(vi.mocked(factoryClient.getRun).mock.calls.length).toBe(
      readsBefore + 1
    );
    expect(observer.getCurrentResult().isFetching).toBe(false);
    unsubscribe();
  });
});
