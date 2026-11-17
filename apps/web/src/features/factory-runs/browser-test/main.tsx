import { SidePanel } from '@components/app/side-panel';
import { ToastRegion } from '@core/component/Toast/ToastRegion';
import { UserContextProvider } from '@core/context/user';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import { queryClient } from '@queries/client';
import { QueryClientProvider } from '@tanstack/solid-query';
import { ImperativeDialogHost } from '@ui';
import { type Accessor, createSignal } from 'solid-js';
import { render } from 'solid-js/web';
import '@fontsource-variable/inter';
import '../../../index.css';
import {
  type AnswerGateInput,
  type RunActionResult,
  type RunsContext,
  RunsProvider,
} from '../context/runs-context';
import type { Run } from '../core/run';
import { TaskRunsSection } from '../views/task-runs-section';

// `touch:` styles key on this attribute, as the app shell sets it.
document.documentElement.dataset.touchDevice = String(isTouchDevice());

const MINUTE = 60_000;

// `?long-question` asks a Gate question long enough to fill a phone.
const LONG_QUESTION = new URLSearchParams(location.search).has('long-question');
const QUESTION = LONG_QUESTION
  ? Array.from(
      { length: 14 },
      (_, line) =>
        `${line + 1}. Check that the plan covers this case before it goes on.`
    ).join('\n')
  : 'Approve this plan, or send it back with a note on what to change?';
const at = (minutesAgo: number) =>
  new Date(Date.now() - minutesAgo * MINUTE).toISOString();

const PLAN = `Add a **Due date** column to the task list.

1. Read the due date property in \`use-tasks-query.ts\` and sort on it.
2. Render it in the list row beside Assignees, with the calendar icon.
3. Cover the empty state with the slashed calendar.`;

// A Run waiting on a person, one still running, and two that ended.
const INITIAL_RUNS: Run[] = [
  {
    id: 'run-waiting',
    request: 'Add a due date column to the task list\n\nSort by it too.',
    createdAt: at(12),
    taskId: 'task-1',
    steps: [
      {
        seq: 1,
        entry: 'plan',
        visit: 1,
        startedAt: at(12),
        end: { outcome: 'done', output: 'A first plan.', endedAt: at(10) },
        gate: {
          question:
            'Approve this plan, or send it back with a note on what to change?',
          answers: ['approve', 'revise'],
          openedAt: at(10),
          answer: {
            answer: 'revise',
            note: 'Keep the column narrow.',
            answeredAt: at(8),
          },
        },
      },
      {
        seq: 2,
        entry: 'plan',
        visit: 2,
        startedAt: at(8),
        end: { outcome: 'done', output: PLAN, endedAt: at(6) },
        gate: {
          question: QUESTION,
          answers: ['approve', 'revise'],
          openedAt: at(6),
        },
      },
    ],
  },
  {
    id: 'run-running',
    request: 'Fix the empty state of the board',
    createdAt: at(40),
    taskId: 'task-1',
    steps: [
      {
        seq: 1,
        entry: 'plan',
        visit: 1,
        startedAt: at(40),
        end: {
          outcome: 'done',
          output: 'Use the shared empty state.',
          endedAt: at(38),
        },
      },
      {
        seq: 2,
        entry: 'write',
        visit: 1,
        startedAt: at(38),
        end: { outcome: 'done', output: 'Changed two files.', endedAt: at(30) },
        checks: [
          {
            round: 1,
            checks: ['web', 'lint'],
            startedAt: at(36),
            end: {
              outcome: 'failed',
              report:
                '- `web`: `bun run check`, exit 1: `EmptyState` has no `title` prop.\n- `lint`: `just check`, exit 0',
              endedAt: at(34),
            },
          },
          {
            round: 2,
            checks: ['web', 'lint'],
            startedAt: at(32),
            end: {
              outcome: 'passed',
              report:
                '- `web`: `bun run check`, exit 0\n- `lint`: `just check`, exit 0',
              endedAt: at(31),
            },
          },
        ],
      },
      { seq: 3, entry: 'review', visit: 1, startedAt: at(30) },
    ],
  },
  {
    id: 'run-done',
    request: 'Rename the Updated column',
    createdAt: at(60 * 26),
    taskId: 'task-1',
    end: { outcome: 'success', endedAt: at(60 * 25) },
    steps: [
      {
        seq: 1,
        entry: 'write',
        visit: 1,
        startedAt: at(60 * 26),
        end: { outcome: 'done', output: 'Renamed it.', endedAt: at(60 * 25) },
      },
    ],
  },
  {
    id: 'run-cancelled',
    request: 'Try a denser row layout',
    createdAt: at(60 * 24 * 3),
    taskId: 'task-1',
    end: { outcome: 'cancelled', endedAt: at(60 * 24 * 3 - 5) },
    steps: [],
  },
];

/** A factory in memory: Runs move on when a Gate is answered or a Run ends. */
function createFixtureContext(): RunsContext {
  const [runs, setRuns] = createSignal(INITIAL_RUNS);
  const update = (id: string, change: (run: Run) => Run) =>
    setRuns((current) =>
      current.map((run) => (run.id === id ? change(run) : run))
    );
  const ready = <T,>(data: Accessor<T>) => ({
    state: () => 'ready' as const,
    data,
    retry: () => {},
  });

  /** Answers the Gate the Run waits on, as the service would. */
  const answer = (input: AnswerGateInput): RunActionResult => {
    const run = runs().find((candidate) => candidate.id === input.runId);
    const step = run?.steps.at(-1);
    // A second answer, or one to a Step the Run no longer waits on, like the
    // service's 409.
    if (!step?.gate || step.gate.answer || step.seq !== input.step.seq) {
      return { ok: false, reason: 'conflict' };
    }
    const now = new Date().toISOString();
    update(input.runId, (current) => ({
      ...current,
      steps: [
        ...current.steps.slice(0, -1),
        {
          ...step,
          gate: step.gate && {
            ...step.gate,
            answer: {
              answer: input.answer,
              note: input.note,
              answeredAt: now,
            },
          },
        },
        {
          seq: current.steps.length + 1,
          entry: input.answer === 'approve' ? 'write' : 'plan',
          visit: input.answer === 'approve' ? 1 : step.visit + 1,
          startedAt: now,
        },
      ],
    }));
    return { ok: true, value: undefined };
  };

  return {
    taskRuns: () => ready(runs),
    run: (runId) => ready(() => runs().find((run) => run.id === runId())),
    flows: () =>
      ready(() => [
        {
          name: 'plan',
          description:
            'Plans the change, asks you to approve the plan, then writes it.',
        },
        {
          name: 'code',
          description:
            'Takes the task to a merged pull request: plan, write, review, publish, CI, merge.',
        },
        {
          name: 'reply',
          description: 'Answers the request in one Step.',
        },
      ]),
    async startRun(input) {
      const run: Run = {
        id: `run-${runs().length + 1}`,
        request: input.request,
        createdAt: new Date().toISOString(),
        taskId: input.taskId,
        steps: [
          {
            seq: 1,
            entry: 'plan',
            visit: 1,
            startedAt: new Date().toISOString(),
          },
        ],
      };
      setRuns((current) => [run, ...current]);
      return { ok: true, value: run };
    },
    async answerGate(input) {
      // A note of "fail" stands in for a failed request; "slow-fail" for
      // one that fails after a moment, while the person does something else.
      if (input.note === 'fail') return { ok: false, reason: 'failed' };
      if (input.note === 'slow-fail') {
        await new Promise((resolve) => setTimeout(resolve, 200));
        return { ok: false, reason: 'failed' };
      }
      // A note of "conflict" stands in for someone else answering first.
      if (input.note === 'conflict') {
        // Their answer is already in when this one is refused, so the read
        // a 409 triggers shows it.
        answer({ ...input, note: undefined });
        return { ok: false, reason: 'conflict' };
      }
      return answer(input);
    },
    async cancelRun(runId) {
      const run = runs().find((candidate) => candidate.id === runId);
      if (run?.end) return { ok: false, reason: 'conflict' };
      // The service takes the cancel at once; a later read shows it ended.
      setTimeout(
        () =>
          update(runId, (current) => ({
            ...current,
            end: { outcome: 'cancelled', endedAt: new Date().toISOString() },
          })),
        500
      );
      return { ok: true, value: undefined };
    },
  };
}

function Fixture() {
  return (
    <main class="flex h-dvh flex-col bg-page p-3">
      <SidePanel.Root
        floating
        persistentFloating
        defaultOpen
        persistKey="factory-runs-test"
      >
        <SidePanel.Toggle />
        <div class="relative min-h-0 flex-1">
          <SidePanel.Layout headerToggle={false} floating persistentFloating>
            <SidePanel.Section
              id="properties"
              title="Properties"
              defaultOpen
              order={25}
            >
              <span class="text-xs text-ink-placeholder">Status, Priority</span>
            </SidePanel.Section>
            <RunsProvider value={createFixtureContext()}>
              <TaskRunsSection taskId="task-1" order={36} />
            </RunsProvider>
            <section class="size-full" aria-label="Task" />
          </SidePanel.Layout>
        </div>
      </SidePanel.Root>
      <ImperativeDialogHost />
      <ToastRegion />
    </main>
  );
}

render(
  () => (
    <QueryClientProvider client={queryClient}>
      <UserContextProvider>
        <Fixture />
      </UserContextProvider>
    </QueryClientProvider>
  ),
  document.getElementById('root')!
);
