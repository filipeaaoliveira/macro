import { StaticMarkdownContext } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import { Show } from 'solid-js';
import { useRunsContext } from './context/runs-context';
import { createRunController } from './primitives/run-controller';
import { MobileRunSheet } from './views/mobile/run-sheet';
import { RunDialog } from './views/run-dialog';

/**
 * One Run, opened from a task: its state sits here, above the one device
 * switch, so the desktop dialog and the phone sheet share it.
 */
export function RunView(props: { runId: string; onClose: () => void }) {
  const controller = createRunController(useRunsContext(), () => props.runId);

  return (
    <StaticMarkdownContext>
      <Show
        when={isTouchDevice()}
        fallback={<RunDialog controller={controller} onClose={props.onClose} />}
      >
        <MobileRunSheet controller={controller} onClose={props.onClose} />
      </Show>
    </StaticMarkdownContext>
  );
}
