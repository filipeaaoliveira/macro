/** @vitest-environment jsdom */
import { cleanup, render } from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PrForeignEntityData } from '../data/queries';
import type { MergePullRequestTarget } from './create-merge-pull-request-action';
import { createGuardedPrActions } from './pr-merge-guard';
import {
  type PrMergeGuard,
  PrMergeGuardProvider,
} from './pr-merge-guard-context';

// The confirmation GitHub's merge asks for, as the detail's actions get it.
const captured = vi.hoisted(() => ({
  confirm: undefined as
    | ((target: MergePullRequestTarget) => Promise<boolean>)
    | undefined,
}));

vi.mock('./create-pr-actions', () => ({
  createPrActions: (
    _detail: unknown,
    confirm: (target: MergePullRequestTarget, owner: null) => Promise<boolean>
  ) => {
    captured.confirm = (target) => confirm(target, null);
    return {
      all: () => [],
      primary: () => undefined,
      secondary: () => [],
      pending: () => false,
    };
  },
}));

const PR_URL = 'https://github.com/acme/shop/pull/7';
const detail = {
  id: 'pr-7',
  prRef: { owner: 'acme', repo: 'shop', number: 7 },
  pullRequest: { url: PR_URL },
} as unknown as PrForeignEntityData;

function mount(merge: PrMergeGuard['merge'], confirmed: boolean) {
  const guard: PrMergeGuard = {
    mark: () => null,
    section: () => null,
    merge,
    mergeRefusal: () => undefined,
    mergeWarning: () => undefined,
    gate: () => null,
  };
  const confirmMerge = vi.fn(async () => confirmed);
  render(() => (
    <PrMergeGuardProvider value={() => guard}>
      {(() => {
        createGuardedPrActions(() => detail, confirmMerge);
        return null;
      })()}
    </PrMergeGuardProvider>
  ));
  const confirm = captured.confirm;
  if (!confirm) throw new Error('not mounted');
  return { confirm, confirmMerge };
}

const target = { owner: 'acme', repo: 'shop', number: 7 };

afterEach(() => {
  cleanup();
  captured.confirm = undefined;
});

describe('a pull request detail’s guarded merge', () => {
  it('asks the guard again once the confirmation closes, and merges nothing it takes then', async () => {
    // Known as a Run's by the time the person confirmed: the guard takes
    // the merge, opening the Run's Gate or saying why not.
    const take = vi.fn(() => true);
    const { confirm } = mount(take, true);

    expect(await confirm(target)).toBe(false);
    expect(take).toHaveBeenCalledWith(PR_URL);
  });

  it('merges on GitHub when the guard still leaves it to GitHub', async () => {
    const take = vi.fn(() => false);
    const { confirm } = mount(take, true);

    expect(await confirm(target)).toBe(true);
    expect(take).toHaveBeenCalledWith(PR_URL);
  });

  it('asks the guard nothing when the person dismissed the confirmation', async () => {
    const take = vi.fn(() => false);
    const { confirm } = mount(take, false);

    expect(await confirm(target)).toBe(false);
    expect(take).not.toHaveBeenCalled();
  });
});
