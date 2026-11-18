import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Button, TextField } from '@ui';
import { createSignal, createUniqueId, Show } from 'solid-js';
import { useRunsContext } from '../context/runs-context';
import type { RunFlaw } from '../core/run';

/** The most bytes a flaw's reason holds, as the factory keeps it. */
const MAX_REASON_BYTES = 4096;

function reasonBytes(reason: string): number {
  return new TextEncoder().encode(reason.trim()).length;
}

/** Why a flawed Run is flawed, and who marked it when. */
export function RunFlawNote(props: {
  flaw: RunFlaw;
  time: (value: string) => string;
}) {
  const marked = () => {
    const time = props.time(props.flaw.markedAt);
    const by = props.flaw.by;
    return by.kind === 'scorer'
      ? t('factoryRuns.flawedByScorer', { scorer: by.name, time })
      : t('factoryRuns.flawedByPerson', { time });
  };

  return (
    <section class="flex flex-col gap-1.5">
      <h3 class="text-xs font-medium text-ink-muted">
        {t('factoryRuns.flawed')}
      </h3>
      <p class="text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
        {props.flaw.reason}
      </p>
      <p class="text-xs text-ink-muted">{marked()}</p>
    </section>
  );
}

/**
 * Mark flawed, for a Run that ended in success, with what is wrong with its
 * result: confirmed in place, as Cancel run is, so no second dialog stacks
 * on a phone's sheet.
 */
export function MarkRunFlawed(props: { runId: string }) {
  const context = useRunsContext();
  const [confirming, setConfirming] = createSignal(false);
  const [reason, setReason] = createSignal('');
  const [marking, setMarking] = createSignal(false);
  const messageId = createUniqueId();
  let markButton: HTMLButtonElement | undefined;

  const keepDone = () => {
    setConfirming(false);
    queueMicrotask(() => markButton?.focus());
  };

  const tooLong = () => reasonBytes(reason()) > MAX_REASON_BYTES;
  const mark = async () => {
    const words = reason().trim();
    if (!words || tooLong() || marking()) return;
    setMarking(true);
    const result = await context.markRunFlawed({
      runId: props.runId,
      reason: words,
    });
    setMarking(false);
    if (result.ok) return;
    if (result.reason === 'conflict') {
      toast.alert(t('factoryRuns.markFlawedConflict'));
      return;
    }
    toast.failure(t('factoryRuns.markFlawedFailed'));
  };

  return (
    <Show
      when={confirming()}
      fallback={
        <Button
          ref={markButton}
          type="button"
          variant="ghost"
          onClick={() => setConfirming(true)}
        >
          {t('factoryRuns.markFlawed')}
        </Button>
      }
    >
      <div class="flex w-full flex-col gap-2">
        <p id={messageId} class="text-sm">
          <span class="font-medium">{t('factoryRuns.markFlawedTitle')}</span>{' '}
          <span class="text-ink-muted">{t('factoryRuns.markFlawedBody')}</span>
        </p>
        <TextField
          value={reason()}
          onChange={setReason}
          disabled={marking()}
          required
          validationState={tooLong() ? 'invalid' : 'valid'}
        >
          <TextField.Label class="sr-only">
            {t('factoryRuns.flawReason')}
          </TextField.Label>
          <TextField.TextArea
            autofocus
            autoResize
            rows={2}
            class="max-h-24 overflow-y-auto"
            aria-describedby={messageId}
            placeholder={t('factoryRuns.flawReason')}
          />
          <TextField.ErrorMessage>
            {t('factoryRuns.flawReasonTooLong', { max: MAX_REASON_BYTES })}
          </TextField.ErrorMessage>
        </TextField>
        <div class="flex flex-wrap items-center justify-end gap-2">
          <Button
            type="button"
            variant="ghost"
            disabled={marking()}
            onClick={keepDone}
          >
            {t('factoryRuns.keepDone')}
          </Button>
          <Button
            type="button"
            variant="danger"
            loading={marking()}
            disabled={!reason().trim() || tooLong()}
            onClick={() => fireAndForget(mark(), 'factory-runs')}
          >
            {t('factoryRuns.markFlawed')}
          </Button>
        </div>
      </div>
    </Show>
  );
}
