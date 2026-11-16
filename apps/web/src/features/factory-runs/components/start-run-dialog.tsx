import { t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import { ActionDialogShell, Button, Dialog, RadioGroup, TextField } from '@ui';
import { createSignal, For, Match, Show, Switch } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import type { Flow } from '../core/run';

/**
 * Starts a Run on a task: what it should do, and which Flow takes it there.
 * Stays open with the error when starting fails.
 */
export function StartRunDialog(props: {
  flows: Flow[] | undefined;
  flowsState: 'pending' | 'error' | 'ready';
  onRetryFlows: () => void;
  onOpenChange: (open: boolean) => void;
  /** Resolves true once the Run started. */
  onStart: (flow: string, request: string) => Promise<boolean>;
}) {
  const [request, setRequest] = createSignal('');
  const [chosenFlow, setChosenFlow] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  const flow = () => chosenFlow() ?? props.flows?.[0]?.name;
  const canSubmit = () =>
    !pending() && request().trim().length > 0 && flow() !== undefined;

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const name = flow();
    if (!canSubmit() || !name) return;

    setPending(true);
    setFailed(false);
    const started = await props.onStart(name, request().trim());
    setPending(false);
    if (started) props.onOpenChange(false);
    else setFailed(true);
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!pending()) props.onOpenChange(open);
      }}
      position="center"
      class="w-120"
      visibleScrim
    >
      <ActionDialogShell>
        <form
          class="flex min-h-0 flex-col"
          aria-busy={pending()}
          onSubmit={(event) => fireAndForget(submit(event), 'factory-runs')}
        >
          <ActionDialogShell.Body>
            <ActionDialogShell.Header>
              <ActionDialogShell.Title>
                {t('factoryRuns.startTitle')}
              </ActionDialogShell.Title>
              <ActionDialogShell.Description>
                {t('factoryRuns.startDescription')}
              </ActionDialogShell.Description>
            </ActionDialogShell.Header>
            <TextField value={request()} onChange={setRequest}>
              <TextField.Label>{t('factoryRuns.requestLabel')}</TextField.Label>
              <TextField.TextArea
                autoResize
                rows={4}
                readOnly={pending()}
                placeholder={t('factoryRuns.requestPlaceholder')}
              />
            </TextField>
            <div class="flex flex-col gap-2">
              <span class="text-[13px] leading-[18px] font-medium">
                {t('factoryRuns.flowLabel')}
              </span>
              <Switch>
                <Match when={props.flowsState === 'pending'}>
                  <p role="status" class="text-sm text-ink-muted">
                    {t('factoryRuns.flowsLoading')}
                  </p>
                </Match>
                <Match when={props.flowsState === 'error'}>
                  <div class="flex items-center gap-2">
                    <p class="text-sm text-ink-muted">
                      {t('factoryRuns.flowsFailed')}
                    </p>
                    <Button
                      type="button"
                      variant="ghost"
                      size="xs"
                      onClick={() => props.onRetryFlows()}
                    >
                      {t('common.retry')}
                    </Button>
                  </div>
                </Match>
                <Match when={props.flows}>
                  {(flows) => (
                    <RadioGroup
                      variant="cards"
                      value={flow()}
                      onChange={setChosenFlow}
                      disabled={pending()}
                      aria-label={t('factoryRuns.flowLabel')}
                      class="grid-cols-1"
                    >
                      <For each={flows()}>
                        {(option) => (
                          <RadioGroup.Item value={option.name}>
                            <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                              <RadioGroup.ItemLabel class="text-sm font-medium text-ink">
                                {flowWordLabel(option.name)}
                              </RadioGroup.ItemLabel>
                              <RadioGroup.ItemDescription class="text-xs text-ink-muted">
                                {option.description}
                              </RadioGroup.ItemDescription>
                            </div>
                            <RadioGroup.ItemControl class="mt-0.5" />
                          </RadioGroup.Item>
                        )}
                      </For>
                    </RadioGroup>
                  )}
                </Match>
              </Switch>
            </div>
            <Show when={failed()}>
              <p role="alert" class="text-sm text-failure">
                {t('factoryRuns.startFailed')}
              </p>
            </Show>
          </ActionDialogShell.Body>
          <ActionDialogShell.Footer>
            <Button
              type="button"
              variant="ghost"
              disabled={pending()}
              onClick={() => props.onOpenChange(false)}
            >
              {t('common.cancel')}
            </Button>
            <Button type="submit" variant="primary" disabled={!canSubmit()}>
              {pending()
                ? t('factoryRuns.startingRun')
                : t('factoryRuns.startRun')}
            </Button>
          </ActionDialogShell.Footer>
        </form>
      </ActionDialogShell>
    </Dialog>
  );
}
