import {
  Card,
  CardBody,
  CardHeader,
} from '@app/features/agent-cards/components/kit/card';
import { Tile, Well } from '@app/features/agent-cards/components/kit/parts';
import { t } from '@app/lib/i18n';
import QuestionIcon from '@icon/system/regular/question.svg';
import { Button, TextField } from '@ui';
import { For, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';

/**
 * The question a Run waits on, as an agent card asking for input: the
 * question in a well and an optional note. Its answers sit in the dialog's
 * footer (`GateAnswers`), so they stay in reach while the plan scrolls.
 */
export function GateCard(props: {
  question: string;
  note: string;
  onNoteChange: (note: string) => void;
  disabled: boolean;
}) {
  return (
    <Card width="wide" class="max-w-none">
      <section aria-label={t('factoryRuns.gateTitle')} class="contents">
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
          <Well class="text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
            {props.question}
          </Well>
          <TextField
            value={props.note}
            onChange={props.onNoteChange}
            disabled={props.disabled}
          >
            <TextField.Label class="sr-only">
              {t('factoryRuns.noteLabel')}
            </TextField.Label>
            <TextField.TextArea
              autoResize
              rows={2}
              placeholder={t('factoryRuns.notePlaceholder')}
            />
          </TextField>
        </CardBody>
      </section>
    </Card>
  );
}

/**
 * One button per answer the Gate offers. The Flow lists its main answer
 * first, so that one is the primary action, last on the right.
 */
export function GateAnswers(props: {
  answers: readonly string[];
  disabled: boolean;
  onAnswer: (answer: string) => void;
}) {
  const main = () => props.answers[0];
  const others = () => props.answers.slice(1).reverse();

  return (
    <>
      <For each={others()}>
        {(answer) => (
          <Button
            type="button"
            variant="secondary"
            disabled={props.disabled}
            onClick={() => props.onAnswer(answer)}
          >
            {flowWordLabel(answer)}
          </Button>
        )}
      </For>
      <Show when={main()}>
        {(answer) => (
          <Button
            type="button"
            variant="primary"
            disabled={props.disabled}
            onClick={() => props.onAnswer(answer())}
          >
            {flowWordLabel(answer())}
          </Button>
        )}
      </Show>
    </>
  );
}
