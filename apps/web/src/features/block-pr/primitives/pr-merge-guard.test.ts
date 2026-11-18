import type {
  PrAction,
  PrActions,
} from '@block-pr/primitives/create-pr-actions';
import { describe, expect, it, vi } from 'vitest';
import { withMergeTaker } from './reviews-merge';

const icon = () => null;
const URL = 'https://github.com/acme/shop/pull/7';

function actions(merge: () => void, close: () => void): PrActions {
  const all = (): PrAction[] => [
    { key: 'merge', label: 'Merge', icon, run: merge },
    { key: 'close', label: 'Close', icon, run: close },
  ];
  return {
    all,
    primary: () => all()[0],
    secondary: () => all().slice(1),
    pending: () => false,
  };
}

describe('withMergeTaker', () => {
  it('lets GitHub merge a pull request no one takes', () => {
    const github = vi.fn();
    const taken = withMergeTaker(actions(github, vi.fn()), () => URL, {
      merge: () => false,
      mergeRefusal: () => undefined,
    });

    taken.primary()?.run();

    expect(github).toHaveBeenCalledOnce();
    expect(taken.primary()?.disabledReason).toBeUndefined();
  });

  it('merges only through the taker when it takes the merge', () => {
    const github = vi.fn();
    const take = vi.fn(() => true);
    const taken = withMergeTaker(actions(github, vi.fn()), () => URL, {
      merge: take,
      mergeRefusal: () => undefined,
    });

    taken.primary()?.run();

    expect(take).toHaveBeenCalledWith(URL);
    expect(github).not.toHaveBeenCalled();
  });

  it('disables Merge with the reason the taker refuses it, and keeps the rest', () => {
    const close = vi.fn();
    const taken = withMergeTaker(actions(vi.fn(), close), () => URL, {
      merge: () => true,
      mergeRefusal: () => 'Only Teo can merge this',
    });

    expect(taken.primary()?.key).toBe('merge');
    expect(taken.primary()?.disabledReason).toBe('Only Teo can merge this');
    expect(taken.secondary().map((action) => action.key)).toEqual(['close']);
    taken.secondary()[0]?.run();
    expect(close).toHaveBeenCalledOnce();
  });
});
