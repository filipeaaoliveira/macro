/** @vitest-environment jsdom */

import { factoryClient } from '@service-factory/client';
import type { StandingRulesSettings as Settings } from '@service-factory/generated/schemas';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StandingRulesSettings } from './standing-rules-settings';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    getStandingRules: vi.fn(),
    setStandingRules: vi.fn(),
  },
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

const teamDefault: Settings = {
  definition: { plan: false, blockers: true, merge: false, ownerGates: false },
  isDefault: true,
  projects: [],
  teamId: '00000000-0000-7000-8000-000000000201',
  canEdit: true,
};

function mount(settings: Settings) {
  vi.mocked(factoryClient.getStandingRules).mockResolvedValue(ok(settings));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(() => (
    <QueryClientProvider client={client}>
      <StandingRulesSettings />
    </QueryClientProvider>
  ));
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('StandingRulesSettings', () => {
  it('turns the stand-in on for one kind of Gate, keeping the rules', async () => {
    vi.mocked(factoryClient.setStandingRules).mockResolvedValueOnce(
      ok({
        ...teamDefault,
        definition: {
          plan: false,
          blockers: true,
          merge: false,
          ownerGates: false,
          standIn: { plan: true, blockers: false },
        },
        isDefault: false,
      })
    );
    mount(teamDefault);

    const plan = await screen.findByRole('checkbox', {
      name: 'Answer plan questions for the run’s owner',
    });
    expect((plan as HTMLInputElement).checked).toBe(false);
    fireEvent.click(plan);

    await waitFor(() =>
      expect(factoryClient.setStandingRules).toHaveBeenCalledWith({
        plan: false,
        blockers: true,
        merge: false,
        ownerGates: false,
        standIn: { plan: true, blockers: false },
      })
    );
    await waitFor(() => expect((plan as HTMLInputElement).checked).toBe(true));
  });

  it('turns an admin’s rule on, keeping the others, and goes back to the default', async () => {
    vi.mocked(factoryClient.setStandingRules)
      .mockResolvedValueOnce(
        ok({
          ...teamDefault,
          definition: {
            plan: false,
            blockers: true,
            merge: true,
            ownerGates: false,
          },
          isDefault: false,
        })
      )
      .mockResolvedValueOnce(ok(teamDefault));
    mount(teamDefault);

    const merge = await screen.findByRole('checkbox', {
      name: 'Merge when CI is green',
    });
    expect((merge as HTMLInputElement).checked).toBe(false);
    fireEvent.click(merge);

    await waitFor(() =>
      expect(factoryClient.setStandingRules).toHaveBeenCalledWith({
        plan: false,
        blockers: true,
        merge: true,
        ownerGates: false,
      })
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Use default' }));
    await waitFor(() =>
      expect(factoryClient.setStandingRules).toHaveBeenLastCalledWith(null)
    );
  });

  it('turns the owner section on, keeping the rules', async () => {
    vi.mocked(factoryClient.setStandingRules).mockResolvedValueOnce(
      ok({
        ...teamDefault,
        definition: { ...teamDefault.definition, ownerGates: true },
        isDefault: false,
      })
    );
    mount(teamDefault);

    const owner = await screen.findByRole('checkbox', {
      name: 'Send undecided gates to the run’s owner',
    });
    expect((owner as HTMLInputElement).checked).toBe(false);
    fireEvent.click(owner);

    await waitFor(() =>
      expect(factoryClient.setStandingRules).toHaveBeenCalledWith({
        plan: false,
        blockers: true,
        merge: false,
        ownerGates: true,
      })
    );
  });

  it('turns the task thread on, keeping the rules and the owner section', async () => {
    const ownerOn: Settings = {
      ...teamDefault,
      definition: { ...teamDefault.definition, ownerGates: true },
      isDefault: false,
    };
    vi.mocked(factoryClient.setStandingRules).mockResolvedValueOnce(
      ok({
        ...ownerOn,
        definition: { ...ownerOn.definition, taskThread: true },
      })
    );
    mount(ownerOn);

    const thread = await screen.findByRole('checkbox', {
      name: 'Post run updates to the task',
    });
    expect((thread as HTMLInputElement).checked).toBe(false);
    fireEvent.click(thread);

    await waitFor(() =>
      expect(factoryClient.setStandingRules).toHaveBeenCalledWith({
        plan: false,
        blockers: true,
        merge: false,
        ownerGates: true,
        taskThread: true,
      })
    );
    await waitFor(() =>
      expect((thread as HTMLInputElement).checked).toBe(true)
    );
  });

  it('shows a member the team’s rules without letting them change them', async () => {
    mount({ ...teamDefault, canEdit: false });

    const blockers = await screen.findByRole('checkbox', {
      name: 'Fix review blockers',
    });
    expect((blockers as HTMLInputElement).checked).toBe(true);
    expect(blockers.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('Only team admins can change them.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Use default' })).toBeNull();
  });
});
