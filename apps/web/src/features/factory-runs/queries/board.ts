import { useUserId } from '@core/context/user';
import { fireAndForget } from '@core/util/fireAndForget';
import { throwOnErr } from '@core/util/result';
import { useUserNames } from '@queries/auth/user-names';
import { useQuickAccessInitiativesQuery } from '@queries/soup/quick-access-initiatives';
import { factoryClient } from '@service-factory/client';
import type { BoardPage as BoardPageResponse } from '@service-factory/generated/schemas';
import {
  infiniteQueryOptions,
  keepPreviousData,
  type QueryClient,
  useInfiniteQuery,
} from '@tanstack/solid-query';
import { createMemo } from 'solid-js';
import type { BoardContext } from '../context/board-context';
import type { BoardFilters, BoardPage, BoardRun } from '../core/board';
import { factoryRunKeys } from './keys';
import { toRun, toSource } from './runs';
import { useStandingRulesQuery } from './standing-rules';

/** How many Runs a page of the board reads. */
export const BOARD_PAGE_SIZE = 25;

/** How often a board showing a Run that goes is read again. */
const BOARD_LIVE_POLL_MS = 15_000;

/**
 * How often a board of Runs that all ended is read again: a new Run, or one
 * a team member started, still shows within a minute.
 */
const BOARD_IDLE_POLL_MS = 60_000;

function toPage(page: BoardPageResponse): BoardPage {
  return {
    runs: page.runs.map((listed) => ({
      id: listed.detail.run.id,
      state: listed.state,
      taskName: listed.taskName ?? undefined,
      run: toRun(listed.detail),
    })),
    next: page.next ?? undefined,
  };
}

/** The Runs of loaded pages in order, each once: a page read again can repeat one. */
export function flattenBoardPages(pages: readonly BoardPage[]): BoardRun[] {
  const seen = new Set<string>();
  return pages.flatMap((page) =>
    page.runs.filter((listed) => {
      if (seen.has(listed.run.id)) return false;
      seen.add(listed.run.id);
      return true;
    })
  );
}

/** Read fast while a loaded Run goes, slowly once they all ended. */
export function boardPollMs(runs: readonly BoardRun[]): number {
  const going = runs.some((listed) => !listed.run.end);
  return going ? BOARD_LIVE_POLL_MS : BOARD_IDLE_POLL_MS;
}

export function boardOptions(filters: BoardFilters) {
  return infiniteQueryOptions({
    queryKey: factoryRunKeys.board(filters).queryKey,
    queryFn: async ({ pageParam, signal }) =>
      toPage(
        await throwOnErr(() =>
          factoryClient.listBoard(
            {
              scope: filters.scope,
              state: filters.state,
              flow: filters.flow,
              repoUrl: filters.repoUrl,
              projectId: filters.projectId,
              search: filters.search || undefined,
              afterCreatedAt: pageParam?.createdAt,
              afterId: pageParam?.id,
              limit: BOARD_PAGE_SIZE,
            },
            signal
          )
        )
      ),
    initialPageParam: undefined as BoardPage['next'],
    getNextPageParam: (page) => page.next,
    refetchInterval: (query) =>
      boardPollMs(flattenBoardPages(query.state.data?.pages ?? [])),
    // Other filters keep the last board on screen while they load.
    placeholderData: keepPreviousData,
    // Read again every few seconds: keep each row's nodes, and focus.
    reconcile: 'id',
  });
}

/** The board's context on the factory service and the app's caches. */
export function createAppBoardContext(_client: QueryClient): BoardContext {
  const viewerId = useUserId();
  return {
    board: (filters) => {
      const query = useInfiniteQuery(() => boardOptions(filters()));
      const runs = createMemo(() =>
        query.isPending || query.isLoadingError
          ? undefined
          : flattenBoardPages(query.data.pages)
      );
      return {
        state: () => {
          if (query.isPending) return 'pending';
          return runs() === undefined ? 'error' : 'ready';
        },
        data: runs,
        retry: () => fireAndForget(query.refetch(), 'factory-runs'),
        hasMore: () => query.hasNextPage,
        loadingMore: () => query.isFetchingNextPage,
        loadMore: () => {
          if (!query.hasNextPage || query.isFetchingNextPage) return;
          fireAndForget(query.fetchNextPage(), 'factory-runs');
        },
      };
    },

    queueLimit: () => {
      const rules = toSource(useStandingRulesQuery());
      return {
        ...rules,
        data: () => {
          const settings = rules.data();
          return settings
            ? (settings.definition.queueLimit ?? null)
            : undefined;
        },
      };
    },

    projects: () => {
      const { query, initiatives } = useQuickAccessInitiativesQuery();
      return {
        state: () => {
          if (query.isPending) return 'pending';
          return query.isError ? 'error' : 'ready';
        },
        data: () =>
          initiatives().map((initiative) => ({
            id: initiative.id,
            name: initiative.name,
          })),
        retry: () => fireAndForget(query.refetch(), 'factory-runs'),
      };
    },

    viewerId,

    personName: (userId) => {
      const names = useUserNames({ userIds: () => [userId()] });
      return () => {
        const name = names()[0]?.name;
        if (!name) return undefined;
        const full = [name.first_name, name.last_name]
          .filter(Boolean)
          .join(' ');
        return full || undefined;
      };
    },
  };
}
