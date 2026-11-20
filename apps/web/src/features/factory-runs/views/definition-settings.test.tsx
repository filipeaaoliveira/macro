/** @vitest-environment jsdom */

import { factoryClient } from '@service-factory/client';
import type { DefinitionSettings as Settings } from '@service-factory/generated/schemas';
import { cleanup, render, screen, within } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DefinitionSettings } from './definition-settings';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    getDefinition: vi.fn(),
    setDefinition: vi.fn(),
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
    commit: READ,
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
  it('shows, read-only, the definition as the newest run read it', async () => {
    mount(named);

    const read = await screen.findByRole('link', {
      name: 'The newest run read it at aaaaaaa.',
    });
    expect(read.getAttribute('href')).toBe(`${REPO}/tree/${READ}`);
    const contents = screen.getByRole('group', {
      name: 'What the definition holds',
    });
    const inside = within(contents);
    expect(
      inside.getByRole('link', { name: 'As it is at aaaaaaa' })
    ).toBeTruthy();
    expect(inside.getByText('The team’s factory.')).toBeTruthy();
    expect(inside.getByText('notes')).toBeTruthy();
    expect(inside.getByText('plan → reply')).toBeTruthy();
    expect(inside.getByText('writer')).toBeTruthy();
    expect(inside.getByText('primary, backup')).toBeTruthy();
    expect(
      inside.getByText('Approve plans, Merge when CI is green')
    ).toBeTruthy();
    expect(inside.getByText('clear')).toBeTruthy();
    expect(inside.getByText('25% of runs of notes')).toBeTruthy();
    // Read-only: a member names nothing and edits none of it.
    expect(contents.querySelector('input, textarea, button')).toBeNull();
  });

  it('lists the open pull requests on the definition, each linked', async () => {
    mount(named);

    const fix = await screen.findByRole('link', {
      name: '#4 Fix the notes flow',
    });
    expect(fix.getAttribute('href')).toBe(`${REPO}/pull/4`);
    expect(
      screen.getByRole('link', { name: '#7 Add a scorer' }).getAttribute('href')
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
});
