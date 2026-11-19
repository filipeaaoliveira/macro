import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { PullRequestRuns } from '@service-factory/generated/schemas';
import { queryOptions, useQueries } from '@tanstack/solid-query';
import { type Accessor, createMemo } from 'solid-js';
import type {
  PullRequestLookup,
  PullRequestRun,
} from '../core/pull-request-run';
import { boardPollMs } from './board';
import { factoryRunKeys } from './keys';
import { toRun } from './runs';

/** The most pull requests one read names, as the factory reads them. */
const PULL_REQUESTS_PER_READ = 100;

/** A pull request's page as Runs are found by it, in any case. */
export function pullRequestKey(url: string): string {
  return url.toLowerCase();
}

/**
 * What one read answered: the pull requests it asked about, by key, the
 * Runs it found, and the keys whose Runs it could not all read.
 */
type PullRequestRead = {
  keys: string[];
  runs: PullRequestRun[];
  unknown: string[];
};

function toRead(keys: string[], found: PullRequestRuns): PullRequestRead {
  return {
    keys,
    runs: found.runs.map((found) => ({
      id: found.listed.detail.run.id,
      state: found.listed.state,
      taskName: found.listed.taskName ?? undefined,
      run: toRun(found.listed.detail),
      mergeGateSeq: found.mergeGateSeq ?? undefined,
    })),
    unknown: found.unknown.map(pullRequestKey),
  };
}

function pullRequestRunsOptions(keys: string[]) {
  return queryOptions({
    queryKey: factoryRunKeys.pullRequests(keys).queryKey,
    queryFn: async ({ signal }) =>
      toRead(
        keys,
        await throwOnErr(() => factoryClient.findPullRequestRuns(keys, signal))
      ),
    // Read fast while one of the Runs goes, slowly once they all ended, as
    // the board is.
    refetchInterval: (query) => boardPollMs(query.state.data?.runs ?? []),
  });
}

/**
 * The Runs that opened the pull requests at `urls`, read a hundred at a
 * time while the factory is on (`enabled`; undefined while that is not
 * known yet). A pull request is known only once a read answered for it,
 * kept through a read again that fails; until then it is `unknown`.
 */
export function usePullRequestRuns(
  urls: Accessor<readonly string[]>,
  enabled: Accessor<boolean | undefined>
) {
  const chunks = createMemo(() => {
    if (!enabled()) return [];
    const unique = [...new Set(urls().map(pullRequestKey))].sort();
    const chunks: string[][] = [];
    for (let at = 0; at < unique.length; at += PULL_REQUESTS_PER_READ) {
      chunks.push(unique.slice(at, at + PULL_REQUESTS_PER_READ));
    }
    return chunks;
  });
  const queries = useQueries(() => ({
    queries: chunks().map((chunk) => pullRequestRunsOptions(chunk)),
  }));

  // What each pull request's latest read said, kept across reads: a read
  // again that fails keeps its last good data, and a new set of pull
  // requests keeps what was read of the last one until its own read.
  const known = createMemo(
    (previous: ReadonlyMap<string, PullRequestLookup>) => {
      const next = new Map(previous);
      for (const query of queries) {
        const data = query.data;
        if (!data) continue;
        const runs = new Map<string, PullRequestRun>();
        for (const run of data.runs) {
          const url = run.run.pullRequestUrl;
          if (url) runs.set(pullRequestKey(url), run);
        }
        for (const key of data.keys) {
          const found = runs.get(key);
          if (found) next.set(key, { kind: 'run', found });
          else if (data.unknown.includes(key)) next.delete(key);
          else next.set(key, { kind: 'none' });
        }
      }
      return next;
    },
    new Map<string, PullRequestLookup>()
  );

  return {
    /** What is known of the Run that opened the pull request at `url`. */
    lookup: (url: string): PullRequestLookup => {
      const on = enabled();
      if (on === undefined) return { kind: 'unknown' };
      if (!on) return { kind: 'none' };
      return known().get(pullRequestKey(url)) ?? { kind: 'unknown' };
    },
  };
}
