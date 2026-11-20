import { t } from '@app/lib/i18n';
import type {
  DefinitionContents as Contents,
  DefinitionPullRequest,
  DefinitionRead,
} from '@service-factory/generated/schemas';
import { For, type JSX, Show } from 'solid-js';
import { RULE_NAMES } from '../core/standing-rules';

/** A link to `repoUrl`'s files at `commit`, labelled by `children`. */
function CommitLink(props: {
  repoUrl: string;
  commit: string;
  children: JSX.Element;
}) {
  return (
    <a
      class="underline"
      href={`${props.repoUrl}/tree/${props.commit}`}
      target="_blank"
      rel="noopener noreferrer"
    >
      {props.children}
    </a>
  );
}

/** One part of the Definition: a heading and what it holds. */
function Part(props: { title: string; children: JSX.Element }) {
  return (
    <section class="flex flex-col gap-1">
      <h4 class="text-xs font-medium text-ink-muted">{props.title}</h4>
      {props.children}
    </section>
  );
}

/** The Standing Rules a Definition sets, by the rules they turn on. */
function standingRulesText(rules: Contents['standingRules']): string {
  if (!rules) return t('factoryRuns.definitionRulesDefault');
  const on = [
    rules.plan && t(RULE_NAMES.approve_plan),
    rules.blockers && t(RULE_NAMES.fix_blockers),
    rules.merge && t(RULE_NAMES.merge_when_green),
  ].filter((name): name is string => Boolean(name));
  return on.length > 0 ? on.join(', ') : t('factoryRuns.definitionRulesNone');
}

/**
 * Which commit of the team's Factory Definition the newest Run read, and,
 * read-only, what the Definition holds there: its Flows, Roles, Standing
 * Rules and Scorers.
 */
export function DefinitionContents(props: {
  repoUrl: string;
  newestRun: DefinitionRead | null | undefined;
  contents: Contents | null | undefined;
}) {
  return (
    <div class="flex flex-col gap-2">
      <Show
        when={props.newestRun}
        fallback={
          <p class="text-xs text-ink-muted">
            {t('factoryRuns.definitionNoRunYet')}
          </p>
        }
      >
        {(read) => (
          <p class="text-xs text-ink-muted">
            <CommitLink repoUrl={props.repoUrl} commit={read().commit}>
              {t('factoryRuns.definitionNewestRun', {
                commit: read().commit.slice(0, 7),
              })}
            </CommitLink>
          </p>
        )}
      </Show>
      <Show when={props.contents}>
        {(contents) => (
          <div
            class="flex flex-col gap-2 rounded border border-edge-muted p-2"
            aria-label={t('factoryRuns.definitionContents')}
            role="group"
          >
            <p class="text-xs text-ink-muted">
              <CommitLink repoUrl={props.repoUrl} commit={contents().commit}>
                {t('factoryRuns.definitionShownAt', {
                  commit: contents().commit.slice(0, 7),
                })}
              </CommitLink>
            </p>
            <Show when={contents().description}>
              <p class="text-sm">{contents().description}</p>
            </Show>
            <Part title={t('factoryRuns.definitionFlows')}>
              <ul class="flex flex-col gap-0.5 text-sm">
                <For each={contents().flows}>
                  {(flow) => (
                    <li class="[overflow-wrap:anywhere]">
                      <span class="font-mono">{flow.name}</span>
                      <span class="text-ink-muted">
                        {' '}
                        {flow.entries.join(' → ')}
                      </span>
                    </li>
                  )}
                </For>
              </ul>
            </Part>
            <Part title={t('factoryRuns.definitionRoles')}>
              <ul class="flex flex-col gap-0.5 text-sm">
                <For each={contents().roles}>
                  {(role) => (
                    <li class="[overflow-wrap:anywhere]">
                      <span class="font-mono">{role.role}</span>
                      <span class="text-ink-muted">
                        {' '}
                        {role.agents.join(', ')}
                      </span>
                    </li>
                  )}
                </For>
              </ul>
            </Part>
            <Part title={t('factoryRuns.definitionStandingRules')}>
              <p class="text-sm">
                {standingRulesText(contents().standingRules)}
              </p>
            </Part>
            <Part title={t('factoryRuns.definitionScorers')}>
              <Show
                when={contents().scorers.length > 0}
                fallback={
                  <p class="text-sm text-ink-muted">
                    {t('factoryRuns.definitionNoScorers')}
                  </p>
                }
              >
                <ul class="flex flex-col gap-0.5 text-sm">
                  <For each={contents().scorers}>
                    {(scorer) => (
                      <li class="[overflow-wrap:anywhere]">
                        <span class="font-mono">{scorer.name}</span>
                        <span class="text-ink-muted">
                          {' '}
                          {t('factoryRuns.definitionScorerSample', {
                            percent: scorer.samplePercent,
                            flows:
                              scorer.flows.length > 0
                                ? scorer.flows.join(', ')
                                : t('factoryRuns.definitionScorerAllFlows'),
                          })}
                        </span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Part>
          </div>
        )}
      </Show>
    </div>
  );
}

/**
 * The open pull requests on the Definition's repository, each linked: the
 * changes to the factory waiting to merge. `null` when GitHub couldn't list
 * them.
 */
export function DefinitionPullRequests(props: {
  repoUrl: string;
  pullRequests: DefinitionPullRequest[] | null | undefined;
}) {
  return (
    <Part title={t('factoryRuns.definitionOpenPullRequests')}>
      <Show
        when={props.pullRequests}
        fallback={
          <p class="text-xs text-ink-muted">
            {t('factoryRuns.definitionPullRequestsUnlisted')}
          </p>
        }
      >
        {(pullRequests) => (
          <Show
            when={pullRequests().length > 0}
            fallback={
              <p class="text-xs text-ink-muted">
                {t('factoryRuns.definitionNoOpenPullRequests')}
              </p>
            }
          >
            <ul class="flex flex-col gap-0.5 text-sm">
              <For each={pullRequests()}>
                {(pullRequest) => (
                  <li class="[overflow-wrap:anywhere]">
                    <a
                      class="underline"
                      href={pullRequest.url}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {t('factoryRuns.definitionPullRequest', {
                        number: pullRequest.number,
                        title: pullRequest.title,
                      })}
                    </a>
                    <Show when={pullRequest.draft}>
                      <span class="text-ink-muted">
                        {' '}
                        {t('factoryRuns.definitionPullRequestDraft')}
                      </span>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        )}
      </Show>
    </Part>
  );
}
