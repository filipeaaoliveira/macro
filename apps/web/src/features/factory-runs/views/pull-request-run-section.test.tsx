/** @vitest-environment jsdom */
import { cleanup, fireEvent, render, screen } from '@solidjs/testing-library';
import type { JSX } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PullRequestRunMark } from '../components/pull-request-run-mark';
import {
  type PullRequestRun,
  pullRequestMerge,
} from '../core/pull-request-run';
import { PullRequestRunSection } from './pull-request-run-section';

vi.mock('@components/app/side-panel', () => ({
  SidePanel: {
    Section: (props: { title: JSX.Element; children: JSX.Element }) => (
      <section>
        <h2>{props.title}</h2>
        {props.children}
      </section>
    ),
  },
}));

const JULIA = 'macro|julia@example.com';
const TEO = 'macro|teo@example.com';

/** A Run of Julia's at its merge Gate, after a round that found a Blocker
 * and a round that passed its fix. */
const atGate: PullRequestRun = {
  id: 'run-1',
  state: 'waiting',
  mergeGateSeq: 9,
  run: {
    id: 'run-1',
    request: 'Add the coupon field',
    createdAt: '2026-11-18T10:00:00Z',
    ownerId: JULIA,
    pullRequestUrl: 'https://github.com/acme/shop/pull/7',
    steps: [
      { seq: 3, entry: 'review', visit: 1, startedAt: '2026-11-18T10:10:00Z' },
      { seq: 5, entry: 'review', visit: 2, startedAt: '2026-11-18T10:20:00Z' },
      {
        seq: 9,
        entry: 'merge-gate',
        visit: 1,
        startedAt: '2026-11-18T10:40:00Z',
      },
    ],
    reviewRounds: [
      {
        round: 1,
        visit: 1,
        seq: 3,
        reviewers: 1,
        fixDiff: false,
        startedAt: '2026-11-18T10:10:00Z',
        end: { outcome: 'changes', endedAt: '2026-11-18T10:12:00Z' },
        findings: [
          {
            seq: 3,
            place: 1,
            path: 'src/cart.ts',
            lines: { start: 12, end: 12 },
            problem: 'The coupon is applied twice.',
            why: 'The total goes negative.',
            blocksAnywhere: false,
            kind: 'blocker',
          },
        ],
      },
      {
        round: 2,
        visit: 2,
        seq: 5,
        reviewers: 1,
        fixDiff: true,
        diffBase: 'abcdef0123',
        startedAt: '2026-11-18T10:20:00Z',
        end: { outcome: 'pass', endedAt: '2026-11-18T10:22:00Z' },
        findings: [],
      },
    ],
  },
};

afterEach(() => {
  cleanup();
});

describe('PullRequestRunMark', () => {
  it('marks the pull request as the factory’s, with its Run’s place and round', () => {
    const openRun = vi.fn();
    render(() => (
      <PullRequestRunMark
        found={atGate}
        merge={pullRequestMerge({ kind: 'run', found: atGate }, JULIA)}
        ownerName="you"
        onOpenRun={openRun}
      />
    ));

    expect(screen.getByText('Factory')).toBeTruthy();
    expect(screen.getByText('Waiting on you to merge')).toBeTruthy();
    expect(screen.getByText('Review round 2')).toBeTruthy();
    expect(
      screen.getByText('Merging answers its run’s merge gate')
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Open run' }));
    expect(openRun).toHaveBeenCalledWith('run-1');
  });

  it('shows open Blockers, and tells a teammate why they can’t merge', () => {
    const blocked: PullRequestRun = {
      ...atGate,
      state: 'running',
      mergeGateSeq: undefined,
      run: {
        ...atGate.run,
        steps: atGate.run.steps.slice(0, 1),
        reviewRounds: atGate.run.reviewRounds?.slice(0, 1),
      },
    };
    render(() => (
      <PullRequestRunMark
        found={blocked}
        merge={pullRequestMerge({ kind: 'run', found: blocked }, TEO)}
        ownerName="Julia Lima"
      />
    ));

    expect(screen.getByText('Review')).toBeTruthy();
    expect(screen.getByText('1 blocker')).toBeTruthy();
    expect(
      screen.getByText(
        'Only Julia Lima can merge this, at its run’s merge gate'
      )
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Open run' })).toBeNull();
  });
});

describe('PullRequestRunSection', () => {
  it('shows the Run’s review rounds and findings, and opens its merge Gate for its owner', () => {
    const answerGate = vi.fn();
    render(() => (
      <PullRequestRunSection
        found={atGate}
        merge={pullRequestMerge({ kind: 'run', found: atGate }, JULIA)}
        ownerName="you"
        order={15}
        onAnswerGate={answerGate}
      />
    ));

    expect(screen.getByText('Factory run')).toBeTruthy();
    expect(screen.getByText('The coupon is applied twice.')).toBeTruthy();
    expect(screen.getByText('src/cart.ts, line 12')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Answer merge gate' }));
    expect(answerGate).toHaveBeenCalledWith('run-1');
  });

  it('offers a teammate no answer to the merge Gate', () => {
    render(() => (
      <PullRequestRunSection
        found={atGate}
        merge={pullRequestMerge({ kind: 'run', found: atGate }, TEO)}
        ownerName="Julia Lima"
        order={15}
        onAnswerGate={vi.fn()}
      />
    ));

    expect(
      screen.queryByRole('button', { name: 'Answer merge gate' })
    ).toBeNull();
    expect(screen.getByText('The coupon is applied twice.')).toBeTruthy();
  });
});
