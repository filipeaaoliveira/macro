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
import type { Tampering } from '../core/run';

/** The Gate's header and its question, in a block nested in the card. */
export function GateQuestion(props: {
  question: string;
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
              {t('factoryRuns.gateTitle')}
            </UiCard.Title>
            <UiCard.Description class="leading-5">
              {t('factoryRuns.gateWaiting')}
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
 * so a long note never pushes the answers out of reach.
 */
export function GateNote(props: {
  note: string;
  onNoteChange: (note: string) => void;
  locked: boolean;
}) {
  return (
    <TextField
      value={props.note}
      onChange={props.onNoteChange}
      disabled={props.locked}
    >
      <TextField.Label class="sr-only">
        {t('factoryRuns.noteLabel')}
      </TextField.Label>
      <TextField.TextArea
        autoResize
        rows={1}
        class="max-h-24 overflow-y-auto"
        placeholder={t('factoryRuns.notePlaceholder')}
      />
    </TextField>
  );
}

/**
 * The Control Files a Tampered Run's branch changed, which its request
 * doesn't name, and the person's approval of them, which approving the Gate
 * needs. The file list scrolls past a few lines.
 */
export function GateControlFiles(props: {
  tampering: Tampering;
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
      <Show
        when={!props.tampering.unlisted}
        fallback={
          <p class="text-ink-muted">{t('factoryRuns.controlFilesUnlisted')}</p>
        }
      >
        <p class="text-ink-muted">{t('factoryRuns.controlFilesChanged')}</p>
        <ul
          aria-label={t('factoryRuns.controlFilesTitle')}
          class="max-h-24 overflow-y-auto font-mono text-xs [overflow-wrap:anywhere]"
        >
          <For each={props.tampering.files}>{(file) => <li>{file}</li>}</For>
        </ul>
      </Show>
      <Checkbox
        size="sm"
        checked={props.approved}
        onChange={props.onApprovedChange}
        disabled={props.locked}
      >
        <Checkbox.Control />
        <Checkbox.Label>{t('factoryRuns.approveControlFiles')}</Checkbox.Label>
      </Checkbox>
    </UiCard>
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
  answers: readonly string[];
  /** The Control Files to approve, when the Run is Tampered. */
  tampering?: Tampering;
  approveControlFiles: boolean;
  onApproveControlFilesChange: (approved: boolean) => void;
  held?: readonly string[];
  note: string;
  onNoteChange: (note: string) => void;
  locked: boolean;
  answering?: string;
  onAnswer: (answer: string) => Promise<boolean>;
}) {
  return (
    <GateGroup>
      <Card width="wide">
        <GateQuestion question={props.question} capped />
        <Show when={props.tampering}>
          {(tampering) => (
            <div class="px-3 pb-2">
              <GateControlFiles
                tampering={tampering()}
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
          />
        </div>
        <CardFooter class="justify-end">
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
