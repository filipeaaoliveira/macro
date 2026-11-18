import { t } from '@app/lib/i18n';
import { StaticMarkdown } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { channelTheme } from '@core/component/LexicalMarkdown/theme';
import { buildSimpleEntityUrl } from '@core/util/url';
import CaretDownIcon from '@icon/system/regular/caret-down.svg';
import { Badge, Button, Card, cn } from '@ui';
import { CollapseTransition } from '@ui/components/CollapseTransition';
import { createSignal, For, type JSX, Match, Show, Switch } from 'solid-js';
import { fellShort, flowWordLabel, flowWordText } from '../core/flow-words';
import {
  type BaseRequiredChecks,
  type CheckRound,
  findingCounts,
  findingsOf,
  isQuestion,
  type MainHold,
  type ReviewFinding,
  type ReviewRound,
  type RunStep,
  reviewsAfresh,
  roundStartingAt,
  WHOLE_CHANGE,
} from '../core/run';
import { RULE_NAMES } from '../core/standing-rules';

function stepLabel(step: Pick<RunStep, 'entry' | 'visit'>): string {
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
  /** Whether a Step's output starts open, e.g. the plan a Gate asks about. */
  outputOpen: (step: RunStep) => boolean;
  time: (value: string) => string;
}) {
  return (
    <ol class="-ml-2 flex flex-col" aria-label={t('factoryRuns.steps')}>
      <For each={props.steps}>
        {(step, index) => (
          <TimelineStep
            step={step}
            reviewRounds={props.reviewRounds ?? []}
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
          />
        )}
      </For>
    </ol>
  );
}

function TimelineStep(props: {
  step: RunStep;
  reviewRounds: ReviewRound[];
  steps: RunStep[];
  first: boolean;
  latest: boolean;
  mainHold?: MainHold;
  baseRequiredChecks?: BaseRequiredChecks;
  baseBranch?: string;
  defaultOpen: boolean;
  time: (value: string) => string;
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
              {t('factoryRuns.stepRunning')}
            </span>
          }
        >
          {(end) => (
            <Badge
              variant="outline"
              size="xs"
              class={cn(
                fellShort(end().outcome) && 'border-failure/30 text-failure'
              )}
            >
              {flowWordLabel(end().outcome)}
            </Badge>
          )}
        </Show>
        <span class="ml-auto shrink-0 text-xs tabular-nums text-ink-subtle">
          {props.time(props.step.end?.endedAt ?? props.step.startedAt)}
        </span>
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
                  {t('factoryRuns.passedOver', {
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
      <For each={rounds()}>
        {(round) => (
          <CheckRoundNote round={round} stepEnded={Boolean(props.step.end)} />
        )}
      </For>
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

/**
 * A round of checks: which it ran, who ran them, and how they went. A round
 * the factory ran on the pushed commit names each check that failed with its
 * exit code. A round with no end runs only while its Step does.
 */
function CheckRoundNote(props: { round: CheckRound; stepEnded: boolean }) {
  const checks = () =>
    props.round.checks.map((name) => {
      const result = props.round.results.find((ran) => ran.name === name);
      return result && result.exitCode !== 0
        ? t('factoryRuns.checkExited', { check: name, code: result.exitCode })
        : name;
    });

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
  return t('factoryRuns.findingFound');
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
