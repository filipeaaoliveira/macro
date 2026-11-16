import { Card } from '@app/features/agent-cards/components/kit/card';
import { t } from '@app/lib/i18n';
import { MobileDrawer } from '@components/app/mobile/MobileDrawer';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import { createSignal, createUniqueId, Match, Show, Switch } from 'solid-js';
import {
  GateAnswers,
  GateGroup,
  GateNote,
  GateQuestion,
} from '../../components/gate-card';
import { runTitle } from '../../core/run';
import { createFocusRescue } from '../../primitives/focus-rescue';
import type { RunController } from '../../primitives/run-controller';
import {
  answerGate,
  RunCancel,
  RunContent,
  RunLoadFailed,
} from '../shared/run-parts';
import { formatRunTime } from '../shared/run-time';

/**
 * The Run on a phone. Its pinned chrome, between the handle and the
 * scrolling content, holds only actions: the note and answers for the Gate
 * it waits on, and Cancel run. The Gate's question opens the scrolling
 * content, above the plan it asks about, so a long question or a phone
 * keyboard never pushes the answers off the sheet.
 */
export function MobileRunSheet(props: {
  controller: RunController;
  onClose: () => void;
}) {
  const [open, setOpen] = createSignal(true);
  const questionId = createUniqueId();
  let content: HTMLDivElement | undefined;
  let status: HTMLElement | undefined;
  const source = props.controller.source;
  createFocusRescue(
    props.controller,
    () => content,
    () => status
  );

  const title = () => {
    const run = source.data();
    return run ? runTitle(run) : t('factoryRuns.runDetails');
  };

  return (
    <MobileDrawer
      side="bottom"
      open={open()}
      onOpenChange={(next) => !next && setOpen(false)}
      // Unmount once the sheet has slid out.
      onContentPresentChange={(present) => {
        if (!present) queueMicrotask(props.onClose);
      }}
    >
      <MobileDrawer.Portal>
        <MobileDrawer.Overlay />
        <MobileDrawer.Content
          ref={(element: HTMLDivElement) => {
            content = element;
          }}
        >
          <MobileDrawer.Handle class="pb-1" />
          <MobileDrawer.Title class="shrink-0 truncate px-5 pb-3 text-lg font-semibold text-ink">
            {title()}
          </MobileDrawer.Title>
          <Show when={source.data()}>
            {(run) => (
              <Show when={!run().end}>
                <div class="flex shrink-0 flex-col gap-3 px-5 pb-3">
                  <Show when={props.controller.waiting()}>
                    {(waiting) => (
                      <GateGroup
                        class="flex flex-col gap-3"
                        describedBy={questionId}
                      >
                        <GateNote
                          note={props.controller.note()}
                          onNoteChange={props.controller.setNote}
                          locked={props.controller.gateLocked()}
                        />
                        <div class="flex flex-wrap items-center justify-end gap-2">
                          <GateAnswers
                            answers={waiting().gate.answers}
                            locked={props.controller.gateLocked()}
                            answering={props.controller.answering()}
                            onAnswer={(value) =>
                              answerGate(props.controller, value)
                            }
                          />
                        </div>
                      </GateGroup>
                    )}
                  </Show>
                  <div class="flex justify-end">
                    <RunCancel controller={props.controller} />
                  </div>
                </div>
              </Show>
            )}
          </Show>
          <MobileDrawer.ScrollBody>
            <div class="flex flex-col gap-5 border-t border-edge-divider px-5 pt-4 pb-6">
              <Switch>
                <Match when={source.state() === 'pending'}>
                  <div class="flex justify-center">
                    <LoadingSpinner />
                  </div>
                </Match>
                <Match when={source.state() === 'error'}>
                  <RunLoadFailed onRetry={source.retry} />
                </Match>
                <Match when={source.data()}>
                  {(run) => (
                    <>
                      <Show when={props.controller.waiting()}>
                        {(waiting) => (
                          <Card width="wide" data-agent-card="gate-question">
                            <GateQuestion
                              id={questionId}
                              question={waiting().gate.question}
                            />
                          </Card>
                        )}
                      </Show>
                      <RunContent
                        controller={props.controller}
                        run={run()}
                        time={formatRunTime}
                        statusRef={(element) => {
                          status = element;
                        }}
                      />
                    </>
                  )}
                </Match>
              </Switch>
            </div>
          </MobileDrawer.ScrollBody>
        </MobileDrawer.Content>
      </MobileDrawer.Portal>
    </MobileDrawer>
  );
}
