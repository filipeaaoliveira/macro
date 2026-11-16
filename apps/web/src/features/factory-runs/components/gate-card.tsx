import {
  Card,
  CardBody,
  CardFooter,
  CardHeader,
} from '@app/features/agent-cards/components/kit/card';
import { Tile, Well } from '@app/features/agent-cards/components/kit/parts';
import { t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import QuestionIcon from '@icon/system/regular/question.svg';
import { Button, cn, TextField } from '@ui';
import { For, type JSX, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';

/** The Gate's header and its question, in a well. */
export function GateQuestion(props: {
  question: string;
  /** Cap the question's height and scroll it, where it is pinned. */
  capped?: boolean;
}) {
  return (
    <>
      <CardHeader
        tile={
          <Tile size={32} tone="accent">
            <QuestionIcon />
          </Tile>
        }
        title={t('factoryRuns.gateTitle')}
        subtitle={t('factoryRuns.gateWaiting')}
        wrap
      />
      <CardBody>
        <Well
          class={cn(
            'text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]',
            props.capped && 'max-h-32 overflow-y-auto'
          )}
        >
          {props.question}
        </Well>
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
 * One button per answer. The Flow lists its main answer first, so that one
 * is the primary action, last on the right. `onAnswer` resolves whether the
 * answer went through; when it did not, focus goes back to the button,
 * which its loading state had taken focus from.
 */
export function GateAnswers(props: {
  answers: readonly string[];
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
      disabled={props.locked}
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
export function GateGroup(props: { children: JSX.Element; class?: string }) {
  return (
    <div
      role="group"
      aria-label={t('factoryRuns.gateTitle')}
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
            locked={props.locked}
            answering={props.answering}
            onAnswer={props.onAnswer}
          />
        </CardFooter>
      </Card>
    </GateGroup>
  );
}
