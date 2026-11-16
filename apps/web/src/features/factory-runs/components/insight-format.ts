import { formatLocale } from '@app/lib/i18n';
import { durationIn } from '../core/insights';

/** A duration in the unit it reads best in: `45 sec`, `12 min`, `3.5 hr`. */
export function formatInsightDuration(seconds: number): string {
  const { value, unit } = durationIn(seconds);
  return new Intl.NumberFormat(formatLocale(), {
    style: 'unit',
    unit,
    unitDisplay: 'short',
    maximumFractionDigits: value < 10 ? 1 : 0,
  }).format(value);
}

/** A token count, shortened past a thousand: `950`, `12K`, `1.4M`. */
export function formatTokens(tokens: number): string {
  return new Intl.NumberFormat(formatLocale(), {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(tokens);
}

/** A share as a percentage: `83%`. */
export function formatShare(share: number): string {
  return new Intl.NumberFormat(formatLocale(), {
    style: 'percent',
    maximumFractionDigits: 0,
  }).format(share);
}

/** When a Run was created, as a list names it: `Nov 12`. */
export function formatRunDate(value: string): string {
  return new Intl.DateTimeFormat(formatLocale(), {
    month: 'short',
    day: 'numeric',
  }).format(new Date(value));
}
