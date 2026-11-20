import { getNotificationActionText } from '@entity/utils/notification';
import { describe, expect, it } from 'vitest';
import { getNotificationAction } from './notification-metadata';
import type { UnifiedNotification } from './types';

/** A factory Gate's notification, notify-only or not. */
function factoryGate(
  notifyOnly?: boolean,
  budgetStopped?: boolean
): UnifiedNotification {
  return {
    notification_metadata: {
      tag: 'factory_gate_waiting',
      content: {
        runId: '00000000-0000-7000-8000-000000000001',
        seq: 3,
        taskId: '00000000-0000-7000-8000-000000000002',
        request: 'Fix the login bug',
        question: 'The plan is ready.',
        notifyOnly,
        budgetStopped,
      },
    },
  } as UnifiedNotification;
}

describe('a factory Gate notification', () => {
  it('waits on an answer, unless the Gate only tells the owner', () => {
    expect(getNotificationAction(factoryGate())).toBe(
      'waits on your answer in'
    );
    expect(getNotificationAction(factoryGate(false))).toBe(
      'waits on your answer in'
    );
    expect(getNotificationAction(factoryGate(true))).toBe(
      'for your information — the run goes on in'
    );
  });

  it('says a Run its token budget ended stopped, and waits on no answer', () => {
    const stopped = factoryGate(false, true);
    expect(getNotificationAction(stopped)).toBe(
      'stopped at its token budget in'
    );
    expect(getNotificationActionText(stopped)).toBe('stopped');
    expect(getNotificationActionText(factoryGate())).toBe('waiting');
  });
});
