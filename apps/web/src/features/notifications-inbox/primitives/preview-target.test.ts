import type { EntityData } from '@entity';
import type { UnifiedNotification } from '@notifications/types';
import { describe, expect, it } from 'vitest';
import type { NotificationInboxRow } from '../context/notifications-inbox-context';
import { previewTargetFor } from './preview-target';

function gateWaiting(id = 'gate-1'): UnifiedNotification {
  return {
    id,
    entity_type: 'document',
    entity_id: 'task-1',
    notification_event_type: 'factory_gate_waiting',
    notification_metadata: {
      tag: 'factory_gate_waiting',
      content: {
        runId: 'run-1',
        seq: 1,
        taskId: 'task-1',
        question: 'Approve this plan?',
        request: 'Add a helper',
      },
    },
    created_at: '2026-11-18T00:00:00Z',
    updated_at: '2026-11-18T00:00:00Z',
    sent: true,
    state: 'unseen',
  };
}
function taskAssigned(): UnifiedNotification {
  return {
    ...gateWaiting('assigned-1'),
    notification_event_type: 'task_assigned',
    notification_metadata: {
      tag: 'task_assigned',
      content: { taskId: 'task-1', taskName: 'Add a helper' },
    },
  } as UnifiedNotification;
}
function taskRow(notifications: UnifiedNotification[]): NotificationInboxRow {
  return {
    id: 'document:task-1',
    type: 'task',
    revision: 'rev-1',
    latestAt: 1,
    unseen: true,
    count: notifications.length,
    actions: [{ kind: 'open' }],
    content: {
      entity: {
        type: 'document',
        id: 'task-1',
        name: 'Add a helper',
        ownerId: 'viewer',
        fileType: 'md',
      } as EntityData,
      notifications,
    },
  };
}

// A desktop click or Enter previews the target; with none, the row opens.
describe('previewTargetFor', () => {
  it('has no preview for a row whose newest notification is a waiting Run', () => {
    expect(previewTargetFor(taskRow([gateWaiting()]))).toBeUndefined();
  });

  it('has no preview for a waiting Run picked from a stack', () => {
    const chosen = gateWaiting('gate-2');
    const row = taskRow([taskAssigned(), chosen]);
    expect(previewTargetFor(row, chosen)).toBeUndefined();
  });

  it("still previews the task for the row's other notifications", () => {
    const assigned = taskAssigned();
    expect(previewTargetFor(taskRow([assigned, gateWaiting()]))).toMatchObject({
      blockId: 'task-1',
    });
    expect(
      previewTargetFor(taskRow([gateWaiting(), assigned]), assigned)
    ).toMatchObject({ blockId: 'task-1' });
  });
});
