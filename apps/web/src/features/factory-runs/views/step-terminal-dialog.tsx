import { AgentSessionProvider } from '@app/features/block-agent/agent-session-provider';
import { Transcript } from '@app/features/block-agent/component/Transcript';
import { t } from '@app/lib/i18n';
import { StaticMarkdownContext } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import XIcon from '@icon/system/regular/x.svg';
import { ActionDialogShell, Badge, Button, Dialog } from '@ui';
import { createSignal, Show } from 'solid-js';
import { stepLabel } from '../components/run-timeline';
import { StepTerminalControls } from '../components/step-terminal';
import type { RunActionResult } from '../context/runs-context';
import {
  type RunStep,
  type SteerAction,
  steerableSession,
  terminalState,
} from '../core/run';
import type { RunController } from '../primitives/run-controller';

function notifySteer(result: RunActionResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.steerConflict'), {
      subtext: t('factoryRuns.steerConflictDescription'),
    });
    return;
  }
  toast.failure(t('factoryRuns.steerFailed'));
}

async function steer(controller: RunController, action: SteerAction) {
  notifySteer(await controller.steer(action));
}

/**
 * The agent session the Step works in, live, as Macro shows any agent
 * session's transcript: read-only here, since the person reaches the agent
 * through the Step Terminal, which the Step keeps a record of.
 */
function StepSessionTranscript(props: { sessionId: string }) {
  return (
    <StaticMarkdownContext>
      <AgentSessionProvider blockId={props.sessionId}>
        <div class="flex size-full min-h-0 @container/agent-session">
          <Transcript />
        </div>
      </AgentSessionProvider>
    </StaticMarkdownContext>
  );
}

/**
 * A running Step's Step Terminal: the live transcript of its agent's
 * session, and the controls that send the agent a note, pause, resume or
 * stop the Step. It stays open once the Step ends, saying so.
 */
export function StepTerminalDialog(props: {
  controller: RunController;
  step: RunStep;
}) {
  const [open, setOpen] = createSignal(true);
  // The session watched: the one the Step worked in when it opened, kept
  // while the Step runs in it.
  const sessionId = () => steerableSession(props.step);
  const state = () =>
    props.step.end || !sessionId() ? 'ended' : terminalState(props.step);

  const close = () => {
    if (!open()) return;
    setOpen(false);
    props.controller.closeTerminal();
  };

  return (
    <Dialog
      open={open()}
      onOpenChange={(next) => !next && close()}
      position="center"
      animate
      class="w-full max-w-[min(56rem,calc(100vw-16px))] overflow-hidden"
    >
      <ActionDialogShell class="h-[min(48rem,calc(100dvh-16px))]">
        <div class="flex shrink-0 items-center justify-between gap-3 border-b border-edge-divider px-5 py-4">
          <Dialog.Title class="min-w-0 truncate text-lg font-semibold">
            {t('factoryRuns.terminalTitle', { step: stepLabel(props.step) })}
          </Dialog.Title>
          <div class="flex shrink-0 items-center gap-2">
            <Show when={state() === 'paused' || state() === 'stopping'}>
              <Badge variant="outline" size="xs">
                {state() === 'paused'
                  ? t('factoryRuns.stepPaused')
                  : t('factoryRuns.stepStopping')}
              </Badge>
            </Show>
            <Dialog.CloseButton
              as={Button}
              aria-label={t('factoryRuns.closeTerminal')}
              tooltip={{
                content: t('factoryRuns.closeTerminal'),
                disabled: true,
              }}
              variant="ghost"
              size="icon-sm"
            >
              <XIcon aria-hidden="true" class="size-4" />
            </Dialog.CloseButton>
          </div>
        </div>
        <div class="relative flex min-h-0 flex-1" data-step-terminal-transcript>
          <Show when={sessionId()}>
            {(id) => <StepSessionTranscript sessionId={id()} />}
          </Show>
        </div>
        <ActionDialogShell.Footer>
          <div class="w-full">
            <StepTerminalControls
              state={state()}
              note={props.controller.terminalNote()}
              onNoteChange={props.controller.setTerminalNote}
              steering={props.controller.steering()}
              onSteer={(action) =>
                fireAndForget(steer(props.controller, action), 'factory-runs')
              }
            />
          </div>
        </ActionDialogShell.Footer>
      </ActionDialogShell>
    </Dialog>
  );
}
