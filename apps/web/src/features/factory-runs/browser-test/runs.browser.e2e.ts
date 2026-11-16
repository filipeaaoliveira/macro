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
  await expect(rows.nth(1)).toContainText('Running review');
  await shoot(page, 'runs-section', testInfo.project.name);

  // The waiting Run shows the plan its Gate asks about, and takes an answer.
  await rows.first().click();
  const dialog = page.getByRole('dialog', {
    name: 'Add a due date column to the task list',
  });
  await expect(dialog.getByText('Your answer is needed')).toBeVisible();
  // The plan renders as markdown.
  await expect(dialog.getByText('Due date', { exact: true })).toBeVisible();
  // The Gate's question, its answers and Cancel are pinned, in view together.
  await expect(
    dialog.getByText(
      'Approve this plan, or send it back with a note on what to change?'
    )
  ).toBeInViewport();
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
  // The Gate's controls are gone; focus lands on where the Run is now.
  await expect(dialog.locator('[data-run-status]')).toBeFocused();
  await expect(dialog.getByText('Answered: Approve')).toBeVisible();
  await expect(dialog.getByText('Ship it behind the flag.')).toBeVisible();
  await expect(dialog.locator('[data-factory-run-step="3"]')).toBeVisible();
  await shoot(page, 'run-answered', testInfo.project.name);

  // Cancel asks first, then ends the Run.
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  await expect(dialog.getByText('Cancel this run?')).toBeVisible();
  await shoot(page, 'run-cancel-confirm', testInfo.project.name);
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  await expect(dialog.getByText('Cancelled').first()).toBeVisible();
  await expect(dialog.locator('[data-run-status]')).toBeFocused();
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

test('keeps a long Gate question and note from pushing the answers away', async ({
  page,
}, testInfo) => {
  await page.goto('/?long-question');
  const show = page.getByRole('button', { name: 'Show details' });
  if (await show.isVisible()) await show.click();
  await page.locator('[data-agent-activity-item]').first().click();
  const dialog = page.getByRole('dialog', {
    name: 'Add a due date column to the task list',
  });
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill(
      Array.from({ length: 12 }, (_, line) => `Note line ${line + 1}`).join(
        '\n'
      )
    );

  await expect(
    dialog.getByRole('button', { name: 'Approve' })
  ).toBeInViewport();
  await expect(
    dialog.getByRole('button', { name: 'Cancel run' })
  ).toBeInViewport();
  // The note scrolls past a few lines rather than growing.
  const noteBox = await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .boundingBox();
  expect(noteBox?.height ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(100);
  if (testInfo.project.name === 'mobile') {
    // The pinned answers name the question they answer, above the Steps.
    const describedBy = await dialog
      .getByRole('group', { name: 'Your answer is needed' })
      .getAttribute('aria-describedby');
    await expect(dialog.locator(`[id="${describedBy}"]`)).toContainText(
      '14. Check that the plan covers this case'
    );
  } else {
    // The capped question is a tab stop, so it can be scrolled from the
    // keyboard; Chrome would make a scroller one anyway, Safari would not.
    const region = dialog.getByRole('region', { name: 'Question' });
    await expect(region).toHaveAttribute('tabindex', '0');
  }
  const question = dialog.getByText('14. Check that the plan covers this case');
  await question.scrollIntoViewIfNeeded();
  await expect(question).toBeInViewport();
  const steps = dialog.getByText('Plan · visit 2');
  await steps.scrollIntoViewIfNeeded();
  await expect(steps).toBeInViewport();
  await expect(
    dialog.getByRole('button', { name: 'Approve' })
  ).toBeInViewport();
  await shoot(page, 'run-long-question', testInfo.project.name);
});

async function openWaitingRun(page: Page) {
  await page.goto('/');
  const show = page.getByRole('button', { name: 'Show details' });
  if (await show.isVisible()) await show.click();
  await page.locator('[data-agent-activity-item]').first().click();
  return page.getByRole('dialog', {
    name: 'Add a due date column to the task list',
  });
}

test('gives focus back to an answer that failed', async ({ page }) => {
  const dialog = await openWaitingRun(page);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('fail');
  const approve = dialog.getByRole('button', { name: 'Approve' });
  await approve.click();
  await expect(page.getByText('Couldn’t send your answer')).toBeVisible();
  await expect(approve).toBeFocused();
});

test('moves focus to the status line when someone else answered first', async ({
  page,
}) => {
  const dialog = await openWaitingRun(page);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('conflict');
  await dialog.getByRole('button', { name: 'Approve' }).click();
  await expect(
    page.getByText('This question was already answered')
  ).toBeVisible();
  // The read that shows their answer takes the Gate away.
  await expect(dialog.getByText('Your answer is needed')).toBeHidden();
  await expect(dialog.locator('[data-run-status]')).toBeFocused();
});

test('leaves focus on Cancel run when an answer in flight fails', async ({
  page,
}) => {
  const dialog = await openWaitingRun(page);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('slow-fail');
  await dialog.getByRole('button', { name: 'Approve' }).click();
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  const keep = dialog.getByRole('button', { name: 'Keep running' });
  await expect(keep).toBeFocused();

  await expect(page.getByText('Couldn’t send your answer')).toBeVisible();
  await expect(keep).toBeFocused();
});

test('moves focus after a cancel even when an answer in flight fails', async ({
  page,
}) => {
  const dialog = await openWaitingRun(page);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('slow-fail');
  await dialog.getByRole('button', { name: 'Approve' }).click();
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  await dialog.getByRole('button', { name: 'Cancel run' }).click();

  await expect(page.getByText('Couldn’t send your answer')).toBeVisible();
  await expect(dialog.getByText('Cancelled').first()).toBeVisible();
  await expect(dialog.locator('[data-run-status]')).toBeFocused();
});

test('locks the Gate once a cancel is confirmed', async ({ page }) => {
  const dialog = await openWaitingRun(page);
  await dialog
    .getByPlaceholder('Add a note for the next steps (optional)')
    .fill('slow-fail');
  await dialog.getByRole('button', { name: 'Cancel run' }).click();
  await dialog.getByRole('button', { name: 'Cancel run' }).click();

  const approve = dialog.getByRole('button', { name: 'Approve' });
  await expect(approve).toBeDisabled();
  // A press on the locked answer sends nothing.
  await approve.click({ force: true });

  await expect(dialog.getByText('Cancelled').first()).toBeVisible();
  await expect(dialog.locator('[data-run-status]')).toBeFocused();
});
