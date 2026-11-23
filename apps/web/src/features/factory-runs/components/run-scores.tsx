import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Badge, Button } from '@ui';
import { createSignal, For, Match, Show, Switch } from 'solid-js';
import { useRunsContext } from '../context/runs-context';
import { labelDisagrees, type RunScore } from '../core/evaluation';
import { CalibrationBadge, calibrationLine } from './scorer-calibration';

function ScoreRow(props: { runId: string; score: RunScore }) {
  const context = useRunsContext();
  const [labelling, setLabelling] = createSignal(false);
  const labelled = () => props.score.label?.passed;

  const label = async (passed: boolean) => {
    if (labelling() || labelled() === passed) return;
    setLabelling(true);
    const result = await context.labelRunScore({
      runId: props.runId,
      scorer: props.score.scorer,
      passed,
    });
    setLabelling(false);
    if (!result.ok) toast.failure(t('factoryRuns.labelScoreFailed'));
  };

  return (
    <li class="flex flex-col gap-1.5 py-2">
      <div class="flex flex-wrap items-center gap-2">
        <h4 class="text-sm font-medium">{props.score.scorer}</h4>
        <span class="text-sm tabular-nums">{props.score.score.toFixed(2)}</span>
        <Badge variant={props.score.passed ? 'success' : 'failure'} size="xs">
          {props.score.passed
            ? t('factoryRuns.scorePassed')
            : t('factoryRuns.scoreFailed')}
        </Badge>
        <CalibrationBadge calibration={props.score.calibration} />
      </div>
      <p class="text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
        {props.score.reason}
      </p>
      <p class="text-xs text-ink-muted">
        {calibrationLine(props.score.calibration)}
      </p>
      <div
        role="group"
        aria-label={t('factoryRuns.labelScore', {
          scorer: props.score.scorer,
        })}
        class="flex flex-wrap items-center gap-2"
      >
        <span class="text-xs text-ink-muted">
          {t('factoryRuns.labelScoreQuestion')}
        </span>
        <Button
          type="button"
          variant={labelled() === true ? 'secondary' : 'ghost'}
          size="xs"
          aria-pressed={labelled() === true}
          disabled={labelling()}
          onClick={() => fireAndForget(label(true), 'factory-runs')}
        >
          {t('factoryRuns.labelShouldPass')}
        </Button>
        <Button
          type="button"
          variant={labelled() === false ? 'secondary' : 'ghost'}
          size="xs"
          aria-pressed={labelled() === false}
          disabled={labelling()}
          onClick={() => fireAndForget(label(false), 'factory-runs')}
        >
          {t('factoryRuns.labelShouldFail')}
        </Button>
      </div>
      <Show when={labelDisagrees(props.score)}>
        <p class="text-xs text-ink-muted">{t('factoryRuns.labelDisagrees')}</p>
      </Show>
      <Show when={!props.score.byCurrentJudge}>
        <p class="text-xs text-ink-muted">
          {t('factoryRuns.scoreByEarlierJudge')}
        </p>
      </Show>
    </li>
  );
}

/**
 * The live Scorers' judgments of the person's own ended Run, each with their
 * label of it: whether the Run should have passed on the Scorer's
 * dimension. Their labels calibrate the Scorer. Nothing shows while no
 * Scorer judged the Run.
 */
export function RunScores(props: { runId: string; endedAt: string }) {
  const context = useRunsContext();
  const source = context.runScores(
    () => props.runId,
    () => true,
    () => props.endedAt
  );

  return (
    <Switch>
      <Match when={source.state() === 'error'}>
        <div class="flex items-center gap-2">
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.scoresLoadFailed')}
          </p>
          <Button variant="ghost" size="xs" onClick={() => source.retry()}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={(source.data() ?? []).length > 0}>
        <section class="flex flex-col gap-1">
          <h3 class="text-xs font-medium text-ink-muted">
            {t('factoryRuns.scores')}
          </h3>
          <ul
            aria-label={t('factoryRuns.scores')}
            class="flex flex-col divide-y divide-edge-muted"
          >
            <For each={source.data()}>
              {(score) => <ScoreRow runId={props.runId} score={score} />}
            </For>
          </ul>
        </section>
      </Match>
    </Switch>
  );
}
