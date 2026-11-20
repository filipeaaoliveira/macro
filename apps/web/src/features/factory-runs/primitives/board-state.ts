import { createMemo, createSignal, onCleanup, untrack } from 'solid-js';
import type { BoardFilters, BoardScope, BoardState } from '../core/board';

/** How long the board waits after the last keystroke to search. */
const SEARCH_DEBOUNCE_MS = 300;

const sameFilters = (a: BoardFilters, b: BoardFilters) =>
  a.scope === b.scope &&
  a.state === b.state &&
  a.flow === b.flow &&
  a.repoUrl === b.repoUrl &&
  a.projectId === b.projectId &&
  a.search === b.search;

/**
 * Whose Runs the board shows, what narrows it, the words searched for, and
 * the Run opened from it. The search reads once typing pauses.
 */
export function createBoardState() {
  const [scope, setScope] = createSignal<BoardScope>('mine');
  const [state, setState] = createSignal<BoardState>();
  const [flow, setFlow] = createSignal<string>();
  const [repoUrl, setRepoUrl] = createSignal<string>();
  const [projectId, setProjectId] = createSignal<string>();
  const [searchText, setSearchTextNow] = createSignal('');
  const [search, setSearch] = createSignal('');
  const [openRunId, setOpenRunId] = createSignal<string>();

  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  const setSearchText = (value: string) => {
    setSearchTextNow(value);
    clearTimeout(timer);
    timer = setTimeout(() => setSearch(value.trim()), SEARCH_DEBOUNCE_MS);
  };

  const read = (): BoardFilters => ({
    scope: scope(),
    state: state(),
    flow: flow(),
    repoUrl: repoUrl(),
    projectId: projectId(),
    search: search() || undefined,
  });
  const filters = createMemo<BoardFilters>(read, untrack(read), {
    equals: sameFilters,
  });

  return {
    scope,
    setScope,
    state,
    setState,
    flow,
    setFlow,
    repoUrl,
    setRepoUrl,
    projectId,
    setProjectId,
    searchText,
    setSearchText,
    filters,
    openRunId,
    setOpenRunId,
    /** Clears the Flow, repository and Project; whose Runs and the state stay. */
    clearFilters: () => {
      setFlow(undefined);
      setRepoUrl(undefined);
      setProjectId(undefined);
    },
  };
}

export type BoardViewState = ReturnType<typeof createBoardState>;
