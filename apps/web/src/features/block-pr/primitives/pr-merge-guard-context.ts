import { type Accessor, createContext, type JSX, useContext } from 'solid-js';

/**
 * Something that owns some pull requests' merges, such as the factory for
 * the pull requests its Runs opened: what a pull request's views show of
 * it, and the merges it takes in place of GitHub's.
 */
export type PrMergeGuard = {
  /** What a pull request's card shows of it, if anything. */
  mark: (url: string) => JSX.Element;
  /** A section of the pull request's side panel, if any. */
  section: (url: string) => JSX.Element;
  /**
   * Takes the merge of the pull request at `url`, returning whether it did:
   * it then merges, or says why not, its own way. False lets GitHub merge.
   */
  merge: (url: string) => boolean;
  /** Why the pull request at `url` doesn't merge now, if it doesn't. */
  mergeRefusal: (url: string) => string | undefined;
  /** What a person should know before GitHub merges it, if anything. */
  mergeWarning: (url: string) => string | undefined;
  /** What its merges open, such as a dialog. */
  gate: () => JSX.Element;
};

/** Makes the guard for the pull requests at `urls`, in the caller's scope. */
export type PrMergeGuardSource = (
  urls: Accessor<readonly string[]>
) => PrMergeGuard;

const Context = createContext<PrMergeGuardSource>();

/** Provides the app's merge guard to every pull request view under it. */
export const PrMergeGuardProvider = Context.Provider;

/**
 * The merge guard for the pull requests at `urls`; none outside a provider,
 * where GitHub merges every pull request as before.
 */
export function usePrMergeGuard(
  urls: Accessor<readonly string[]>
): PrMergeGuard | undefined {
  return useContext(Context)?.(urls);
}
