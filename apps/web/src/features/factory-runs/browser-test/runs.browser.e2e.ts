import { expect, type Page, test } from '@playwright/test';

const SCREENSHOT_DIR = process.env.FACTORY_RUNS_SCREENSHOT_DIR;

async function shoot(page: Page, name: string, project: string) {
  if (!SCREENSHOT_DIR) return;
  await page.screenshot({ path: `${SCREENSHOT_DIR}/${name}-${project}.png` });
}

test('answers a Gate, starts a Run and cancels one', async ({
  page,
}, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  // On touch the floating panel is a sheet that starts closed.
  const show = page.getByRole('button', { name: 'Show details' });
  if (await show.isVisible()) await show.click();

  const rows = page.locator('[data-agent-activity-item]');
  await expect(rows).toHaveCount(4);
  await expect(rows.first()).toContainText('Plan needs your answer');
  await expect(rows.nth(1)).toContainText('Running Review');
  await shoot(page, 'runs-section', testInfo.project.name);

  // The waiting Run shows the plan its Gate asks about, and takes an answer.
  await rows.first().click();
  const dialog = page.getByRole('dialog', {
    name: 'Add a due date column to the task list',
  });
  await expect(dialog.getByText('Your answer is needed')).toBeVisible();
  // The plan renders as markdown.
  await expect(dialog.getByText('Due date', { exact: true })).toBeVisible();
  // The answers and Cancel sit in the pinned footer, in reach unscrolled.
  await expect(
    dialog.getByRole('button', { name: 'Approve' })
  ).toBeInViewport();
  await expect(
    dialog.getByRole('button', { name: 'Cancel run' })
  ).toBeInViewport();
  // A phone sheet closes by its handle, with no X.
  await expect(dialog.getByRole('button', { name: 'Close run' })).toHaveCount(
    testInfo.project.name === 'mobile' ? 0 : 1
  );
  await shoot(page, 'run-gate', testInfo.project.name);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('Ship it behind the flag.');
  await dialog.getByRole('button', { name: 'Approve' }).click();
  await expect(dialog.getByText('Your answer is needed')).toBeHidden();
  await expect(dialog.getByText('Answered: Approve')).toBeVisible();
  await expect(dialog.getByText('Ship it behind the flag.')).toBeVisible();
  await expect(
    dialog.locator('[data-factory-run-step="write:1"]')
  ).toBeVisible();
  await shoot(page, 'run-answered', testInfo.project.name);

  // Cancel asks first, then ends the Run.
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  await expect(dialog.getByText('Cancel this run?')).toBeVisible();
  await shoot(page, 'run-cancel-confirm', testInfo.project.name);
  await dialog.getByRole('button', { name: 'Cancel run' }).first().click();
  await expect(dialog.getByText('Cancelled').first()).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Cancel run' })).toHaveCount(
    0
  );
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // A new Run starts from the section and opens. On touch, opening a Run
  // closed the panel's sheet.
  if (await show.isVisible()) await show.click();
  await page.getByRole('button', { name: 'New run' }).click();
  const start = page.getByRole('dialog', { name: 'New run' });
  await start.getByLabel('Request').fill('Show overdue tasks in red');
  await start.getByText('Code', { exact: true }).click();
  await shoot(page, 'start-run', testInfo.project.name);
  await start.getByRole('button', { name: 'Start run' }).click();
  const started = page.getByRole('dialog', {
    name: 'Show overdue tasks in red',
  });
  await expect(started).toBeVisible();
  await expect(start).toBeHidden();
  // Focus moves into the Run that opened, not back to New run.
  await expect
    .poll(() =>
      started.evaluate((element) => element.contains(document.activeElement))
    )
    .toBe(true);
  await shoot(page, 'run-started', testInfo.project.name);
  await page.keyboard.press('Escape');
  if (await show.isVisible()) await show.click();
  await expect(rows).toHaveCount(5);
  await expect(rows.first()).toContainText('Show overdue tasks in red');

  expect(errors).toEqual([]);
});
