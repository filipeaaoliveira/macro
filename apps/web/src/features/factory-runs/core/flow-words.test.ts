import { describe, expect, it } from 'vitest';
import { fellShort, flowWordLabel, flowWordText } from './flow-words';

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

  it('names the outcomes that fell short, a failed verify and deploy among them', () => {
    for (const outcome of [
      'failed',
      'red',
      'fail',
      'deploy_failed',
      'unresolved',
    ]) {
      expect(fellShort(outcome)).toBe(true);
    }
    for (const outcome of ['done', 'pass', 'deployed', 'not_needed', 'off']) {
      expect(fellShort(outcome)).toBe(false);
    }
  });

  it('names a step without a result short only once its owner stopped it', () => {
    expect(fellShort('no_result', 'stop')).toBe(true);
    expect(fellShort('no_result', 'retry')).toBe(false);
    expect(fellShort('no_result')).toBe(false);
  });
});
