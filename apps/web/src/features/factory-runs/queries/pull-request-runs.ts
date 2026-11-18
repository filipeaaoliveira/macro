import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { PullRequestRuns } from '@service-factory/generated/schemas';
import { queryOptions, useQueries } from '@tanstack/solid-query';
import { type Accessor, createMemo } from 'solid-js';
import type { PullRequestRun } from '../core/pull-request-run';
import { boardPollMs } from './board';
import { factoryRunKeys } from './keys';
import { toRun } from './runs';

/** The most pull requests one read names, as the factory reads them. */
const PULL_REQUESTS_PER_READ = 100;

/** A pull request's page as Runs are found by it, in any case. */
export function pullRequestKey(url: string): string {
  return url.toLowerCase();
}

function toPullRequestRuns(found: PullRequestRuns): PullRequestRun[] {
  return found.runs.map((found) => ({
    id: found.listed.detail.run.id,
    state: found.listed.state,
    taskName: found.listed.taskName ?? undefined,
    run: toRun(found.listed.detail),
    mergeGateSeq: found.mergeGateSeq ?? undefined,
  }));
}

function pullRequestRunsOptions(urls: readonly string[]) {
  return queryOptions({
    queryKey: factoryRunKeys.pullRequests(urls).queryKey,
    queryFn: async ({ signal }) =>
      toPullRequestRuns(
        await throwOnErr(() =>
          factoryClient.findPullRequestRuns([...urls], signal)
        )
      ),
    // Read fast while one of the Runs goes, slowly once they all ended, as
    // the board is.
    refetchInterval: (query) => boardPollMs(query.state.data ?? []),
  });
}

/**
 * The Runs that opened the pull requests at `urls`, read a hundred at a
 * time while `enabled`: none for a pull request no Run the person can see
 * opened.
 */
export function usePullRequestRuns(
  urls: Accessor<readonly string[]>,
  enabled: Accessor<boolean>
) {
  const chunks = createMemo(() => {
    if (!enabled()) return [];
    const unique = [...new Set(urls())].sort();
    const chunks: string[][] = [];
    for (let at = 0; at < unique.length; at += PULL_REQUESTS_PER_READ) {
      chunks.push(unique.slice(at, at + PULL_REQUESTS_PER_READ));
    }
    return chunks;
  });
  const queries = useQueries(() => ({
    queries: chunks().map((chunk) => pullRequestRunsOptions(chunk)),
  }));

  const found = createMemo(() => {
    const runs = new Map<string, PullRequestRun>();
    for (const query of queries) {
      if (!query.isSuccess) continue;
      for (const run of query.data) {
        const url = run.run.pullRequestUrl;
        if (url) runs.set(pullRequestKey(url), run);
      }
    }
    return runs;
  });

  return {
    /** The Run that opened the pull request at `url`, once read. */
    runFor: (url: string): PullRequestRun | undefined =>
      found().get(pullRequestKey(url)),
  };
}
