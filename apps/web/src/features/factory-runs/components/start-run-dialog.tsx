import { ListSkeleton } from '@app/components/view-shell/ListSkeleton';
import { t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import { ActionDialogShell, Button, Dialog, RadioGroup, TextField } from '@ui';
import {
  createSignal,
  For,
  Index,
  type JSX,
  Match,
  Show,
  Switch,
} from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import type { Flow } from '../core/run';
import { RepositoryField } from './repository-field';

/** What a Run is started with. */
export type StartRunChoice = {
  flow: string;
  request: string;
  /** The repository's URL, for a Flow that works on one. */
  repoUrl?: string;
};

/**
 * Starts a Run on a task: what it should do, which Flow takes it there, and
 * the repository it works on when the Flow works on one. Stays open with the
 * error when starting fails.
 */
export function StartRunDialog(props: {
  flows: Flow[] | undefined;
  flowsState: 'pending' | 'error' | 'ready';
  onRetryFlows: () => void;
  /** The URLs of the repositories a Run can work on. */
  repositories: string[] | undefined;
  repositoriesState: 'pending' | 'error' | 'ready';
  onRetryRepositories: () => void;
  onOpenChange: (open: boolean) => void;
  /** Resolves true once the Run started. */
  onStart: (choice: StartRunChoice) => Promise<boolean>;
}) {
  const [request, setRequest] = createSignal('');
  const [chosenFlow, setChosenFlow] = createSignal<string>();
  const [repoUrl, setRepoUrl] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  const flow = () =>
    props.flows?.find((option) => option.name === chosenFlow()) ??
    props.flows?.[0];
  const needsRepository = () => flow()?.repository ?? false;
  const canSubmit = () =>
    !pending() &&
    request().trim().length > 0 &&
    flow() !== undefined &&
    (!needsRepository() || repoUrl() !== undefined);

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const name = flow()?.name;
    if (!canSubmit() || !name) return;

    setPending(true);
    setFailed(false);
    const started = await props.onStart({
      flow: name,
      request: request().trim(),
      repoUrl: needsRepository() ? repoUrl() : undefined,
    });
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
            <Switch>
              <Match when={props.flowsState === 'pending'}>
                <FlowsNotice>
                  <FlowCardsSkeleton />
                </FlowsNotice>
              </Match>
              <Match when={props.flowsState === 'error'}>
                <FlowsNotice>
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
                </FlowsNotice>
              </Match>
              <Match when={props.flows}>
                {(flows) => (
                  <RadioGroup
                    variant="cards"
                    value={flow()?.name}
                    onChange={setChosenFlow}
                    disabled={pending()}
                    class="grid-cols-1"
                  >
                    {/* The dialog body styles the text-field label slot. */}
                    <RadioGroup.Label data-slot="text-field-label">
                      {t('factoryRuns.flowLabel')}
                    </RadioGroup.Label>
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
            <Show when={needsRepository()}>
              <RepositoryField
                repositories={props.repositories}
                state={props.repositoriesState}
                onRetry={props.onRetryRepositories}
                value={repoUrl()}
                onChange={setRepoUrl}
                disabled={pending()}
              />
            </Show>
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
            <Button
              type="submit"
              variant="primary"
              loading={pending()}
              disabled={!pending() && !canSubmit()}
            >
              {t('factoryRuns.startRun')}
            </Button>
          </ActionDialogShell.Footer>
        </form>
      </ActionDialogShell>
    </Dialog>
  );
}

/** The Flow field while its choices load or can't be loaded. */
function FlowsNotice(props: { children: JSX.Element }) {
  return (
    <div class="flex flex-col gap-2">
      <span data-slot="text-field-label">{t('factoryRuns.flowLabel')}</span>
      {props.children}
    </div>
  );
}

/** The Flow cards' placeholder, in their geometry. */
function FlowCardsSkeleton() {
  return (
    <ListSkeleton.Root label={t('factoryRuns.flowsLoading')}>
      <div class="grid gap-2">
        <Index each={[0, 1]}>
          {() => (
            <div class="flex flex-col gap-2 rounded-lg border border-edge-button p-3">
              <ListSkeleton.Bar class="w-20" />
              <ListSkeleton.Bar class="h-2 w-64 max-w-full" />
            </div>
          )}
        </Index>
      </div>
    </ListSkeleton.Root>
  );
}
