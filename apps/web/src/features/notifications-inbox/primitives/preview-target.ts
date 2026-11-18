import {
  type PreviewPanelSelection,
  previewBlockTarget,
} from '@components/app/previewTarget';
import type { EntityData } from '@entity';
import type { UnifiedNotification } from '@notifications/types';
import type { NotificationInboxRow } from '../context/notifications-inbox-context';
import { agentTurnParams, crmCommentParams } from './notification-location';
import { rowEntity, runNotification } from './row-entity';

/**
 * What the preview pane can show inline; calendar events show through the
 * view's calendar route, and projects open in their own view.
 */
const previewSelection = (
  entity: EntityData
): PreviewPanelSelection | undefined =>
  entity.type === 'calendar_event' ||
  entity.type === 'initiative' ||
  entity.type === 'harness' ||
  entity.type === 'team'
    ? undefined
    : entity;

/**
 * What the preview shows for a row or one of its notifications; undefined when
 * it has no inline preview. An agent's notification opens at its turn, and a
 * company or contact at its comment. A waiting factory Run has none: it opens
 * at the Run, not the task its notification is filed under.
 */
export const previewTargetFor = (
  row: NotificationInboxRow,
  notification?: UnifiedNotification
) => {
  if (runNotification(row, notification)) return undefined;
  const entity = previewSelection(rowEntity(row, notification));
  if (!entity) return undefined;
  const target = previewBlockTarget(entity);
  const params =
    (notification && agentTurnParams(notification)) ??
    crmCommentParams(notification ?? row.content.notifications[0]);
  return params ? { ...target, params } : target;
};
