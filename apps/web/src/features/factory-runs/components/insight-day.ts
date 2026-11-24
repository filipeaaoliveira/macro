import { formatLocale } from '@app/lib/i18n';

/** A day of the period, as an axis or a table names it. */
export function formatInsightDay(date: string, long = false): string {
  return new Intl.DateTimeFormat(formatLocale(), {
    timeZone: 'UTC',
    month: 'short',
    day: 'numeric',
    ...(long ? { weekday: 'short' } : {}),
  }).format(new Date(`${date}T12:00:00Z`));
}
