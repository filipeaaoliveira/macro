import {
  Card,
  CardBody,
  CardFooter,
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
 * question in a well, an optional note, and one button per answer. It sits
 * in the Run view's pinned area, so the question is in view whenever its
 * answers are. The Flow lists its main answer first, so that one is the
 * primary action, last on the right.
 */
export function GateCard(props: {
  question: string;
  answers: readonly string[];
  note: string;
  onNoteChange: (note: string) => void;
  /** Nothing here can be used: an answer is sent, or the Gate was answered. */
  locked: boolean;
  /** The answer on the wire, shown loading. */
  answering?: string;
  onAnswer: (answer: string) => void;
}) {
  const main = () => props.answers[0];
  const others = () => props.answers.slice(1).reverse();

  return (
    <div role="group" aria-label={t('factoryRuns.gateTitle')}>
      <Card width="wide">
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
            disabled={props.locked}
          >
            <TextField.Label class="sr-only">
              {t('factoryRuns.noteLabel')}
            </TextField.Label>
            <TextField.TextArea
              autoResize
              rows={1}
              placeholder={t('factoryRuns.notePlaceholder')}
            />
          </TextField>
        </CardBody>
        <CardFooter class="justify-end pt-0">
          <For each={others()}>
            {(answer) => (
              <Button
                type="button"
                variant="secondary"
                disabled={props.locked}
                loading={props.answering === answer}
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
                disabled={props.locked}
                loading={props.answering === answer()}
                onClick={() => props.onAnswer(answer())}
              >
                {flowWordLabel(answer())}
              </Button>
            )}
          </Show>
        </CardFooter>
      </Card>
    </div>
  );
}
