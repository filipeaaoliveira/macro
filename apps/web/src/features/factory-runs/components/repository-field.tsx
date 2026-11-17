import { repositoryLabel } from '@app/features/agents-view/core/repository';
import { t } from '@app/lib/i18n';
import CaretDownIcon from '@icon/system/regular/caret-down.svg';
import CheckIcon from '@icon/system/regular/check.svg';
import { Combobox } from '@kobalte/core/combobox';
import GithubIcon from '@phosphor/github-logo.svg';
import { Button, Layer, selectTriggerClasses } from '@ui';
import { createMemo, createUniqueId, Match, Show, Switch } from 'solid-js';

type RepositoryOption = { url: string; label: string };

/**
 * The GitHub repository a Run works on, searched by `owner/name` among the
 * repositories the person can start one on.
 */
export function RepositoryField(props: {
  /** Their URLs; undefined until they are read. */
  repositories: string[] | undefined;
  state: 'pending' | 'error' | 'ready';
  onRetry: () => void;
  value: string | undefined;
  onChange: (url: string) => void;
  /** Where to send someone who reaches no repository yet. */
  onConnectGitHub?: () => void;
  disabled?: boolean;
}) {
  const labelId = createUniqueId();
  const descriptionId = createUniqueId();
  let input: HTMLInputElement | undefined;

  const options = createMemo<RepositoryOption[]>(() =>
    (props.repositories ?? []).map((url) => ({
      url,
      label: repositoryLabel(url),
    }))
  );
  const selected = () =>
    options().find((option) => option.url === props.value) ?? null;
  const loading = () => props.state === 'pending';
  const none = () => props.state === 'ready' && options().length === 0;

  return (
    <div class="flex flex-col gap-2">
      <span id={labelId} data-slot="text-field-label">
        {t('factoryRuns.repositoryLabel')}
      </span>
      <Combobox<RepositoryOption>
        options={options()}
        value={selected()}
        onChange={(option) => {
          if (option) props.onChange(option.url);
        }}
        optionValue="url"
        optionTextValue="label"
        optionLabel="label"
        defaultFilter="contains"
        disabled={props.disabled || loading() || options().length === 0}
        placeholder={
          loading()
            ? t('factoryRuns.repositoriesLoading')
            : t('factoryRuns.repositoryPlaceholder')
        }
        itemComponent={(item) => (
          <Combobox.Item
            item={item.item}
            // Keep virtual focus in the search input.
            on:mousedown={(event: MouseEvent) => event.preventDefault()}
            class="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm outline-none data-highlighted:bg-ink/5 touch:min-h-11"
          >
            <GithubIcon class="size-4 shrink-0 text-ink-muted" />
            <Combobox.ItemLabel class="min-w-0 flex-1 truncate text-ink">
              {item.item.rawValue.label}
            </Combobox.ItemLabel>
            <Combobox.ItemIndicator class="shrink-0 text-accent">
              <CheckIcon class="size-3" />
            </Combobox.ItemIndicator>
          </Combobox.Item>
        )}
      >
        <Combobox.Control
          class={selectTriggerClasses('min-w-0 touch:min-h-11')}
        >
          <GithubIcon class="size-4 shrink-0 text-ink-muted" />
          <Combobox.Input
            ref={input}
            aria-labelledby={labelId}
            aria-describedby={descriptionId}
            class="min-w-0 w-full bg-transparent text-sm text-ink outline-none"
          />
          <Combobox.Trigger
            aria-label={t('factoryRuns.showRepositories')}
            class="shrink-0 text-ink-extra-muted"
          >
            <CaretDownIcon class="size-3" />
          </Combobox.Trigger>
        </Combobox.Control>
        <Combobox.Portal
          mount={input?.closest<HTMLElement>('.portal-scope') ?? undefined}
        >
          <Layer depth={3}>
            <Combobox.Content class="menu-surface z-action-menu max-h-[min(20rem,var(--kb-popper-content-available-height))] w-[var(--kb-popper-anchor-width)] max-w-[calc(100vw-2rem)] overflow-y-auto p-1.5">
              <Combobox.Listbox />
            </Combobox.Content>
          </Layer>
        </Combobox.Portal>
      </Combobox>
      <Switch
        fallback={
          <p id={descriptionId} class="text-xs text-ink-muted">
            {t('factoryRuns.repositoryDescription')}
          </p>
        }
      >
        <Match when={props.state === 'error'}>
          <div id={descriptionId} class="flex items-center gap-2">
            <p class="text-xs text-ink-muted">
              {t('factoryRuns.repositoriesFailed')}
            </p>
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={() => props.onRetry()}
            >
              {t('common.retry')}
            </Button>
          </div>
        </Match>
        <Match when={none()}>
          <div id={descriptionId} class="flex flex-col items-start gap-2">
            <p class="text-xs text-ink-muted">
              {t('factoryRuns.noRepositories')}
            </p>
            <Show when={props.onConnectGitHub}>
              {(connect) => (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => connect()()}
                >
                  {t('factoryRuns.connectGitHub')}
                </Button>
              )}
            </Show>
          </div>
        </Match>
      </Switch>
    </div>
  );
}
