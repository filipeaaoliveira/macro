import { formatLocale } from '@app/lib/i18n';

/** When a Run started or ended, in full, in the interface's language. */
export function formatRunTime(value: string): string {
  return new Intl.DateTimeFormat(formatLocale(), {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(value));
}
