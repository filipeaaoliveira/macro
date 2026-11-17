/** @vitest-environment jsdom */

import { toast } from '@core/component/Toast/Toast';
import { factoryClient } from '@service-factory/client';
import type { ControlFilesSettings as Settings } from '@service-factory/generated/schemas';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ControlFilesSettings } from './control-files-settings';

vi.mock('@service-factory/client', () => ({
  factoryClient: {
    getControlFiles: vi.fn(),
    setControlFiles: vi.fn(),
  },
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

const teamDefault: Settings = {
  patterns: ['.github/workflows/**', '**/*.test.*'],
  isDefault: true,
  teamId: '00000000-0000-7000-8000-000000000201',
  canEdit: true,
};

function mount(settings: Settings) {
  vi.mocked(factoryClient.getControlFiles).mockResolvedValue(ok(settings));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(() => (
    <QueryClientProvider client={client}>
      <ControlFilesSettings />
    </QueryClientProvider>
  ));
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ControlFilesSettings', () => {
  it('saves an admin’s patterns, one a line, and goes back to the default', async () => {
    vi.mocked(factoryClient.setControlFiles)
      .mockResolvedValueOnce(
        ok({
          ...teamDefault,
          patterns: ['ci/**', 'Makefile'],
          isDefault: false,
        })
      )
      .mockResolvedValueOnce(ok(teamDefault));
    mount(teamDefault);

    const field = await screen.findByLabelText(
      'Control file patterns, one a line'
    );
    expect((field as HTMLTextAreaElement).value).toBe(
      '.github/workflows/**\n**/*.test.*'
    );
    fireEvent.input(field, { target: { value: 'ci/**\n\n  Makefile  \n' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(factoryClient.setControlFiles).toHaveBeenCalledWith([
        'ci/**',
        'Makefile',
      ])
    );
    const useDefault = screen.getByRole('button', { name: 'Use default' });
    await waitFor(() =>
      expect(useDefault.hasAttribute('disabled')).toBe(false)
    );
    fireEvent.click(useDefault);
    await waitFor(() =>
      expect(factoryClient.setControlFiles).toHaveBeenLastCalledWith(null)
    );
  });

  it('shows a member the team’s patterns without letting them change them', async () => {
    mount({ ...teamDefault, canEdit: false });

    const field = await screen.findByLabelText(
      'Control file patterns, one a line'
    );
    expect(field.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText(/Only team admins can change them/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });

  it('asks again before saving an empty list', async () => {
    vi.mocked(factoryClient.setControlFiles).mockResolvedValue(
      ok({ ...teamDefault, patterns: [], isDefault: false })
    );
    mount(teamDefault);

    const field = await screen.findByLabelText(
      'Control file patterns, one a line'
    );
    fireEvent.input(field, { target: { value: '  \n' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByRole('alert')).toBeTruthy();
    expect(factoryClient.setControlFiles).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Save empty list' }));
    await waitFor(() =>
      expect(factoryClient.setControlFiles).toHaveBeenCalledWith([])
    );
  });

  it('says why the service refused the patterns', async () => {
    vi.mocked(factoryClient.setControlFiles).mockResolvedValue(
      err([
        {
          code: 'BAD_REQUEST',
          message:
            '"./ci.yml" is not a glob pattern of a path from the repository\'s root',
        },
      ])
    );
    mount(teamDefault);

    const field = await screen.findByLabelText(
      'Control file patterns, one a line'
    );
    fireEvent.input(field, { target: { value: './ci.yml' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        'Couldn’t save the control files',
        {
          subtext:
            '"./ci.yml" is not a glob pattern of a path from the repository\'s root',
        }
      )
    );
  });
});
