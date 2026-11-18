/** @vitest-environment jsdom */
import type { TaskBoardMove } from '@app/features/tasks-view/core/task-board';
import { confirmMergePullRequest } from '@block-pr/component/MergePullRequestButton';
import type { GithubPullRequestEntity } from '@entity';
import { cleanup, render } from '@solidjs/testing-library';
import type { JSX } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ReviewsBoard } from './ReviewsBoard';

const board = vi.hoisted(() => ({
  onMove: undefined as ((move: TaskBoardMove) => Promise<boolean>) | undefined,
  merge: vi.fn(),
}));

// The board's columns and cards are the task board's; only the move it
// hands its host matters here.
vi.mock('@app/features/tasks-view/components/task-board', () => ({
  TaskBoard: {
    Root: (props: {
      onMove: (move: TaskBoardMove) => Promise<boolean>;
      children: JSX.Element;
    }) => {
      board.onMove = props.onMove;
      return null;
    },
  },
}));
vi.mock('@block-pr/component/MergePullRequestButton', () => ({
  confirmMergePullRequest: vi.fn(async () => true),
}));
vi.mock('@queries/storage/pr-merge', () => ({
  useMergeGithubPullRequestMutation: () => ({ mutateAsync: board.merge }),
}));
vi.mock('@queries/storage/pr-draft', () => ({
  useSetGithubPullRequestDraftMutation: () => ({ mutateAsync: vi.fn() }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: vi.fn(), success: vi.fn() },
}));

/** An open pull request whose every check passed: Ready to merge. */
const ready = {
  id: 'pr-7',
  updatedAt: '2026-11-18T10:00:00Z',
  metadata: {
    number: 7,
    name: 'Add the coupon field',
    owner: 'acme',
    repo: 'shop',
    url: 'https://github.com/acme/shop/pull/7',
    status: 'open',
    draft: false,
    additions: 1,
    deletions: 0,
    comments: [],
    labels: [],
    checks: [{ name: 'build', status: 'completed', conclusion: 'success' }],
  },
} as unknown as GithubPullRequestEntity;

function mount(takeMerge?: (review: GithubPullRequestEntity) => boolean) {
  render(() => (
    <ReviewsBoard
      reviews={[ready]}
      mergeability={() => 'mergeable'}
      links={{
        linksFor: () => undefined,
        companyName: () => undefined,
        onOpen: () => {},
      }}
      scope="all"
      hasMore={false}
      isLoadingMore={false}
      onLoadMore={() => {}}
      onOpen={() => {}}
      takeMerge={takeMerge}
    />
  ));
  const onMove = board.onMove;
  if (!onMove) throw new Error('no board');
  return onMove;
}

const toMerged: TaskBoardMove = {
  id: 'pr-7',
  fromLane: 'ready',
  toLane: 'merged',
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  board.onMove = undefined;
});

describe('ReviewsBoard', () => {
  it('gives a merge it takes to its taker, and never asks GitHub', async () => {
    const takeMerge = vi.fn(() => true);
    const move = mount(takeMerge);

    expect(await move(toMerged)).toBe(false);
    expect(takeMerge).toHaveBeenCalledWith(ready);
    expect(confirmMergePullRequest).not.toHaveBeenCalled();
    expect(board.merge).not.toHaveBeenCalled();
  });

  it('merges on GitHub a pull request no one takes, as before', async () => {
    const move = mount(() => false);

    expect(await move(toMerged)).toBe(true);
    expect(confirmMergePullRequest).toHaveBeenCalledOnce();
    expect(board.merge).toHaveBeenCalledWith({
      owner: 'acme',
      repo: 'shop',
      number: 7,
    });
  });
});
