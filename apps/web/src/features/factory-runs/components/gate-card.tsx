import { t } from '@app/lib/i18n';
import QuestionIcon from '@icon/system/regular/question.svg';
import { Button, TextField } from '@ui';
import { For, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';

/**
 * The question a Run waits on, laid out like an agent's approval card: the
 * question, an optional note, and one button per answer. The Flow lists its
 * main answer first, so that one is the primary action, at the right.
 */
export function GateCard(props: {
  question: string;
  answers: readonly string[];
  note: string;
  onNoteChange: (note: string) => void;
  /** An answer is on the wire: nothing here can be used again. */
  disabled: boolean;
  onAnswer: (answer: string) => void;
}) {
  const main = () => props.answers[0];
  const others = () => props.answers.slice(1).reverse();

  return (
    <section
      aria-label={t('factoryRuns.gateTitle')}
      class="min-w-0 overflow-hidden rounded-xl border border-edge-muted bg-panel text-ink"
    >
      <div class="flex items-start gap-3 px-4 pt-4">
        <div class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent-bg text-accent">
          <QuestionIcon class="size-4" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="text-sm font-medium">{t('factoryRuns.gateTitle')}</div>
          <p class="mt-0.5 text-xs leading-5 text-ink-muted">
            {t('factoryRuns.gateWaiting')}
          </p>
        </div>
      </div>
      <div class="mx-4 mt-3 rounded-lg bg-surface px-3 py-2.5 text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
        {props.question}
      </div>
      <div class="px-4 pt-3">
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
      </div>
      <div class="flex min-w-0 flex-wrap items-center justify-end gap-2 p-3">
        <For each={others()}>
          {(answer) => (
            <Button
              type="button"
              size="md"
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
              size="md"
              variant="primary"
              disabled={props.disabled}
              onClick={() => props.onAnswer(answer())}
            >
              {flowWordLabel(answer())}
            </Button>
          )}
        </Show>
      </div>
    </section>
  );
}
