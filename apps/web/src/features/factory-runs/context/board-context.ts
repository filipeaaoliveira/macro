import type { Accessor } from 'solid-js';
import { createContext, useContext } from 'solid-js';
import type { BoardFilters, BoardRun } from '../core/board';
import type { RunsSource } from './runs-context';

/** The board's Runs as loaded so far, and the pages after them. */
export type BoardSource = RunsSource<BoardRun[]> & {
  /** Whether another page follows the loaded ones. */
  hasMore: Accessor<boolean>;
  loadingMore: Accessor<boolean>;
  loadMore: () => void;
};

/** A Project the person can filter the board by. */
export type BoardProject = { id: string; name: string };

/** What the board needs from the app beyond the Runs views'. */
export type BoardContext = {
  /** The Runs `filters` name, newest first; kept current while they go. */
  board: (filters: Accessor<BoardFilters>) => BoardSource;
  /** The most places the person's Queue holds; `null` for no limit. */
  queueLimit: () => RunsSource<number | null>;
  /** The Projects the person can see, to filter by. */
  projects: () => RunsSource<BoardProject[]>;
  /** The signed-in person's user id, to tell their Runs from their team's. */
  viewerId: Accessor<string | undefined>;
  /** A task's name, once read; none for a task the person can't see. */
  taskName: (taskId: Accessor<string>) => Accessor<string | undefined>;
  /** A person's name, once read. */
  personName: (userId: Accessor<string>) => Accessor<string | undefined>;
};

const Context = createContext<BoardContext>();

export const BoardProvider = Context.Provider;

export function useBoardContext(): BoardContext {
  const context = useContext(Context);
  if (!context) throw new Error('BoardProvider is required');
  return context;
}
