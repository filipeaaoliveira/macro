import { createEffect, on, Show } from 'solid-js';
import { useBoardContext } from './context/board-context';
import { useRunsContext } from './context/runs-context';
import { createBoardState } from './primitives/board-state';
import { RunView } from './run-view';
import { BoardView } from './views/board';

/**
 * The factory's board: its state sits here, and a Run opened from it opens
 * as from a task.
 */
export function FactoryBoardView(props: {
  /** A Run to open at once, as a link to it does. */
  openRunId?: string;
}) {
  const board = useBoardContext();
  const runs = useRunsContext();
  const state = createBoardState();
  createEffect(
    on(
      () => props.openRunId,
      (runId) => {
        if (runId) state.setOpenRunId(runId);
      }
    )
  );
  const source = board.board(state.filters);
  const flows = runs.flows();
  const projects = board.projects();
  const queueLimit = board.queueLimit();
  const waiting = runs.waitingRuns();
  const queue = {
    ...waiting,
    data: () => {
      const places = waiting.data();
      if (!places) return undefined;
      return {
        held: places.waiting.length + places.starting.length,
        waiting: places.waiting.length,
      };
    },
  };
  // The repositories to filter by: those of the Runs shown, and the one
  // picked even once no Run shown is on it.
  const repositories = () => {
    const urls = new Set<string>();
    for (const listed of source.data() ?? []) {
      const url = listed.run.repository?.url;
      if (url) urls.add(url);
    }
    const picked = state.repoUrl();
    if (picked) urls.add(picked);
    return [...urls].sort();
  };
  const flowNames = () => {
    const names = new Set((flows.data() ?? []).map((flow) => flow.name));
    const picked = state.flow();
    if (picked) names.add(picked);
    return [...names].sort();
  };

  return (
    <>
      <BoardView
        state={state}
        source={source}
        flows={flowNames()}
        repositories={repositories()}
        projects={projects.data() ?? []}
        queue={queue}
        queueLimit={queueLimit}
      />
      <Show when={state.openRunId()} keyed>
        {(runId) => (
          <RunView
            runId={runId}
            onClose={() => state.setOpenRunId(undefined)}
          />
        )}
      </Show>
    </>
  );
}
