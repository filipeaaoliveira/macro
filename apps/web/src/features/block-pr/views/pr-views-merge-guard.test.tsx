/** @vitest-environment jsdom */
import { cleanup, render, screen } from '@solidjs/testing-library';
import type { JSX } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import PrBlock from '../component/Block';
import type { PrAction, PrActions } from '../primitives/create-pr-actions';
import {
  type PrMergeGuard,
  PrMergeGuardProvider,
} from '../primitives/pr-merge-guard-context';
import { StandalonePrDetail } from './PrDetail';

const PR = 'https://github.com/acme/shop/pull/7';
const REFUSAL = 'Only its run’s owner can merge this';

// The views' own pieces stand in as plain elements, so the test sees only
// what the merge guard adds and the actions that reach the header.
const stubs = vi.hoisted(() => {
  const passThrough = (props: { children?: unknown }) => props.children;
  const nothing = () => null;
  return {
    passThrough,
    nothing,
    // The actions each header or actions button was given, newest last.
    headerActions: [] as PrActions[],
    githubMerge: vi.fn(),
  };
});

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({ enabled: true, loading: false }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: vi.fn(), success: vi.fn() },
}));
vi.mock('@core/block', () => ({ useBlockId: () => 'pr-entity-7' }));
vi.mock('@core/mobile/isTouchDevice', () => ({ isTouchDevice: () => false }));

// The pull request is loaded; its data is what the guard keys on.
vi.mock('../data/queries', () => ({
  prForeignEntityQueryKey: (id: string) => ['github-pr', 'foreign-entity', id],
  usePrForeignEntityQuery: () => ({
    isPending: false,
    status: 'success',
    refetch: () => Promise.resolve(),
    data: {
      id: 'pr-entity-7',
      prRef: { owner: 'acme', repo: 'shop', number: 7 },
      pullRequest: {
        url: PR,
        status: 'open',
        draft: false,
        name: 'Add coupon',
        displayName: 'acme/shop#7',
        githubKey: 'acme/shop#7',
        number: 7,
        owner: 'acme',
        repo: 'shop',
      },
    },
  }),
}));
vi.mock('@queries/storage/github-pull-requests', () => ({
  useRefreshGithubPullRequest: stubs.nothing,
}));
vi.mock('@queries/storage/keys', () => ({
  githubPullRequestChangesKeys: { summary: () => ({ queryKey: [] }) },
}));

// GitHub's own actions: a Merge that would go to GitHub, which the guard
// must stand in front of.
vi.mock('../primitives/create-pr-actions', () => ({
  createPrActions: (): PrActions => {
    const merge: PrAction = {
      key: 'merge',
      label: 'Merge',
      icon: () => null,
      run: stubs.githubMerge,
    };
    return {
      all: () => [merge],
      primary: () => merge,
      secondary: () => [],
      pending: () => false,
    };
  },
}));
vi.mock('../primitives/pr-file-operations', () => ({
  usePrFileOperations: () => () => [],
}));
vi.mock('../component/MergePullRequestButton', () => ({
  confirmMergePullRequest: () => Promise.resolve(true),
}));
vi.mock('../component/PrSplitHeader', () => ({
  PrSplitHeader: (props: { actions: PrActions }) => {
    stubs.headerActions.push(props.actions);
    return null;
  },
}));
vi.mock('../component/PullRequestActionsButton', () => ({
  PullRequestActionsButton: (props: { actions: PrActions }) => {
    stubs.headerActions.push(props.actions);
    return null;
  },
}));

// Presentational and app-shell modules the views import.
vi.mock('../component/sidepanel/PrSidePanelSections', () => ({
  PrSidePanelSections: stubs.nothing,
}));
vi.mock('../component/PrChanges', () => ({
  PrChangesProvider: stubs.passThrough,
}));
vi.mock('../component/PrSkeletons', () => ({
  PrDescriptionSkeleton: stubs.nothing,
  PrMetadataSkeleton: stubs.nothing,
  PrTimelineSkeleton: stubs.nothing,
  PrTitleSkeleton: stubs.nothing,
}));
vi.mock('../component/PrStatus', () => ({
  PR_PILL_CLASS: '',
  PrStatusChip: stubs.nothing,
  PrStatusIcon: stubs.nothing,
}));
vi.mock('../component/PrTimeline', () => ({ PrTimeline: stubs.nothing }));
vi.mock('../data/prDiscussionSource', () => ({
  createPrDiscussionSource: () => ({}),
}));
vi.mock('../hooks/usePrShare', () => ({
  usePrShare: () => ({
    openShare: () => undefined,
    copyLink: () => undefined,
    ShareModal: stubs.nothing,
  }),
}));
vi.mock('@app/components/view-shell', () => ({
  useTouchTopBarsFloat: () => () => false,
  ViewShell: {
    TopBar: stubs.passThrough,
    TopBarActions: stubs.passThrough,
    TopBarTitle: stubs.passThrough,
  },
}));
vi.mock('@app/features/changes/changes', () => ({
  ChangesSplit: stubs.passThrough,
  ChangesToggle: stubs.nothing,
}));
vi.mock('@app/features/changes/context/changes-controller', () => ({
  useOptionalChanges: () => undefined,
}));
vi.mock('@app/features/chat/ChatWithAgentButton', () => ({
  AskMacroButton: stubs.nothing,
}));
vi.mock('@components/app/GlobalAppState', () => ({
  useGlobalNotificationSource: () => undefined,
}));
vi.mock('@components/app/HeaderActionButton', () => ({
  HeaderActionButton: stubs.nothing,
}));
vi.mock('@components/app/HeaderActionGroup', () => ({
  HeaderActionGroup: stubs.passThrough,
}));
vi.mock('@components/app/SplitHeaderScope', () => ({
  SplitHeaderScope: { Root: stubs.passThrough, TopBar: stubs.nothing },
}));
vi.mock('@components/app/side-panel', () => ({
  SidePanel: {
    Root: stubs.passThrough,
    Layout: stubs.passThrough,
    Toggle: stubs.nothing,
  },
}));
vi.mock('@components/app/split-layout/components/HeaderIsland', () => ({
  HeaderIsland: stubs.passThrough,
}));
vi.mock('@components/app/split-layout/components/SplitFileMenu', () => ({
  SplitFileMenu: stubs.nothing,
}));
vi.mock('@components/app/split-layout/components/SplitHeader', () => ({
  SplitHeaderRight: stubs.passThrough,
}));
vi.mock('@components/app/split-panel', () => ({
  SplitPanel: { CloseButton: stubs.nothing },
}));
vi.mock(
  '@core/component/LexicalMarkdown/component/core/StaticMarkdown',
  () => ({
    StaticMarkdown: stubs.nothing,
    StaticMarkdownContext: stubs.passThrough,
  })
);
vi.mock('@core/component/SharePermissions', () => ({
  Permissions: { CAN_VIEW: 'view' },
}));
vi.mock('@entity/components/GithubLabelPill', () => ({
  GithubLabelPills: stubs.nothing,
}));
vi.mock('@entity/views/PrAgentSessionsChip', () => ({
  PrAgentSessionsChip: stubs.nothing,
}));
vi.mock('@macro-inc/lexical-core', () => ({
  normalizeGithubMarkdown: (markdown: string) => markdown,
}));
vi.mock('@notifications', () => ({ NotificationReadMarker: stubs.nothing }));
vi.mock('@ui', () => ({
  Button: stubs.passThrough,
  ButtonGroup: { Button: stubs.nothing },
  CopyButton: stubs.nothing,
  cn: (...classes: unknown[]) => classes.filter(Boolean).join(' '),
  Layer: stubs.passThrough,
  Scroll: stubs.passThrough,
  Tooltip: stubs.passThrough,
}));

/**
 * The app's merge guard as a provider would give it: it records the pull
 * requests it was asked about, shows its section and gate, and refuses
 * every merge.
 */
function createGuardSource() {
  const asked: string[][] = [];
  const merge = vi.fn((_url: string) => true);
  const source = (urls: () => readonly string[]): PrMergeGuard => {
    asked.push([...urls()]);
    return {
      mark: () => null,
      section: (url): JSX.Element => <p>Run section for {url}</p>,
      merge,
      mergeRefusal: () => REFUSAL,
      mergeWarning: () => undefined,
      gate: (): JSX.Element => <p>Run merge gate</p>,
    };
  };
  return { source, asked, merge };
}

function headerMerge() {
  const actions = stubs.headerActions.at(-1);
  return actions?.all().find((action) => action.key === 'merge');
}

afterEach(() => {
  cleanup();
  stubs.headerActions = [];
  stubs.githubMerge.mockClear();
});

describe.each([
  ['the pull request block', () => <PrBlock />],
  [
    'the standalone pull request detail',
    () => <StandalonePrDetail foreignEntityId="pr-entity-7" />,
  ],
])('%s', (_name, View) => {
  it('asks the provided merge guard about its pull request', () => {
    const guard = createGuardSource();
    render(() => (
      <PrMergeGuardProvider value={guard.source}>
        <View />
      </PrMergeGuardProvider>
    ));

    expect(guard.asked).toContainEqual([PR]);
  });

  it('shows the guard’s section and gate', () => {
    const guard = createGuardSource();
    render(() => (
      <PrMergeGuardProvider value={guard.source}>
        <View />
      </PrMergeGuardProvider>
    ));

    expect(screen.getByText(`Run section for ${PR}`)).toBeTruthy();
    expect(screen.getByText('Run merge gate')).toBeTruthy();
  });

  it('gives its header a Merge the guard refuses and takes', () => {
    const guard = createGuardSource();
    render(() => (
      <PrMergeGuardProvider value={guard.source}>
        <View />
      </PrMergeGuardProvider>
    ));

    const merge = headerMerge();
    expect(merge?.disabledReason).toBe(REFUSAL);

    merge?.run();
    expect(guard.merge).toHaveBeenCalledWith(PR);
    expect(stubs.githubMerge).not.toHaveBeenCalled();
  });
});
