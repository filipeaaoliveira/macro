/** @vitest-environment jsdom */
import {
  type PrMergeGuard,
  usePrMergeGuard,
} from '@block-pr/primitives/pr-merge-guard-context';
import { cleanup, render, screen } from '@solidjs/testing-library';
import type { ParentProps } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppProviders } from './AppProviders';

// The app's merge guard, as the factory would give it: it refuses every
// merge, so a view that gets it shows that.
const factory = vi.hoisted(() => ({
  urls: [] as string[][],
}));

vi.mock('@app/features/factory-runs/pr-merge-guard', () => ({
  factoryMergeGuard: (urls: () => readonly string[]): PrMergeGuard => {
    factory.urls.push([...urls()]);
    return {
      mark: () => null,
      section: () => null,
      merge: () => true,
      mergeRefusal: () => 'Only its run’s owner can merge this',
      mergeWarning: () => undefined,
      gate: () => null,
    };
  },
}));

// The rest of AppProviders' state and effects are beside the point here.
const { passThrough, nothing } = vi.hoisted(() => ({
  passThrough: (props: ParentProps) => props.children,
  nothing: () => undefined,
}));
vi.mock(
  '@app/features/access-requests/use-access-request-invalidations',
  () => ({ useAccessRequestInvalidations: nothing })
);
vi.mock('@app/features/agent-navigation/views/agent-navigation', () => ({
  AgentNavigationProvider: passThrough,
}));
vi.mock('@app/features/meetings/incoming-meeting-invitations', () => ({
  IncomingMeetingInvitationsProvider: passThrough,
}));
vi.mock('@app/features/notification-sounds/core/quiet-hours', () => ({
  quietHoursMute: () => false,
}));
vi.mock('@app/features/notification-sounds/primitives/sound-settings', () => ({
  readNotificationSoundSettings: () => ({ quietHours: null }),
}));
vi.mock('@app/features/notification-sounds/use-notification-sounds', () => ({
  notificationSoundFor: () => null,
  useNotificationSounds: nothing,
}));
vi.mock(
  '@app/features/notifications/PendingNotificationNavigationEffect',
  () => ({ usePendingNotificationNavigationEffect: nothing })
);
vi.mock('@app/features/routines/routine-drafts-host', () => ({
  RoutineDraftsHost: passThrough,
}));
vi.mock('@app/features/soup/search/context', () => ({
  SearchProvider: passThrough,
}));
vi.mock('@app/lib/analytics/posthog', () => ({
  usePosthog: () => ({ flagsLoaded: () => true }),
  useFeatureFlag: () => () => ({ enabled: true, loading: false }),
}));
vi.mock('@app/lib/i18n/use-interface-language-sync', () => ({
  useInterfaceLanguageSync: nothing,
}));
vi.mock('@app/lib/queries/calendar/graphql/use-calendar-cache', () => ({
  useCalendarCache: nothing,
}));
vi.mock('@app/lib/queries/invalidate-on-reconnect', () => ({
  useInvalidateQueriesOnReconnect: nothing,
}));
vi.mock('@app/lib/queries/soup/backfill', () => ({
  useSoupBackfills: nothing,
}));
vi.mock('@app/lib/tauri/notification', () => ({
  MacOSNotificationUpdateRequiredError: class extends Error {},
}));
vi.mock('@app/signal/splitLayout', () => ({ globalSplitManager: nothing }));
vi.mock('@components/app/GlobalAppState', () => ({
  GlobalAppStateProvider: passThrough,
}));
vi.mock('@components/app/useReactiveFavicon', () => ({
  useReactiveFavicon: nothing,
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: vi.fn(), success: vi.fn() },
}));
vi.mock('@core/context/team', () => ({ TeamContextProvider: passThrough }));
vi.mock('@core/context/user', () => ({
  useIsAuthenticated: () => () => true,
  useUserId: () => () => 'macro|julia@example.com',
  useUserInfo: () => () => undefined,
}));
vi.mock('@core/mobile/isNativeMobilePlatform', () => ({
  isNativeMobilePlatform: () => false,
}));
vi.mock('@core/orchestrator', () => ({ createBlockOrchestrator: nothing }));
vi.mock('@notifications', () => ({
  createNotificationSource: nothing,
  useNotificationUpdates: nothing,
  usePlatformNotificationState: () => 'not-supported',
}));
vi.mock('@notifications/notification-platform', () => ({
  maybeHandlePlatformNotification: nothing,
}));
vi.mock('@queries/chat', () => ({ useChatRenameWebsocketSync: nothing }));
vi.mock('@queries/prompt-dismissals', () => ({
  useWarmPromptDismissals: nothing,
}));
vi.mock('@queries/sync/use-query-sync', () => ({ useQuerySync: nothing }));
vi.mock('@queries/undo', () => ({ MutationUndoProvider: passThrough }));
vi.mock('@service-connection/client', () => ({
  useRefreshTrackedEntitiesOnFocus: nothing,
  useReopenTrackedEntitiesOnReconnect: nothing,
}));
vi.mock('@service-storage/util/pendingFolderUploads', () => ({
  useResumeFolderUploads: nothing,
}));
// An ownerless dialog renders in the host: one with a pull request in it
// asks for the guard there.
vi.mock('@ui', () => ({
  ImperativeDialogHost: () => {
    const guard = usePrMergeGuard(() => [DIALOG_PR]);
    return <p>{guard.mergeRefusal(DIALOG_PR) ?? 'Dialog merges on GitHub'}</p>;
  },
}));

const PR = 'https://github.com/acme/shop/pull/7';
const DIALOG_PR = 'https://github.com/acme/shop/pull/8';

function PullRequestView() {
  const guard = usePrMergeGuard(() => [PR]);
  return <p>{guard.mergeRefusal(PR) ?? 'Merges on GitHub'}</p>;
}

afterEach(() => {
  cleanup();
  factory.urls = [];
});

describe('the app’s providers', () => {
  it('give every pull request view under them the factory’s merge guard', () => {
    render(() => (
      <AppProviders>
        <PullRequestView />
      </AppProviders>
    ));

    expect(factory.urls).toContainEqual([PR]);
    expect(
      screen.getAllByText('Only its run’s owner can merge this')
    ).toHaveLength(2);
  });

  it('give an ownerless dialog the factory’s merge guard too', () => {
    render(() => <AppProviders>{null}</AppProviders>);

    expect(factory.urls).toContainEqual([DIALOG_PR]);
    expect(
      screen.getByText('Only its run’s owner can merge this')
    ).toBeTruthy();
  });
});
