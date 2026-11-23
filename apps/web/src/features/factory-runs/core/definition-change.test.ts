import { describe, expect, it } from 'vitest';
import {
  changeRequest,
  checkDraft,
  checkRequest,
  type DefinitionChangeDraft,
  definitionFileUrl,
  isDefinitionFile,
  sendUnder,
  sentUnchanged,
  startedBefore,
  startsAfresh,
} from './definition-change';

const draft = (
  changes: Partial<DefinitionChangeDraft> = {}
): DefinitionChangeDraft => ({
  title: 'Add a docs flow',
  description: '',
  files: [{ path: 'flows/docs.json', text: '{"entries": []}' }],
  ...changes,
});

describe('isDefinitionFile', () => {
  it('takes the definition’s own files and a flow’s, nothing else', () => {
    for (const path of [
      'factory.json',
      'agents.json',
      'rules.json',
      'scorers.json',
      'flows/code.json',
      'flows/fix-ci_2.json',
    ]) {
      expect(isDefinitionFile(path)).toBe(true);
    }
    for (const path of [
      'README.md',
      'flows/code.yaml',
      'flows/nested/code.json',
      'flows/.json',
      'flows/my flow.json',
      'flows/Code.json',
      'flows/notes.v2.json',
      `flows/${'a'.repeat(65)}.json`,
      '/agents.json',
      'src/agents.json',
    ]) {
      expect(isDefinitionFile(path)).toBe(false);
    }
  });
});

describe('checkDraft', () => {
  it('readies a titled change of JSON files the definition reads', () => {
    expect(checkDraft(draft())).toEqual({
      title: undefined,
      description: undefined,
      files: [undefined],
      ready: true,
    });
  });

  it('stops a blank or too long title, and a too long description', () => {
    expect(checkDraft(draft({ title: '   ' })).title).toBe('blank');
    expect(checkDraft(draft({ title: 'é'.repeat(129) })).title).toBe('tooLong');
    expect(checkDraft(draft({ title: 'é'.repeat(128) })).ready).toBe(true);
    const long = checkDraft(draft({ description: 'a'.repeat(16 * 1024 + 1) }));
    expect(long.description).toBe('tooLong');
    expect(long.ready).toBe(false);
  });

  it('names each file’s problem: its path, a second write of a path, or text that isn’t JSON', () => {
    const check = checkDraft(
      draft({
        files: [
          { path: ' agents.json ', text: '{}' },
          { path: 'agents.json', text: '{}' },
          { path: 'notes.txt', text: '{}' },
          { path: 'rules.json', text: '{"plan": tru' },
        ],
      })
    );
    expect(check.files).toEqual([undefined, 'duplicate', 'path', 'json']);
    expect(check.ready).toBe(false);
  });

  it('sends nothing that writes no file', () => {
    expect(checkDraft(draft({ files: [] })).ready).toBe(false);
  });
});

describe('changeRequest', () => {
  it('trims the title and paths, keeps each file’s text, and sends no blank description', () => {
    expect(
      changeRequest(
        draft({
          title: '  Add a docs flow ',
          files: [
            { path: ' factory.json', text: '{"flows": ["docs"]}\n' },
            { path: 'flows/docs.json', text: '{}' },
          ],
        }),
        'change-1'
      )
    ).toEqual({
      changeId: 'change-1',
      title: 'Add a docs flow',
      description: null,
      files: {
        'factory.json': '{"flows": ["docs"]}\n',
        'flows/docs.json': '{}',
      },
    });
    expect(
      changeRequest(draft({ description: ' For the docs team. ' }), 'change-2')
        .description
    ).toBe('For the docs team.');
  });
});

describe('definitionFileUrl', () => {
  it('links a file at the commit the change goes on, else at the head', () => {
    const repo = 'https://github.com/macro/factory-definition';
    expect(definitionFileUrl(repo, 'agents.json', 'abc123')).toBe(
      `${repo}/blob/abc123/agents.json`
    );
    expect(definitionFileUrl(repo, 'flows/code.json', null)).toBe(
      `${repo}/blob/HEAD/flows/code.json`
    );
  });
});

describe('sendUnder', () => {
  const ids = () => {
    let next = 0;
    return () => `change-${++next}`;
  };

  it('sends a first change under a new id', () => {
    const sent = sendUnder(undefined, draft(), ids());
    expect(sent.request.changeId).toBe('change-1');
    expect(sent.sent.id).toBe('change-1');
  });

  it('keeps the id for the same change sent again, and takes a new one once it is edited', () => {
    const newId = ids();
    const first = sendUnder(undefined, draft(), newId);
    const again = sendUnder(first.sent, draft(), newId);
    expect(again.request.changeId).toBe('change-1');

    const retitled = sendUnder(
      again.sent,
      draft({ title: 'Add a docs flow, fixed' }),
      newId
    );
    expect(retitled.request.changeId).toBe('change-2');
    const retexted = sendUnder(
      retitled.sent,
      draft({
        title: 'Add a docs flow, fixed',
        files: [{ path: 'flows/docs.json', text: '{"entries": ["plan"]}' }],
      }),
      newId
    );
    expect(retexted.request.changeId).toBe('change-3');
  });
});

describe('checkRequest', () => {
  it('asks what a send would, without a change id', () => {
    expect(checkRequest(draft({ title: ' Add a docs flow ' }))).toEqual({
      title: 'Add a docs flow',
      description: null,
      files: { 'flows/docs.json': '{"entries": []}' },
    });
  });
});

describe('startsAfresh', () => {
  it('fills a file nothing was written in, or only what it started from', () => {
    expect(startsAfresh({ path: 'agents.json', text: '' })).toBe(true);
    expect(
      startsAfresh({
        path: 'rules.json',
        text: '{"a": 1}',
        start: { path: 'agents.json', commit: 'c1', text: '{"a": 1}' },
      })
    ).toBe(true);
    expect(
      startsAfresh({
        path: 'flows/new.json',
        text: '',
        start: { path: 'flows/new.json', commit: 'c1', text: null },
      })
    ).toBe(true);
    expect(startsAfresh({ path: 'agents.json', text: '{}' })).toBe(false);
    expect(
      startsAfresh({
        path: 'agents.json',
        text: '{"a": 2}',
        start: { path: 'agents.json', commit: 'c1', text: '{"a": 1}' },
      })
    ).toBe(false);
  });
});

describe('startedBefore', () => {
  it('names the files read at another commit than the head, under the path they were read at', () => {
    const files = [
      {
        path: 'agents.json',
        text: '{}',
        start: { path: 'agents.json', commit: 'old', text: '{}' },
      },
      {
        path: 'rules.json',
        text: '{}',
        start: { path: 'rules.json', commit: 'head', text: '{}' },
      },
      {
        path: 'scorers.json',
        text: '{}',
        start: { path: 'agents.json', commit: 'old', text: '{}' },
      },
      { path: 'factory.json', text: '{}' },
    ];
    expect(startedBefore(files, 'head')).toEqual(['agents.json']);
  });
});

describe('sentUnchanged', () => {
  it('is the change last sent, unedited since', () => {
    const first = sendUnder(undefined, draft(), () => 'change-1');
    expect(sentUnchanged(first.sent, draft())).toBe(true);
    expect(sentUnchanged(first.sent, draft({ title: 'Other' }))).toBe(false);
    expect(sentUnchanged(undefined, draft())).toBe(false);
  });
});
