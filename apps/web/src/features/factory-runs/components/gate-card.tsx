import {
  Card,
  CardBody,
  CardFooter,
} from '@app/features/agent-cards/components/kit/card';
import { t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import QuestionIcon from '@icon/system/regular/question.svg';
import { Button, Checkbox, cn, Item, TextField, Card as UiCard } from '@ui';
import { For, type JSX, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import {
  type ControlFileChanges,
  type Gate,
  type GateModelReading,
  isTampered,
} from '../core/run';

/**
 * The Gate's header and its question, in a block nested in the card. An
 * agent's question says which Role asks.
 */
export function GateQuestion(props: {
  question: string;
  /** The Role of the agent that asks, for an agent's question. */
  asker?: string;
  /**
   * Cap the question's height where it is pinned; it scrolls, and takes
   * keyboard focus so it can be scrolled without a pointer.
   */
  capped?: boolean;
  /** For a group of answers elsewhere to name the question it answers. */
  id?: string;
}) {
  return (
    <>
      <UiCard.Header class="pb-2">
        <Item class="items-start gap-3 border-0 p-0">
          <Item.Media>
            <QuestionIcon />
          </Item.Media>
          <Item.Content class="gap-0.5">
            <UiCard.Title class="[overflow-wrap:anywhere]">
              <Show when={props.asker} fallback={t('factoryRuns.gateTitle')}>
                {(asker) =>
                  t('factoryRuns.questionTitle', {
                    role: flowWordLabel(asker()),
                  })
                }
              </Show>
            </UiCard.Title>
            <UiCard.Description class="leading-5">
              {props.asker
                ? t('factoryRuns.questionWaiting')
                : t('factoryRuns.gateWaiting')}
            </UiCard.Description>
          </Item.Content>
        </Item>
      </UiCard.Header>
      <CardBody>
        <UiCard
          variant="filled"
          offset={1}
          class="px-3 py-2.5 text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]"
        >
          <div
            id={props.id}
            role={props.capped ? 'region' : undefined}
            aria-label={props.capped ? t('factoryRuns.question') : undefined}
            tabIndex={props.capped ? 0 : undefined}
            class={cn(
              props.capped &&
                'max-h-32 overflow-y-auto rounded-sm outline-none focus-visible:outline-2 focus-visible:outline-accent'
            )}
          >
            {props.question}
          </div>
        </UiCard>
      </CardBody>
    </>
  );
}

/**
 * A note for the next Steps, at most a few lines tall before it scrolls,
 * so a long note never pushes the answers out of reach. For an agent's
 * question it is the answer itself, which the person must write.
 */
export function GateNote(props: {
  note: string;
  onNoteChange: (note: string) => void;
  locked: boolean;
  /** The note answers an agent's question. */
  answers?: boolean;
}) {
  return (
    <TextField
      value={props.note}
      onChange={props.onNoteChange}
      disabled={props.locked}
      required={props.answers}
    >
      <TextField.Label class="sr-only">
        {props.answers
          ? t('factoryRuns.answerLabel')
          : t('factoryRuns.noteLabel')}
      </TextField.Label>
      <TextField.TextArea
        autoResize
        rows={props.answers ? 2 : 1}
        class="max-h-24 overflow-y-auto"
        placeholder={
          props.answers
            ? t('factoryRuns.answerPlaceholder')
            : t('factoryRuns.notePlaceholder')
        }
      />
    </TextField>
  );
}

/**
 * The Control Files a Run's branch changed: those its request doesn't ask
 * for, with the person's approval of them, which approving the Gate needs,
 * and those the request names, which need none. Each list scrolls past a
 * few lines.
 */
export function GateControlFiles(props: {
  changes: ControlFileChanges;
  approved: boolean;
  onApprovedChange: (approved: boolean) => void;
  locked: boolean;
}) {
  return (
    <UiCard
      variant="filled"
      offset={1}
      class="flex flex-col gap-2 px-3 py-2.5 text-sm leading-5"
    >
      <p class="font-medium">{t('factoryRuns.controlFilesTitle')}</p>
      <Show when={props.changes.unlisted}>
        <p class="text-ink-muted">{t('factoryRuns.controlFilesUnlisted')}</p>
      </Show>
      <Show when={props.changes.tampered.length > 0}>
        <p class="text-ink-muted">{t('factoryRuns.controlFilesChanged')}</p>
        <ControlFileList
          label={t('factoryRuns.controlFilesTitle')}
          files={props.changes.tampered}
        />
      </Show>
      <Show when={props.changes.named.length > 0}>
        <p class="text-ink-muted">{t('factoryRuns.controlFilesNamed')}</p>
        <ControlFileList
          label={t('factoryRuns.controlFilesNamedLabel')}
          files={props.changes.named}
        />
      </Show>
      <Show when={isTampered(props.changes)}>
        <Checkbox
          size="sm"
          checked={props.approved}
          onChange={props.onApprovedChange}
          disabled={props.locked}
        >
          <Checkbox.Control />
          <Checkbox.Label>
            {t('factoryRuns.approveControlFiles')}
          </Checkbox.Label>
        </Checkbox>
      </Show>
    </UiCard>
  );
}

function ControlFileList(props: { label: string; files: readonly string[] }) {
  return (
    <ul
      aria-label={props.label}
      class="max-h-24 overflow-y-auto font-mono text-xs [overflow-wrap:anywhere]"
    >
      <For each={props.files}>{(file) => <li>{file}</li>}</For>
    </ul>
  );
}

/**
 * One button per answer. The Flow lists its main answer first, so that one
 * is the primary action, last on the right. `onAnswer` resolves whether the
 * answer went through; when it did not, focus goes back to the button,
 * which its loading state had taken focus from. An answer in `held` can't
 * be sent yet.
 */
export function GateAnswers(props: {
  answers: readonly string[];
  held?: readonly string[];
  locked: boolean;
  /** The answer on the wire, shown loading. */
  answering?: string;
  onAnswer: (answer: string) => Promise<boolean>;
}) {
  const main = () => props.answers[0];
  const others = () => props.answers.slice(1).reverse();

  const answer = async (value: string, button: HTMLButtonElement) => {
    const sent = await props.onAnswer(value);
    if (sent) return;
    queueMicrotask(() => {
      // The Gate went (someone answered first): the status line takes focus.
      if (!button.isConnected) return;
      // Leave focus alone if the person has moved on, to Cancel run say.
      const active = document.activeElement;
      const lost =
        !active ||
        active === document.body ||
        active === button.closest('[role="dialog"]');
      if (!lost) return;
      button.focus();
      // The buttons are locked: keep focus on the Gate.
      if (document.activeElement !== button) {
        button.closest<HTMLElement>('[role="group"]')?.focus();
      }
    });
  };

  const AnswerButton = (answerProps: {
    answer: string;
    variant: 'primary' | 'secondary';
  }) => (
    <Button
      type="button"
      variant={answerProps.variant}
      disabled={props.locked || props.held?.includes(answerProps.answer)}
      loading={props.answering === answerProps.answer}
      onClick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
        fireAndForget(
          answer(answerProps.answer, event.currentTarget),
          'factory-runs'
        )
      }
    >
      {flowWordLabel(answerProps.answer)}
    </Button>
  );

  return (
    <>
      <For each={others()}>
        {(value) => <AnswerButton answer={value} variant="secondary" />}
      </For>
      <Show when={main()}>
        {(value) => <AnswerButton answer={value()} variant="primary" />}
      </Show>
    </>
  );
}

/**
 * Sends the note as the person's words in place of an answer: the factory
 * reads which answer they give, or asks back. Only a Flow's Gate takes
 * words this way; an agent's question takes the note as its answer.
 */
export function GateReply(props: {
  /** Whether there are words to send. */
  ready: boolean;
  locked: boolean;
  replying: boolean;
  onReply: () => Promise<boolean>;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      class="mr-auto"
      disabled={props.locked || !props.ready}
      loading={props.replying}
      onClick={() => fireAndForget(props.onReply(), 'factory-runs')}
    >
      {t('factoryRuns.sendReply')}
    </Button>
  );
}

/** How the Gate model read the Gate, in one muted line, when it decided nothing. */
function ModelReadingLine(props: { reading: GateModelReading }) {
  const said = () => {
    const { pick, confidence } = props.reading;
    if (confidence === undefined) return t('factoryRuns.modelDidNotAnswer');
    if (pick === undefined) {
      return t('factoryRuns.modelCouldNotTell', { confidence });
    }
    return t('factoryRuns.modelLeaned', {
      answer: flowWordLabel(pick),
      confidence,
    });
  };
  return (
    <p class="whitespace-pre-wrap text-ink-muted">
      {said()}
      <Show when={props.reading.reason}>{(reason) => <> {reason()}</>}</Show>
    </p>
  );
}

/**
 * What the factory did with the Gate so far, while it waits: the stand-in's
 * answer for the owner, which they may override until its window ends; or
 * why the stand-in left the Gate to them, and words it couldn't read as an
 * answer (which end the stand-in's window), each with how the Gate model
 * read them.
 */
export function GateStatus(props: {
  gate: Pick<Gate, 'askedBack' | 'standIn' | 'standInDeclined'>;
  /** A time, as the view shows it. */
  time: (value: string) => string;
}) {
  return (
    <Show
      when={
        props.gate.standIn ?? props.gate.askedBack ?? props.gate.standInDeclined
      }
    >
      <UiCard
        variant="filled"
        offset={1}
        role="status"
        class="flex flex-col gap-1 px-3 py-2.5 text-sm leading-5 [overflow-wrap:anywhere]"
      >
        {/*
          Words the factory asked back on in the stand-in's window ended it:
          the Gate waits for the owner, so the window is no longer shown.
        */}
        <Show
          when={props.gate.askedBack ? undefined : props.gate.standIn}
          fallback={
            <>
              <Show when={props.gate.standInDeclined}>
                {(declined) => (
                  <>
                    <p class="font-medium">
                      {t('factoryRuns.standInDeclined')}
                    </p>
                    <ModelReadingLine reading={declined()} />
                  </>
                )}
              </Show>
              <Show when={props.gate.askedBack}>
                {(asked) => (
                  <>
                    <p>
                      {t('factoryRuns.askedBack', { reply: asked().reply })}
                    </p>
                    <Show when={asked().reading}>
                      {(reading) => <ModelReadingLine reading={reading()} />}
                    </Show>
                  </>
                )}
              </Show>
            </>
          }
        >
          {(standIn) => (
            <>
              <p class="font-medium">
                {t('factoryRuns.standInAnswered', {
                  answer: flowWordLabel(standIn().answer),
                  confidence: standIn().confidence,
                })}
              </p>
              <p class="whitespace-pre-wrap text-ink-muted">
                {standIn().reason}
              </p>
              <p class="text-ink-muted">
                {t('factoryRuns.standInWindow', {
                  time: props.time(standIn().until),
                })}
              </p>
            </>
          )}
        </Show>
      </UiCard>
    </Show>
  );
}

/** A named group around a Gate's parts, wherever a view places them. */
export function GateGroup(props: {
  children: JSX.Element;
  class?: string;
  /** The question's id, where the question sits outside the group. */
  describedBy?: string;
}) {
  return (
    <div
      role="group"
      aria-label={t('factoryRuns.gateTitle')}
      aria-describedby={props.describedBy}
      tabIndex={-1}
      class={cn('outline-none', props.class)}
    >
      {props.children}
    </div>
  );
}

/**
 * The question a Run waits on, as an agent card asking for input: the
 * question in a capped well, the note and the answers. The desktop dialog
 * pins it under the scrolling Steps.
 */
export function GateCard(props: {
  question: string;
  /** The Role of the agent that asks, for an agent's question. */
  asker?: string;
  answers: readonly string[];
  /** What the factory did with the Gate so far: asked back, or stood in. */
  gate: Pick<Gate, 'askedBack' | 'standIn' | 'standInDeclined'>;
  time: (value: string) => string;
  /** The Control Files the Run's branch changed, if any. */
  controlFiles?: ControlFileChanges;
  approveControlFiles: boolean;
  onApproveControlFilesChange: (approved: boolean) => void;
  held?: readonly string[];
  note: string;
  onNoteChange: (note: string) => void;
  locked: boolean;
  answering?: string;
  onAnswer: (answer: string) => Promise<boolean>;
  replying: boolean;
  onReply: () => Promise<boolean>;
}) {
  return (
    <GateGroup>
      <Card width="wide">
        <GateQuestion question={props.question} asker={props.asker} capped />
        <div class="px-3 pb-2 empty:hidden">
          <GateStatus gate={props.gate} time={props.time} />
        </div>
        <Show when={props.controlFiles}>
          {(changes) => (
            <div class="px-3 pb-2">
              <GateControlFiles
                changes={changes()}
                approved={props.approveControlFiles}
                onApprovedChange={props.onApproveControlFilesChange}
                locked={props.locked}
              />
            </div>
          )}
        </Show>
        <div class="px-3">
          <GateNote
            note={props.note}
            onNoteChange={props.onNoteChange}
            locked={props.locked}
            answers={props.asker !== undefined}
          />
        </div>
        <CardFooter class="justify-end">
          <Show when={props.asker === undefined}>
            <GateReply
              ready={props.note.trim() !== ''}
              locked={props.locked}
              replying={props.replying}
              onReply={props.onReply}
            />
          </Show>
          <GateAnswers
            answers={props.answers}
            held={props.held}
            locked={props.locked}
            answering={props.answering}
            onAnswer={props.onAnswer}
          />
        </CardFooter>
      </Card>
    </GateGroup>
  );
}
