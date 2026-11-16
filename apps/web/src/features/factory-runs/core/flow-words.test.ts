import { describe, expect, it } from 'vitest';
import { flowWordLabel, flowWordText } from './flow-words';

describe('flow words', () => {
  it('reads a Flow word as a label, acronyms in capitals', () => {
    expect(flowWordLabel('ci')).toBe('CI');
    expect(flowWordLabel('ci-fix')).toBe('CI fix');
    expect(flowWordLabel('review')).toBe('Review');
    expect(flowWordLabel('pr_merge')).toBe('PR merge');
  });

  it('keeps a word lowercase inside a sentence, but not an acronym', () => {
    expect(flowWordText('ci-fix')).toBe('CI fix');
    expect(flowWordText('write')).toBe('write');
  });
});
