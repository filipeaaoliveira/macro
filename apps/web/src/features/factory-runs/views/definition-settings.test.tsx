/** @vitest-environment jsdom */

import { factoryClient } from '@service-factory/client';
import type { DefinitionSettings as Settings } from '@service-factory/generated/schemas';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DefinitionSettings } from './definition-settings';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    getDefinition: vi.fn(),
    setDefinition: vi.fn(),
    proposeDefinitionChange: vi.fn(),
    checkDefinitionChange: vi.fn(),
    getDefinitionFile: vi.fn(),
  },
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

const REPO = 'https://github.com/macro/factory-definition';
const HEAD = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const READ = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

const named: Settings = {
  repoUrl: REPO,
  commit: HEAD,
  problem: null,
  requiresPullRequest: true,
  teamId: '00000000-0000-7000-8000-000000000201',
  canEdit: false,
  newestRun: { commit: READ, createdAt: '2026-11-20T10:00:00Z' },
  contents: {
    commit: HEAD,
    description: 'The team’s factory.',
    flows: [
      {
        name: 'notes',
        description: 'Write a note.',
        entries: ['plan', 'reply'],
      },
    ],
    roles: [{ role: 'writer', agents: ['primary', 'backup'] }],
    standingRules: {
      plan: true,
      blockers: false,
      merge: true,
      ownerGates: false,
    },
    scorers: [{ name: 'clear', flows: ['notes'], samplePercent: 25 }],
  },
  openPullRequests: [
    {
      number: 4,
      title: 'Fix the notes flow',
      url: `${REPO}/pull/4`,
      draft: false,
    },
    { number: 7, title: 'Add a scorer', url: `${REPO}/pull/7`, draft: true },
  ],
};

function mount(settings: Settings) {
  vi.mocked(factoryClient.getDefinition).mockResolvedValue(ok(settings));
  vi.mocked(factoryClient.getDefinitionFile).mockImplementation(async (path) =>
    ok({ path, commit: HEAD, text: null })
  );
  vi.mocked(factoryClient.checkDefinitionChange).mockResolvedValue(
    ok({ commit: HEAD })
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(() => (
    <QueryClientProvider client={client}>
      <DefinitionSettings />
    </QueryClientProvider>
  ));
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('DefinitionSettings', () => {
  it('shows, read-only, the definition at its head, and the newest run’s commit', async () => {
    mount(named);

    const read = await screen.findByRole('link', {
      name: 'The newest run read it at aaaaaaa. (opens in a new tab)',
    });
    expect(read.getAttribute('href')).toBe(`${REPO}/tree/${READ}`);
    const contents = screen.getByRole('group', {
      name: 'What the definition holds',
    });
    const inside = within(contents);
    expect(
      inside.getByRole('link', {
        name: 'As it is at bbbbbbb (opens in a new tab)',
      })
    ).toBeTruthy();
    expect(inside.getByText('The team’s factory.')).toBeTruthy();
    expect(inside.getByText('notes')).toBeTruthy();
    expect(inside.getByText('plan → reply')).toBeTruthy();
    expect(inside.getByText('Write a note.')).toBeTruthy();
    expect(inside.getByText('writer')).toBeTruthy();
    expect(inside.getByText('primary, backup')).toBeTruthy();
    expect(
      inside.getByText('Approve plans, Merge when CI is green')
    ).toBeTruthy();
    expect(inside.getByText('clear')).toBeTruthy();
    expect(inside.getByText('25% of runs of notes')).toBeTruthy();
    // Read-only: a member names nothing and edits none of it.
    expect(contents.querySelector('input, textarea, button')).toBeNull();
    expect(
      inside.queryByText(
        'Its default branch doesn’t read now, so this is the commit the newest run read.'
      )
    ).toBeNull();
  });

  it('says when it shows the newest run’s commit, the head not reading', async () => {
    mount({
      ...named,
      commit: null,
      problem: 'agents.json is missing',
      contents: { ...named.contents!, commit: READ },
    });

    const contents = await screen.findByRole('group', {
      name: 'What the definition holds',
    });
    expect(
      within(contents).getByRole('link', {
        name: 'As it is at aaaaaaa (opens in a new tab)',
      })
    ).toBeTruthy();
    expect(
      within(contents).getByText(
        'Its default branch doesn’t read now, so this is the commit the newest run read.'
      )
    ).toBeTruthy();
  });

  it('lists the open pull requests on the definition, each linked', async () => {
    mount(named);

    const fix = await screen.findByRole('link', {
      name: '#4 Fix the notes flow (opens in a new tab)',
    });
    expect(fix.getAttribute('href')).toBe(`${REPO}/pull/4`);
    expect(
      screen
        .getByRole('link', { name: '#7 Add a scorer (opens in a new tab)' })
        .getAttribute('href')
    ).toBe(`${REPO}/pull/7`);
    expect(screen.getByText('(draft)')).toBeTruthy();
  });

  it('says why the definition doesn’t read', async () => {
    mount({
      ...named,
      commit: null,
      problem:
        'the team’s factory definition does not read: agents.json is missing',
      newestRun: null,
      contents: null,
      openPullRequests: [],
    });

    const alert = await screen.findByText(
      'It doesn’t read: the team’s factory definition does not read: agents.json is missing'
    );
    expect(alert.getAttribute('role')).toBe('alert');
    expect(screen.getByText('No run has read it yet.')).toBeTruthy();
    expect(
      screen.queryByRole('group', { name: 'What the definition holds' })
    ).toBeNull();
    expect(screen.getByText('No open pull requests.')).toBeTruthy();
  });

  it('says when its pull requests couldn’t be listed', async () => {
    mount({ ...named, openPullRequests: null });

    expect(
      await screen.findByText(
        'Its pull requests couldn’t be listed with your GitHub account.'
      )
    ).toBeTruthy();
  });

  describe('a change to the definition', () => {
    const admin: Settings = { ...named, canEdit: true };

    async function write(changes: {
      title?: string;
      path?: string;
      text?: string;
    }) {
      fireEvent.click(
        await screen.findByRole('button', { name: 'Propose a change' })
      );
      fireEvent.input(
        screen.getByRole('textbox', { name: 'What it changes' }),
        {
          target: { value: changes.title ?? 'Add a review step' },
        }
      );
      fireEvent.input(screen.getByRole('textbox', { name: 'File 1' }), {
        target: { value: changes.path ?? 'flows/notes.json' },
      });
      fireEvent.input(
        screen.getByRole('textbox', { name: 'File 1’s new text' }),
        { target: { value: changes.text ?? '{"entries": ["plan"]}' } }
      );
    }

    const send = () =>
      fireEvent.click(
        screen.getByRole('button', { name: 'Open pull request' })
      );

    it('opens a valid change as a pull request and links it', async () => {
      vi.mocked(factoryClient.proposeDefinitionChange).mockResolvedValue(
        ok({
          number: 9,
          title: 'Add a review step',
          url: `${REPO}/pull/9`,
          draft: false,
        })
      );
      mount(admin);
      await write({});
      expect(
        screen
          .getByRole('link', {
            name: 'The file as it is now (opens in a new tab)',
          })
          .getAttribute('href')
      ).toBe(`${REPO}/blob/${HEAD}/flows/notes.json`);
      send();

      const link = await screen.findByRole('link', {
        name: '#9 Add a review step (opens in a new tab)',
      });
      expect(link.getAttribute('href')).toBe(`${REPO}/pull/9`);
      const request = vi.mocked(factoryClient.proposeDefinitionChange).mock
        .calls[0]?.[0];
      expect(request).toMatchObject({
        title: 'Add a review step',
        description: null,
        files: { 'flows/notes.json': '{"entries": ["plan"]}' },
      });
      expect(request?.changeId).toMatch(/^[0-9a-f-]{36}$/);
      // The settings are read again, to list the new pull request.
      await waitFor(() =>
        expect(factoryClient.getDefinition).toHaveBeenCalledTimes(2)
      );
      expect(
        screen.queryByRole('group', { name: 'Change the definition' })
      ).toBeNull();
    });

    it('sends nothing the browser can tell is wrong, and says what', async () => {
      mount(admin);
      await write({ path: 'README.md', text: '{"entries": [' });
      send();

      expect(
        await screen.findByText(
          'Name factory.json, agents.json, rules.json, scorers.json or a flow’s file, flows/name.json.'
        )
      ).toBeTruthy();
      fireEvent.input(screen.getByRole('textbox', { name: 'File 1' }), {
        target: { value: 'agents.json' },
      });
      send();
      expect(await screen.findByText('This isn’t valid JSON.')).toBeTruthy();
      expect(factoryClient.proposeDefinitionChange).not.toHaveBeenCalled();
    });

    it('shows why the definition doesn’t read with the change', async () => {
      vi.mocked(factoryClient.proposeDefinitionChange).mockResolvedValue(
        err([
          {
            code: 'INVALID_DEFINITION_CHANGE',
            message: 'flows/notes.json: the entry review is not defined',
          },
        ])
      );
      mount(admin);
      await write({});
      send();

      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toBe(
        'The definition doesn’t read with this change: flows/notes.json: the entry review is not defined'
      );
      // The change stays, to be fixed.
      expect(
        (screen.getByRole('textbox', { name: 'File 1' }) as HTMLInputElement)
          .value
      ).toBe('flows/notes.json');
    });

    it('sends a change whose answer was lost under its id again, and a closed one under a new id', async () => {
      vi.mocked(factoryClient.proposeDefinitionChange)
        .mockResolvedValueOnce(
          err([{ code: 'SERVER_ERROR', message: 'Internal server error' }])
        )
        .mockResolvedValueOnce(
          err([{ code: 'DEFINITION_CHANGE_CLOSED', message: 'closed' }])
        )
        .mockResolvedValueOnce(
          ok({
            number: 10,
            title: 'Again',
            url: `${REPO}/pull/10`,
            draft: false,
          })
        );
      mount(admin);
      await write({});
      send();
      expect(
        await screen.findByText(
          'The change couldn’t be sent. Send it again: if its pull request was opened, you’ll get it back.'
        )
      ).toBeTruthy();
      send();
      expect(
        await screen.findByText(
          'This change’s pull request was closed without merging. Send it again to open a new one.'
        )
      ).toBeTruthy();
      send();
      await screen.findByRole('link', {
        name: '#10 Again (opens in a new tab)',
      });

      const ids = vi
        .mocked(factoryClient.proposeDefinitionChange)
        .mock.calls.map(([request]) => request.changeId);
      expect(ids[1]).toBe(ids[0]);
      expect(ids[2]).not.toBe(ids[1]);
    });

    it('sends a change edited after a lost answer under a new id', async () => {
      vi.mocked(factoryClient.proposeDefinitionChange)
        .mockResolvedValueOnce(
          err([{ code: 'SERVER_ERROR', message: 'Internal server error' }])
        )
        .mockResolvedValueOnce(
          ok({
            number: 11,
            title: 'Fixed',
            url: `${REPO}/pull/11`,
            draft: false,
          })
        );
      mount(admin);
      await write({});
      send();
      await screen.findByText(
        'The change couldn’t be sent. Send it again: if its pull request was opened, you’ll get it back.'
      );
      fireEvent.input(
        screen.getByRole('textbox', { name: 'File 1’s new text' }),
        { target: { value: '{"entries": ["plan", "reply"]}' } }
      );
      send();
      await screen.findByRole('link', {
        name: '#11 Fixed (opens in a new tab)',
      });

      const [first, second] = vi
        .mocked(factoryClient.proposeDefinitionChange)
        .mock.calls.map(([request]) => request);
      expect(second?.changeId).not.toBe(first?.changeId);
      expect(second?.files).toEqual({
        'flows/notes.json': '{"entries": ["plan", "reply"]}',
      });
    });

    it('says why GitHub didn’t open the pull request, in the factory’s words, and why the repository couldn’t be read', async () => {
      vi.mocked(factoryClient.proposeDefinitionChange)
        .mockResolvedValueOnce(
          err([
            {
              code: 'DEFINITION_CHANGE_NOT_OPENED',
              message:
                'GitHub did not create the branch definition-change/1: a rule forbids it',
            },
          ])
        )
        .mockResolvedValueOnce(
          err([{ code: 'DEFINITION_UNREACHABLE', message: 'not found' }])
        );
      mount(admin);
      await write({});
      send();
      expect((await screen.findByRole('alert')).textContent).toBe(
        'GitHub did not create the branch definition-change/1: a rule forbids it. Sending it again tries a new branch.'
      );
      send();
      await waitFor(() =>
        expect(screen.getByRole('alert').textContent).toBe(
          'The definition’s repository couldn’t be read with your GitHub account: not found'
        )
      );
    });

    const fileText = () =>
      (
        screen.getByRole('textbox', {
          name: 'File 1’s new text',
        }) as HTMLTextAreaElement
      ).value;

    const pick = async (path: string) => {
      fireEvent.click(
        await screen.findByRole('button', { name: 'Propose a change' })
      );
      fireEvent.input(screen.getByRole('textbox', { name: 'File 1' }), {
        target: { value: path },
      });
    };

    it('starts a file from its text at the head once its path is picked', async () => {
      vi.mocked(factoryClient.getDefinitionFile).mockResolvedValueOnce(
        ok({
          path: 'flows/notes.json',
          commit: HEAD,
          text: '{"entries": ["reply"]}',
        })
      );
      mount(admin);
      await pick('flows/notes.json');

      await waitFor(() => expect(fileText()).toBe('{"entries": ["reply"]}'));
      expect(factoryClient.getDefinitionFile).toHaveBeenCalledWith(
        'flows/notes.json',
        expect.anything()
      );
      // Another path, its text untouched, starts from that file instead.
      vi.mocked(factoryClient.getDefinitionFile).mockResolvedValueOnce(
        ok({ path: 'agents.json', commit: HEAD, text: '{"roles": {}}' })
      );
      fireEvent.input(screen.getByRole('textbox', { name: 'File 1' }), {
        target: { value: 'agents.json' },
      });
      await waitFor(() => expect(fileText()).toBe('{"roles": {}}'));
    });

    it('says a picked file isn’t there yet, and keeps text already written', async () => {
      mount(admin);
      await pick('flows/new.json');

      expect(
        await screen.findByText(
          'There’s no file here yet: this change adds it.'
        )
      ).toBeTruthy();
      expect(fileText()).toBe('');
      fireEvent.input(
        screen.getByRole('textbox', { name: 'File 1’s new text' }),
        { target: { value: '{"entries": []}' } }
      );
      fireEvent.input(screen.getByRole('textbox', { name: 'File 1' }), {
        target: { value: 'agents.json' },
      });
      expect(fileText()).toBe('{"entries": []}');
      expect(factoryClient.getDefinitionFile).toHaveBeenCalledTimes(1);
    });

    it('says why a picked file couldn’t be read', async () => {
      vi.mocked(factoryClient.getDefinitionFile).mockResolvedValueOnce(
        err([{ code: 'DEFINITION_UNREACHABLE', message: 'not found' }])
      );
      mount(admin);
      await pick('agents.json');

      expect((await screen.findByRole('alert')).textContent).toBe(
        'Couldn’t read the file as it is now: not found'
      );
      expect(fileText()).toBe('');
    });

    it('checks a change without opening a pull request', async () => {
      vi.mocked(factoryClient.checkDefinitionChange)
        .mockResolvedValueOnce(ok({ commit: HEAD }))
        .mockResolvedValueOnce(
          err([
            {
              code: 'INVALID_DEFINITION_CHANGE',
              message: 'flows/notes.json: the entry review is not defined',
            },
          ])
        );
      mount(admin);
      await write({});
      fireEvent.click(screen.getByRole('button', { name: 'Check' }));

      expect(
        await screen.findByText(
          'The definition reads with this change at bbbbbbb. Nothing was sent.'
        )
      ).toBeTruthy();
      expect(factoryClient.checkDefinitionChange).toHaveBeenCalledWith({
        title: 'Add a review step',
        description: null,
        files: { 'flows/notes.json': '{"entries": ["plan"]}' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Check' }));
      expect((await screen.findByRole('alert')).textContent).toBe(
        'The definition doesn’t read with this change: flows/notes.json: the entry review is not defined'
      );
      expect(factoryClient.proposeDefinitionChange).not.toHaveBeenCalled();
    });

    it('checks a change before it sends it, and sends none that doesn’t read', async () => {
      vi.mocked(factoryClient.checkDefinitionChange).mockResolvedValueOnce(
        err([{ code: 'NO_PUSH_ACCESS', message: 'read only' }])
      );
      mount(admin);
      await write({});
      send();

      expect((await screen.findByRole('alert')).textContent).toBe(
        'Your GitHub account can’t push to the definition’s repository.'
      );
      expect(factoryClient.checkDefinitionChange).toHaveBeenCalledTimes(1);
      expect(factoryClient.proposeDefinitionChange).not.toHaveBeenCalled();
    });

    it('offers no change to a member who doesn’t administer the team', async () => {
      mount(named);

      expect(
        await screen.findByText('Only team admins can change it.')
      ).toBeTruthy();
      expect(
        screen.queryByRole('button', { name: 'Propose a change' })
      ).toBeNull();
    });
  });
});
