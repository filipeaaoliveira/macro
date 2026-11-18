vi.mock('../notification-helpers', () => ({
  isChannelNotification: () => false,
}));
vi.mock('../notification-source', () => ({
  CHANNEL_EVENT_TYPES: [
    'channel_mention',
    'channel_message_send',
    'channel_message_reply',
    'document_mention',
  ],
}));
vi.mock('../notification-stacking', () => ({
  getMostRecentNotification: vi.fn(),
  stackNotifications: vi.fn(),
}));

const getNotificationById = vi.hoisted(() => vi.fn());
const track = vi.hoisted(() => vi.fn());

vi.mock('@app/lib/analytics', () => ({ analytics: { track } }));

vi.mock('@app/features/calendar-view/calendar-range', () => ({
  createCalendarRange: vi.fn(),
}));
vi.mock('@block-channel/utils/link', () => ({
  getChannelParams: vi.fn(),
  navigateToChannelMessage: vi.fn(),
}));
vi.mock('@core/constant/allBlocks', () => ({
  itemToBlockName: (value: { fileType: string }) => value.fileType,
  resolveBlockAlias: (type: string) => type,
}));
vi.mock('@core/constant/featureFlags', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@core/constant/featureFlags')>()),
  isFeatureEnabled: () => false,
  USE_MACRO_PR_SUMMARY_BLOCK: true,
}));
vi.mock('@core/util/url', () => ({
  buildSimpleEntityUrl: ({ type, id }: { type: string; id: string }) =>
    `https://macro.com/app/${type}/${id}`,
  openExternalUrl: vi.fn(),
}));
vi.mock('@queries/notification/user-notifications', () => ({
  getNotificationById,
}));

import type { SplitManager } from '@components/app/split-layout/layoutManager';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  openNotification,
  openNotificationFromId,
} from '../notification-navigation';
import type { NotificationSource } from '../notification-source';
import type { UnifiedNotification } from '../types';

function gateWaiting(): UnifiedNotification {
  return {
    id: 'gate-1',
    entity_id: 'task-1',
    entity_type: 'document',
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

const runContent = expect.objectContaining({
  type: 'component',
  id: 'factory-run',
  params: { runId: 'run-1' },
});

describe('factory gate notification navigation', () => {
  const openWithSplit = vi.fn();
  const layout = {
    getSplitByContent: vi.fn(() => undefined),
    openWithSplit,
  } as unknown as SplitManager;
  beforeEach(() => {
    openWithSplit.mockReset();
    getNotificationById.mockReset();
  });

  it("opens the Run, not the task, from the app's notification lists", async () => {
    const result = await openNotification(gateWaiting(), layout, false, {
      fileType: undefined,
    });
    expect(result.isOk()).toBe(true);
    expect(openWithSplit).toHaveBeenCalledExactlyOnceWith(
      runContent,
      expect.objectContaining({ activate: true })
    );
  });

  it('opens the Run from a push notification tap', async () => {
    getNotificationById.mockResolvedValue(gateWaiting());
    const source = { notifications: () => [] } as unknown as NotificationSource;
    const result = await openNotificationFromId('gate-1', layout, source);
    expect(result.isOk()).toBe(true);
    expect(openWithSplit).toHaveBeenCalledExactlyOnceWith(
      runContent,
      expect.objectContaining({ activate: true })
    );
  });
});
