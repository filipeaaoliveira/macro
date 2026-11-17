import { t } from '@app/lib/i18n';
import { StaticMarkdown } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { channelTheme } from '@core/component/LexicalMarkdown/theme';
import CaretDownIcon from '@icon/system/regular/caret-down.svg';
import { Badge, Button, Card, cn } from '@ui';
import { CollapseTransition } from '@ui/components/CollapseTransition';
import { createSignal, For, type JSX, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import type { CheckRound, RunStep } from '../core/run';

function stepLabel(step: Pick<RunStep, 'entry' | 'visit'>): string {
  const entry = flowWordLabel(step.entry);
  if (step.visit <= 1) return entry;
  return t('factoryRuns.stepVisit', { entry, visit: step.visit });
}

/**
 * A Run's Steps, oldest first, on a rail like an agent's activity timeline.
 * Each Step shows its outcome and the rounds of checks it ran; its output
 * (an agent's plan, a review) and the checks' reports open below it as
 * markdown.
 */
export function RunTimeline(props: {
  steps: RunStep[];
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
            first={index() === 0}
            latest={index() === props.steps.length - 1}
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
  first: boolean;
  latest: boolean;
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
            <Badge variant="outline" size="xs">
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
      <Show when={answer()}>
        {(answer) => (
          <StepNote>
            {t('factoryRuns.answered', {
              answer: flowWordLabel(answer().answer),
            })}
            <Show when={answer().note}>
              {(note) => (
                <span class="block whitespace-pre-wrap text-ink-muted">
                  {note()}
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
 * A round of checks: which it ran, and how they went. A round with no end
 * runs only while its Step does.
 */
function CheckRoundNote(props: { round: CheckRound; stepEnded: boolean }) {
  return (
    <StepNote>
      <span class="flex min-w-0 items-center gap-2">
        {t('factoryRuns.checksRound', { round: props.round.round })}
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
      <span class="block text-ink-muted">{props.round.checks.join(', ')}</span>
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
