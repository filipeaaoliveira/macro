import { t } from '@app/lib/i18n';
import { Button, TextField } from '@ui';
import { createSignal, createUniqueId, Match, Show, Switch } from 'solid-js';
import type { SteerAction } from '../core/run';

/**
 * A running Step's Step Terminal controls: a note to its agent, Pause or
 * Resume, and Stop step, confirmed in place as Cancel run is. While the
 * Step is paused, the note goes with the resume, and it says when the Step
 * ends unless resumed. Between its agent's turns the controls stay, disabled,
 * saying why. Nothing is sent once the Step ended or is stopping, or while
 * a steer is on the wire.
 */
export function StepTerminalControls(props: {
  state: 'working' | 'paused' | 'stopping' | 'between' | 'ended';
  /** When a paused Step ends unless resumed, as the person reads times. */
  endsBy?: string;
  /** The Step ended because it stayed paused past its time. */
  ranOut?: boolean;
  note: string;
  onNoteChange: (note: string) => void;
  /** The steer on the wire, if one is. */
  steering?: SteerAction;
  onSteer: (action: SteerAction) => void;
}) {
  const [confirmingStop, setConfirmingStop] = createSignal(false);
  const stopMessageId = createUniqueId();
  let keepButton: HTMLButtonElement | undefined;
  let stopButton: HTMLButtonElement | undefined;
  const locked = () =>
    props.steering !== undefined ||
    props.state === 'stopping' ||
    props.state === 'between' ||
    props.state === 'ended';
  const open = () =>
    props.state === 'working' ||
    props.state === 'paused' ||
    props.state === 'between';

  const askToStop = () => {
    setConfirmingStop(true);
    queueMicrotask(() => keepButton?.focus());
  };
  const keepRunning = () => {
    setConfirmingStop(false);
    queueMicrotask(() => stopButton?.focus());
  };
  const stop = () => {
    setConfirmingStop(false);
    props.onSteer('stop');
  };

  return (
    <div class="flex flex-col gap-3">
      <Switch>
        <Match when={props.state === 'ended' && props.ranOut}>
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.terminalPauseRanOut')}
          </p>
        </Match>
        <Match when={props.state === 'ended'}>
          <p class="text-sm text-ink-muted">{t('factoryRuns.terminalEnded')}</p>
        </Match>
        <Match when={props.state === 'between'}>
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.terminalBetweenTurns')}
          </p>
        </Match>
        <Match when={props.state === 'stopping'}>
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.terminalStoppingHint')}
          </p>
        </Match>
        <Match when={props.state === 'paused'}>
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.terminalPausedHint')}{' '}
            <Show when={props.endsBy}>
              {(endsBy) => (
                <span class="font-medium text-ink">
                  {t('factoryRuns.terminalPausedEndsBy', { time: endsBy() })}
                </span>
              )}
            </Show>
          </p>
        </Match>
      </Switch>
      <Show when={open()}>
        <TextField
          value={props.note}
          onChange={props.onNoteChange}
          disabled={locked()}
        >
          <TextField.Label class="sr-only">
            {t('factoryRuns.terminalNoteLabel')}
          </TextField.Label>
          <TextField.TextArea
            autoResize
            rows={2}
            class="max-h-32 overflow-y-auto"
            placeholder={t('factoryRuns.terminalNotePlaceholder')}
          />
        </TextField>
      </Show>
      <Show when={open()}>
        <Show
          when={confirmingStop()}
          fallback={
            <div class="flex flex-wrap items-center justify-end gap-2">
              <Button
                ref={stopButton}
                type="button"
                variant="danger-ghost"
                class="mr-auto"
                disabled={locked()}
                loading={props.steering === 'stop'}
                onClick={askToStop}
              >
                {t('factoryRuns.stopStep')}
              </Button>
              <Show
                when={props.state === 'paused'}
                fallback={
                  <>
                    <Button
                      type="button"
                      variant="ghost"
                      disabled={locked()}
                      loading={props.steering === 'pause'}
                      onClick={() => props.onSteer('pause')}
                    >
                      {t('factoryRuns.pauseStep')}
                    </Button>
                    <Button
                      type="button"
                      variant="primary"
                      disabled={locked() || props.note.trim() === ''}
                      loading={props.steering === 'note'}
                      onClick={() => props.onSteer('note')}
                    >
                      {t('factoryRuns.sendNote')}
                    </Button>
                  </>
                }
              >
                <Button
                  type="button"
                  variant="primary"
                  disabled={locked()}
                  loading={props.steering === 'resume'}
                  onClick={() => props.onSteer('resume')}
                >
                  {t('factoryRuns.resumeStep')}
                </Button>
              </Show>
            </div>
          }
        >
          <div class="flex w-full flex-wrap items-center justify-end gap-2">
            <p
              id={stopMessageId}
              class="mr-auto min-w-0 flex-1 basis-60 text-sm"
            >
              <span class="font-medium">{t('factoryRuns.stopStepTitle')}</span>{' '}
              <span class="text-ink-muted">
                {t('factoryRuns.stopStepBody')}
              </span>
            </p>
            <Button
              ref={keepButton}
              type="button"
              variant="ghost"
              aria-describedby={stopMessageId}
              onClick={keepRunning}
            >
              {t('factoryRuns.keepStepRunning')}
            </Button>
            <Button type="button" variant="danger" onClick={stop}>
              {t('factoryRuns.stopStep')}
            </Button>
          </div>
        </Show>
      </Show>
    </div>
  );
}
