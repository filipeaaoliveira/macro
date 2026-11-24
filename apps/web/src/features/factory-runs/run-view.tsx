import { StaticMarkdownContext } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import { createSignal, Show } from 'solid-js';
import { useRunsContext } from './context/runs-context';
import { createRunController } from './primitives/run-controller';
import { MobileRunSheet } from './views/mobile/run-sheet';
import { RunDialog } from './views/run-dialog';
import { RunPane } from './views/run-pane';
import { StepTerminalDialog } from './views/step-terminal-dialog';

/**
 * One Run, opened from a task: its state sits here, above the one device
 * switch, so the desktop dialog and the phone sheet share it, and the Step
 * Terminal either opens over it.
 */
export function RunView(props: { runId: string; onClose: () => void }) {
  // A Run it links (the Run it retries, its retry) opens in its place.
  const [opened, setOpened] = createSignal<string>();
  const controller = createRunController(
    useRunsContext(),
    () => opened() ?? props.runId,
    setOpened
  );

  return (
    <StaticMarkdownContext>
      <Show
        when={isTouchDevice()}
        fallback={<RunDialog controller={controller} onClose={props.onClose} />}
      >
        <MobileRunSheet controller={controller} onClose={props.onClose} />
      </Show>
      <Show when={controller.terminalStep()}>
        {(step) => <StepTerminalDialog controller={controller} step={step()} />}
      </Show>
    </StaticMarkdownContext>
  );
}

/**
 * One Run as a pane of its own, as the board and a Run's link open it, with
 * its Step Terminal over it. A Run it links (the Run it retries, its retry)
 * opens with `onOpenRun`, in the pane's place; without it, in the view's.
 */
export function RunPageView(props: {
  runId: string;
  onOpenRun?: (runId: string) => void;
  /** The entries of the team's Definition's Flow, by its name. */
  flowEntries?: (flow: string) => readonly string[] | undefined;
}) {
  const [opened, setOpened] = createSignal<string>();
  const controller = createRunController(
    useRunsContext(),
    () => (props.onOpenRun ? props.runId : (opened() ?? props.runId)),
    (runId) => (props.onOpenRun ? props.onOpenRun(runId) : setOpened(runId))
  );

  return (
    <StaticMarkdownContext>
      <RunPane controller={controller} flowEntries={props.flowEntries} />
      <Show when={controller.terminalStep()}>
        {(step) => <StepTerminalDialog controller={controller} step={step()} />}
      </Show>
    </StaticMarkdownContext>
  );
}
