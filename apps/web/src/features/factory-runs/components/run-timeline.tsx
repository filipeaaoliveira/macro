import { t } from '@app/lib/i18n';
import { StaticMarkdown } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { channelTheme } from '@core/component/LexicalMarkdown/theme';
import { buildSimpleEntityUrl } from '@core/util/url';
import CaretDownIcon from '@icon/system/regular/caret-down.svg';
import { Badge, Button, Card, cn } from '@ui';
import { CollapseTransition } from '@ui/components/CollapseTransition';
import { createSignal, For, type JSX, Match, Show, Switch } from 'solid-js';
import { flowWordLabel, flowWordText, outcomeTone } from '../core/flow-words';
import {
  type BaseRequiredChecks,
  type BreakRecord,
  type BreakStatus,
  breakRecordsOf,
  type CheckRound,
  type FiledTask,
  findingCounts,
  findingsOf,
  isBlockingBreak,
  isQuestion,
  type MainHold,
  type ReviewFinding,
  type ReviewRound,
  type RunStep,
  reviewsAfresh,
  roundStartingAt,
  type Steer,
  type SteerAction,
  splitChildTasks,
  steerableSession,
  terminalState,
  USAGE_LIMIT_REASON,
  WHOLE_CHANGE,
} from '../core/run';
import { RULE_NAMES } from '../core/standing-rules';

/** The classes of a Step's outcome badge, by its tone. */
const OUTCOME_TONES = {
  short: 'border-failure/30 text-failure',
  sentBack: 'border-warning/40 text-warning-ink',
  plain: undefined,
} as const;

export function stepLabel(step: Pick<RunStep, 'entry' | 'visit'>): string {
  const entry = flowWordLabel(step.entry);
  if (step.visit <= 1) return entry;
  return t('factoryRuns.stepVisit', { entry, visit: step.visit });
}

/**
 * A Run's Steps, oldest first, on a rail like an agent's activity timeline.
 * Each Step shows its outcome and the rounds of checks it ran; a review
 * round shows on its first reviewer's Step, and each reviewer's findings on
 * its own. A Step's output (an agent's plan, a review) and the checks'
 * reports open below it as markdown.
 */
export function RunTimeline(props: {
  steps: RunStep[];
  /** The hold on the Run's base branch, shown on its latest Step. */
  mainHold?: MainHold;
  /** Why the base's required checks hold nothing, likewise. */
  baseRequiredChecks?: BaseRequiredChecks;
  baseBranch?: string;
  reviewRounds?: ReviewRound[];
  /** The breaks its write and fix Steps reported, each under its Step. */
  breakRecords?: BreakRecord[];
  /** Whether a Step's output starts open, e.g. the plan a Gate asks about. */
  outputOpen: (step: RunStep) => boolean;
  time: (value: string) => string;
  /** Opens a running Step's Step Terminal; none where it can't open. */
  onWatch?: (step: RunStep) => void;
}) {
  return (
    <ol class="-ml-2 flex flex-col" aria-label={t('factoryRuns.steps')}>
      <For each={props.steps}>
        {(step, index) => (
          <TimelineStep
            step={step}
            reviewRounds={props.reviewRounds ?? []}
            breakRecords={props.breakRecords ?? []}
            steps={props.steps}
            first={index() === 0}
            latest={index() === props.steps.length - 1}
            mainHold={
              index() === props.steps.length - 1 ? props.mainHold : undefined
            }
            baseRequiredChecks={
              index() === props.steps.length - 1
                ? props.baseRequiredChecks
                : undefined
            }
            baseBranch={props.baseBranch}
            defaultOpen={props.outputOpen(step)}
            time={props.time}
            onWatch={props.onWatch}
          />
        )}
      </For>
    </ol>
  );
}

function TimelineStep(props: {
  step: RunStep;
  reviewRounds: ReviewRound[];
  breakRecords: BreakRecord[];
  steps: RunStep[];
  first: boolean;
  latest: boolean;
  mainHold?: MainHold;
  baseRequiredChecks?: BaseRequiredChecks;
  baseBranch?: string;
  defaultOpen: boolean;
  time: (value: string) => string;
  onWatch?: (step: RunStep) => void;
}) {
  // Follows `defaultOpen` until the person toggles it: a Step that ends at a
  // Gate while the dialog is open shows the output the Gate asks about.
  const [toggled, setToggled] = createSignal<boolean>();
  const open = () => toggled() ?? props.defaultOpen;
  const output = () => props.step.end?.output.trim() ?? '';
  const answer = () => props.step.gate?.answer;
  const rounds = () => props.step.checks ?? [];
  const reports = () => rounds().filter((round) => round.end?.report.trim());
  const details = () => output() !== '' || reports().length > 0;
  const round = () => roundStartingAt(props.reviewRounds, props.step.seq);
  const findings = () => findingsOf(props.reviewRounds, props.step.seq);
  const breaks = () => breakRecordsOf(props.breakRecords, props.step.seq);

  return (
    // Rail and dot sit where the agent activity timeline puts them
    // (`ActivityStep`), centred on the 28px heading row (36px on touch).
    <li class="relative pl-8" data-factory-run-step={props.step.seq}>
      <span
        aria-hidden="true"
        class={cn(
          'absolute left-[15.5px] w-px bg-ink-placeholder/45',
          props.first ? 'top-3.5 touch:top-4.5' : 'top-0',
          props.latest ? 'h-3.5 touch:h-4.5' : 'bottom-0',
          // A lone Step has no rail to draw.
          props.first && props.latest && 'hidden'
        )}
      />
      <span
        aria-hidden="true"
        class={cn(
          'absolute top-3.5 left-[12.5px] size-[7px] -translate-y-1/2 rounded-full border-[1.5px] touch:top-4.5',
          props.step.end
            ? 'border-ink-subtle bg-ink-subtle'
            : 'border-accent bg-accent motion-safe:animate-pulse'
        )}
      />
      <div class="flex min-h-7 min-w-0 items-center gap-2 text-sm touch:min-h-9">
        <span class="min-w-0 truncate font-medium text-ink">
          {stepLabel(props.step)}
        </span>
        <Show
          when={props.step.end}
          fallback={
            <span class="text-xs text-ink-subtle">
              <Switch fallback={t('factoryRuns.stepRunning')}>
                <Match when={terminalState(props.step) === 'paused'}>
                  {t('factoryRuns.stepPaused')}
                </Match>
                <Match when={terminalState(props.step) === 'stopping'}>
                  {t('factoryRuns.stepStopping')}
                </Match>
              </Switch>
            </span>
          }
        >
          {(end) => (
            <Badge
              variant="outline"
              size="xs"
              data-outcome-tone={outcomeTone(
                end().outcome,
                props.step.gate?.answer?.answer
              )}
              class={cn(
                OUTCOME_TONES[
                  outcomeTone(end().outcome, props.step.gate?.answer?.answer) ??
                    'plain'
                ]
              )}
            >
              {flowWordLabel(end().outcome)}
            </Badge>
          )}
        </Show>
        <span class="ml-auto shrink-0 text-xs tabular-nums text-ink-subtle">
          {props.time(props.step.end?.endedAt ?? props.step.startedAt)}
        </span>
        <Show when={props.onWatch && steerableSession(props.step)}>
          <Button
            type="button"
            variant="ghost"
            size="xs"
            aria-label={t('factoryRuns.watchStepLabel', {
              step: stepLabel(props.step),
            })}
            onClick={() => props.onWatch?.(props.step)}
          >
            {t('factoryRuns.watchStep')}
          </Button>
        </Show>
        <Show
          when={details()}
          fallback={
            // Keeps the time column in line with Steps that have output.
            <span aria-hidden="true" class="size-6 shrink-0 touch:size-9" />
          }
        >
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-expanded={open()}
            aria-label={t('factoryRuns.showOutput', {
              step: stepLabel(props.step),
            })}
            onClick={() => setToggled(!open())}
          >
            <CaretDownIcon
              class={cn(
                'size-3 touch:size-6 shrink-0 transition-transform duration-150 motion-reduce:transition-none',
                open() && 'rotate-180'
              )}
            />
          </Button>
        </Show>
      </div>
      <Show when={props.step.agent}>
        {(agent) => (
          <StepNote>
            {/* An Agent passed over is the one that took the Step only when
                none could start it. */}
            {props.step.passedOver?.some((passed) => passed.agent === agent())
              ? t('factoryRuns.stepNoAgent', {
                  role: flowWordLabel(props.step.role ?? ''),
                })
              : t('factoryRuns.stepAgent', {
                  role: flowWordLabel(props.step.role ?? ''),
                  agent: agent(),
                })}
            <For each={props.step.passedOver}>
              {(passed) => (
                <span class="block text-ink-muted">
                  {/* An Agent at its usage limit did start: its first
                      turn ended before it did any work. */}
                  {passed.reason === USAGE_LIMIT_REASON
                    ? t('factoryRuns.passedOverUsageLimit', {
                        agent: passed.agent,
                      })
                    : t('factoryRuns.passedOver', {
                        agent: passed.agent,
                        reason: flowWordText(passed.reason),
                      })}
                </span>
              )}
            </For>
          </StepNote>
        )}
      </Show>
      <Show when={answer()}>
        {(answer) => (
          <StepNote>
            <Show
              when={!(props.step.gate && isQuestion(props.step.gate))}
              fallback={t('factoryRuns.questionAnswered')}
            >
              <Show
                when={answer().rule}
                fallback={
                  <Switch
                    fallback={t('factoryRuns.answered', {
                      answer: flowWordLabel(answer().answer),
                    })}
                  >
                    <Match when={answer().standing}>
                      {(standing) =>
                        t('factoryRuns.answeredByStandingAgent', {
                          answer: flowWordLabel(answer().answer),
                          agent: standing().agent,
                          role: standing().standingRole,
                        })
                      }
                    </Match>
                    <Match when={answer().byOrchestrator}>
                      {t('factoryRuns.answeredByOrchestrator', {
                        answer: flowWordLabel(answer().answer),
                      })}
                    </Match>
                    <Match when={answer().byStandIn}>
                      {t('factoryRuns.answeredByStandIn', {
                        answer: flowWordLabel(answer().answer),
                      })}
                    </Match>
                    <Match when={answer().replyConfidence}>
                      {(confidence) =>
                        t('factoryRuns.answeredFromReply', {
                          answer: flowWordLabel(answer().answer),
                          confidence: confidence(),
                        })
                      }
                    </Match>
                  </Switch>
                }
              >
                {(rule) =>
                  t('factoryRuns.answeredByRule', {
                    answer: flowWordLabel(answer().answer),
                    rule: t(RULE_NAMES[rule()]),
                  })
                }
              </Show>
            </Show>
            <Show when={answer().note}>
              {(note) => (
                <span class="block whitespace-pre-wrap text-ink-muted">
                  {note()}
                </span>
              )}
            </Show>
            <Show when={answer().byStandIn && props.step.gate?.standIn}>
              {(standIn) => (
                <span class="block whitespace-pre-wrap text-ink-muted">
                  {standIn().reason}
                </span>
              )}
            </Show>
          </StepNote>
        )}
      </Show>
      <Show when={splitChildTasks(props.step).length > 0}>
        <ChildTasksNote tasks={splitChildTasks(props.step)} />
      </Show>
      <For each={props.step.terminal?.steers}>
        {(steer) => <SteerNote steer={steer} />}
      </For>
      <For each={rounds()}>
        {(round) => (
          <CheckRoundNote round={round} stepEnded={Boolean(props.step.end)} />
        )}
      </For>
      <Show when={breaks().length > 0}>
        <BreakRecordsNote records={breaks()} />
      </Show>
      <Show when={round()}>
        {(round) => (
          <ReviewRoundNote
            round={round()}
            afresh={reviewsAfresh(props.reviewRounds, props.steps, round())}
          />
        )}
      </Show>
      <For each={findings()}>
        {(finding) => <FindingNote finding={finding} />}
      </For>
      <Show when={props.mainHold}>
        {(hold) => (
          <MainHoldNote hold={hold()} baseBranch={props.baseBranch ?? ''} />
        )}
      </Show>
      <Show when={props.baseRequiredChecks}>
        {(why) => (
          <StepNote>
            {why() === 'none'
              ? t('factoryRuns.baseChecksNone', {
                  branch: props.baseBranch ?? '',
                })
              : t('factoryRuns.baseChecksUnread', {
                  branch: props.baseBranch ?? '',
                })}
          </StepNote>
        )}
      </Show>
      <Show when={details()}>
        <CollapseTransition open={open()}>
          <Card variant="filled" offset={1} class="mt-1 mb-3 px-3 py-2.5">
            <Show when={output()}>
              <StepMarkdown markdown={output()} />
            </Show>
            <For each={reports()}>
              {(round, index) => (
                <div class={cn((output() || index() > 0) && 'mt-3')}>
                  <p class="mb-1 text-xs font-medium text-ink-subtle">
                    {t('factoryRuns.checksRound', { round: round.round })}
                  </p>
                  <StepMarkdown markdown={round.end?.report.trim() ?? ''} />
                </div>
              )}
            </For>
          </Card>
        </CollapseTransition>
      </Show>
    </li>
  );
}

/** Agent prose as its messages render it, at the block's size. */
function StepMarkdown(props: { markdown: string }) {
  return (
    <div class="chat-markdown-container max-w-full text-sm whitespace-pre-wrap wrap-break-word">
      <StaticMarkdown
        markdown={props.markdown}
        theme={channelTheme}
        target="internal"
        lazy={false}
      />
    </div>
  );
}

/** What begins the name of a setup command's run among a round's results. */
const SETUP_RESULT = 'setup:';

/**
 * A round of checks: which it ran, who ran them, and how they went. A round
 * the factory ran on the pushed commit names each check that failed with its
 * exit code, and first each command of the repository's setup that failed
 * there, before any check ran. A round with no end runs only while its Step
 * does.
 */
function CheckRoundNote(props: { round: CheckRound; stepEnded: boolean }) {
  const checks = () => [
    ...props.round.results
      .filter((ran) => ran.name.startsWith(SETUP_RESULT) && ran.exitCode !== 0)
      .map((ran) =>
        t('factoryRuns.setupExited', {
          command: ran.name.slice(SETUP_RESULT.length),
          code: ran.exitCode,
        })
      ),
    ...props.round.checks.map((name) => {
      const result = props.round.results.find((ran) => ran.name === name);
      return result && result.exitCode !== 0
        ? t('factoryRuns.checkExited', { check: name, code: result.exitCode })
        : name;
    }),
  ];

  // Who ran the round's checks: the factory on the pushed commit, or the
  // agent, which is known only once its report ends the round.
  const who = () => {
    if (props.round.verified) {
      const commit = props.round.checkedCommit;
      return commit
        ? t('factoryRuns.checksVerifiedOn', { commit: commit.slice(0, 7) })
        : t('factoryRuns.checksVerified');
    }
    if (props.round.end) return t('factoryRuns.checksReported');
    return undefined;
  };

  return (
    <StepNote>
      <span class="flex min-w-0 items-center gap-2">
        {t('factoryRuns.checksRound', { round: props.round.round })}
        <Show when={who()}>{(who) => <span>{who()}</span>}</Show>
        <Show
          when={props.round.end}
          fallback={
            <span>
              {props.stepEnded
                ? t('factoryRuns.checksUnfinished')
                : t('factoryRuns.checksRunning')}
            </span>
          }
        >
          {(end) => (
            <Badge variant="outline" size="xs">
              {flowWordLabel(end().outcome)}
            </Badge>
          )}
        </Show>
      </span>
      <span class="block text-ink-muted">{checks().join(', ')}</span>
    </StepNote>
  );
}

const BREAK_STATUSES = {
  verified: 'factoryRuns.breakVerified',
  not_reproduced: 'factoryRuns.breakNotReproduced',
  hung: 'factoryRuns.breakHung',
  not_run: 'factoryRuns.breakNotRun',
  not_applied: 'factoryRuns.breakNotApplied',
  unreadable: 'factoryRuns.breakUnreadable',
  dropped: 'factoryRuns.breakDropped',
  unchecked: 'factoryRuns.breakUnchecked',
  interrupted: 'factoryRuns.breakInterrupted',
} as const satisfies Record<BreakStatus, string>;

/**
 * The breaks a write or fix Step reported for the tests it added: each
 * test, how the factory's run of its break went (in the warning tone when
 * the test is left unproven), and the edit the break makes.
 */
function BreakRecordsNote(props: { records: BreakRecord[] }) {
  return (
    <section data-break-records aria-label={t('factoryRuns.breakRecords')}>
      <StepNote>{t('factoryRuns.breakRecords')}</StepNote>
      <For each={props.records}>
        {(record) => (
          <StepNote>
            <span class="flex min-w-0 items-center gap-2">
              <Badge
                variant="outline"
                size="xs"
                data-break-status={record.status}
                data-break-tone={
                  isBlockingBreak(record.status) ? 'warning' : undefined
                }
                class={cn(
                  'shrink-0',
                  isBlockingBreak(record.status) && OUTCOME_TONES.sentBack
                )}
              >
                {t(BREAK_STATUSES[record.status])}
              </Badge>
              <span class="min-w-0 truncate font-mono">{record.test}</span>
            </span>
            <span class="block font-mono text-ink-muted">{record.file}</span>
            <span class="block whitespace-pre-wrap font-mono text-ink-muted">
              {t('factoryRuns.breakOldText', { text: record.oldText })}
            </span>
            <span class="block whitespace-pre-wrap font-mono text-ink-muted">
              {t('factoryRuns.breakNewText', { text: record.newText })}
            </span>
          </StepNote>
        )}
      </For>
    </section>
  );
}

/**
 * The Run's base branch is red: the Run merges nothing until its checks
 * pass, even once its merge is approved.
 */
function MainHoldNote(props: { hold: MainHold; baseBranch: string }) {
  return (
    <StepNote>
      <span class="flex min-w-0 items-center gap-2">
        <Badge variant="outline" size="xs">
          {t('factoryRuns.mainHold')}
        </Badge>
        {t('factoryRuns.mainHoldBody', { branch: props.baseBranch })}
      </span>
      <span class="block whitespace-pre-wrap text-ink-muted">
        {props.hold.failed}
      </span>
      <Show when={props.hold.taskId}>
        {(taskId) => (
          <a
            class="block text-ink-muted underline"
            href={buildSimpleEntityUrl({ type: 'task', id: taskId() })}
          >
            {t('factoryRuns.mainHoldTask')}
          </a>
        )}
      </Show>
    </StepNote>
  );
}

/**
 * A Run's review rounds, oldest first, each with what its reviewers found,
 * as the Run's timeline shows them under their Steps: for a view that
 * shows the review apart from the Steps, such as a pull request's.
 */
export function ReviewRoundList(props: {
  rounds: ReviewRound[];
  steps: Pick<RunStep, 'seq' | 'entry'>[];
}) {
  return (
    <For each={props.rounds}>
      {(round) => (
        <>
          <ReviewRoundNote
            round={round}
            afresh={reviewsAfresh(props.rounds, props.steps, round)}
          />
          <For each={round.findings}>
            {(finding) => <FindingNote finding={finding} />}
          </For>
        </>
      )}
    </For>
  );
}

/**
 * A review round: which of the Run's (a fresh review when its entry was
 * reset, as when the change was sent back), the diff it reviews, and once
 * it ended, how, with how many of its findings block and how many became
 * follow-ups.
 */
function ReviewRoundNote(props: { round: ReviewRound; afresh: boolean }) {
  const counts = () => findingCounts(props.round);
  return (
    <StepNote>
      <span class="flex min-w-0 items-center gap-2">
        {props.afresh
          ? t('factoryRuns.reviewRoundAfresh', { round: props.round.round })
          : t('factoryRuns.reviewRound', { round: props.round.round })}
        <Show
          when={props.round.end}
          fallback={<span>{t('factoryRuns.reviewRoundReviewing')}</span>}
        >
          {(end) => (
            <Badge variant="outline" size="xs">
              {flowWordLabel(end().outcome)}
            </Badge>
          )}
        </Show>
      </span>
      <span class="block text-ink-muted">
        <Show
          when={props.round.failure}
          fallback={
            <>
              {props.round.fixDiff
                ? t('factoryRuns.reviewRoundFixDiff', {
                    commit: (props.round.diffBase ?? '').slice(0, 7),
                  })
                : t('factoryRuns.reviewRoundWhole')}
              <Show when={props.round.end}>
                {' · '}
                {t('factoryRuns.reviewBlockers', { count: counts().blockers })}
                {' · '}
                {t('factoryRuns.reviewFollowUps', {
                  count: counts().followUps,
                })}
              </Show>
            </>
          }
        >
          {(failure) =>
            t('factoryRuns.reviewRoundNotOpened', { failure: failure() })
          }
        </Show>
      </span>
    </StepNote>
  );
}

/** Where a finding is: its file, and its lines when it names some. */
function findingPlace(finding: ReviewFinding): string {
  if (finding.path === WHOLE_CHANGE) return t('factoryRuns.findingWholeChange');
  const lines = finding.lines;
  if (!lines) return finding.path;
  if (lines.start === lines.end) {
    return t('factoryRuns.findingLine', {
      path: finding.path,
      line: lines.start,
    });
  }
  return t('factoryRuns.findingLines', {
    path: finding.path,
    start: lines.start,
    end: lines.end,
  });
}

function findingKind(finding: ReviewFinding): string {
  if (finding.kind === 'blocker') return t('factoryRuns.findingBlocker');
  if (finding.kind === 'follow_up') {
    return finding.followUpTaskId
      ? t('factoryRuns.findingFollowUpTask')
      : t('factoryRuns.findingFollowUp');
  }
  if (finding.kind === 'dismissed') return t('factoryRuns.findingDismissed');
  return t('factoryRuns.findingFound');
}

/** What the fix said of a blocker, and how the next round ruled on it. */
function findingResolution(finding: ReviewFinding): string[] {
  if (finding.fixStatus === 'fixed') return [t('factoryRuns.findingFixed')];
  if (finding.fixStatus !== 'disputed') return [];
  const disputed = t('factoryRuns.findingDisputed', {
    reason: finding.dispute ?? '',
  });
  if (finding.ruling === 'upheld') {
    return [disputed, t('factoryRuns.findingDisputeUpheld')];
  }
  if (finding.ruling === 'dropped') {
    return [disputed, t('factoryRuns.findingDisputeDropped')];
  }
  return [disputed];
}

/**
 * One finding of a reviewer: what it is (a follow-up's badge opens its
 * task), where, what is wrong and why.
 */
function FindingNote(props: { finding: ReviewFinding }) {
  return (
    <StepNote>
      <span class="flex min-w-0 items-center gap-2">
        <Show
          when={props.finding.followUpTaskId}
          fallback={
            <Badge variant="outline" size="xs">
              {findingKind(props.finding)}
            </Badge>
          }
        >
          {(taskId) => (
            <a
              href={buildSimpleEntityUrl({ type: 'task', id: taskId() })}
              class="shrink-0"
            >
              <Badge variant="outline" size="xs">
                {findingKind(props.finding)}
              </Badge>
            </a>
          )}
        </Show>
        <span class="min-w-0 truncate font-mono">
          {findingPlace(props.finding)}
        </span>
      </span>
      <span class="block text-ink-muted">{props.finding.problem}</span>
      <span class="block text-ink-muted">{props.finding.why}</span>
      <Show
        when={
          props.finding.blocksAnywhere && props.finding.path !== WHOLE_CHANGE
        }
      >
        <span class="block text-ink-muted">
          {t('factoryRuns.findingFoundAgain')}
        </span>
      </Show>
      <Show when={props.finding.kind === 'dismissed'}>
        <span class="block text-ink-muted">
          {t('factoryRuns.findingDismissedNote')}
        </span>
      </Show>
      <For each={findingResolution(props.finding)}>
        {(line) => (
          <span class="block whitespace-pre-wrap text-ink-muted">{line}</span>
        )}
      </For>
    </StepNote>
  );
}

/** The child tasks a split filed, each opening its task. */
function ChildTasksNote(props: { tasks: FiledTask[] }) {
  return (
    <StepNote>
      {t('factoryRuns.splitChildTasks', { count: props.tasks.length })}
      <For each={props.tasks}>
        {(task) => (
          <a
            data-split-child-task
            class="block text-ink-muted underline"
            href={buildSimpleEntityUrl({ type: 'task', id: task.id })}
          >
            {task.name}
          </a>
        )}
      </For>
    </StepNote>
  );
}

const STEERS = {
  note: 'factoryRuns.steerNote',
  pause: 'factoryRuns.steerPause',
  resume: 'factoryRuns.steerResume',
  stop: 'factoryRuns.steerStop',
} as const satisfies Record<SteerAction, string>;

/** What the Run's owner did to the Step from its Step Terminal. */
function SteerNote(props: { steer: Steer }) {
  return (
    <StepNote>
      {t(STEERS[props.steer.action])}
      <Show when={props.steer.undelivered}>
        {' · '}
        {t('factoryRuns.steerUndelivered')}
      </Show>
      <Show when={props.steer.note}>
        {(note) => (
          <span class="block whitespace-pre-wrap text-ink-muted">{note()}</span>
        )}
      </Show>
    </StepNote>
  );
}

function StepNote(props: { children: JSX.Element }) {
  return (
    <p class="mb-2 min-w-0 text-xs leading-5 text-ink-subtle [overflow-wrap:anywhere]">
      {props.children}
    </p>
  );
}
