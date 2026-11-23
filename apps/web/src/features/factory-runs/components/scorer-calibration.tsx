import { t } from '@app/lib/i18n';
import { Badge } from '@ui';
import { Show } from 'solid-js';
import { calibrationReading, type ScorerCalibration } from '../core/evaluation';
import { formatShare } from './insight-format';

/**
 * How far a Scorer's judge agrees with the person's labels, in a line:
 * calibrated, the labels it still needs, or too little agreement.
 */
export function calibrationLine(calibration: ScorerCalibration): string {
  const reading = calibrationReading(calibration);
  switch (reading.kind) {
    case 'calibrated':
      return t('factoryEvaluation.calibratedLine', {
        agreement: formatShare(reading.agreement),
        count: calibration.labelled,
      });
    case 'needsLabels':
      return t('factoryEvaluation.needsLabelsLine', {
        count: reading.remaining,
      });
    case 'disagrees':
      return t('factoryEvaluation.disagreesLine', {
        agreement: formatShare(reading.agreement),
        threshold: formatShare(reading.threshold),
      });
  }
}

/**
 * Marks a Scorer whose judge is not calibrated yet, so nothing reads its
 * results as settled; a calibrated one is marked as such.
 */
export function CalibrationBadge(props: { calibration: ScorerCalibration }) {
  return (
    <Show
      when={props.calibration.calibrated}
      fallback={
        <Badge variant="outline" size="xs">
          {t('factoryEvaluation.uncalibrated')}
        </Badge>
      }
    >
      <Badge variant="success" size="xs">
        {t('factoryEvaluation.calibrated')}
      </Badge>
    </Show>
  );
}
