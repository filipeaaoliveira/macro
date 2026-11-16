import { expect, type Page, test } from '@playwright/test';

const SCREENSHOT_DIR = process.env.FACTORY_RUNS_SCREENSHOT_DIR;

/**
 * Saves the page with its scrolling pane laid out in full: the viewport grows
 * to the pane's content for the shot, then goes back.
 */
async function shoot(page: Page, name: string, project: string) {
  if (!SCREENSHOT_DIR) return;
  const viewport = page.viewportSize();
  const height = await page.evaluate(() =>
    Math.max(
      ...[...document.querySelectorAll<HTMLElement>('main *')]
        .filter((element) => getComputedStyle(element).overflowY === 'auto')
        .map((element) => element.scrollHeight + element.offsetTop)
    )
  );
  if (viewport && height > viewport.height) {
    await page.setViewportSize({ width: viewport.width, height });
  }
  await page.screenshot({ path: `${SCREENSHOT_DIR}/${name}-${project}.png` });
  if (viewport) await page.setViewportSize(viewport);
}

/** The page never scrolls sideways, at any width. */
async function expectNoSideScroll(page: Page) {
  const overflow = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
}

test('shows how runs went, filters by flow and opens a run', async ({
  page,
}, testInfo) => {
  const project = testInfo.project.name;
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/insights.html');

  // A placeholder holds the page until the first read lands.
  await expect(page.getByLabel('Loading run insights…')).toBeVisible();
  const outcomes = page.getByRole('list', { name: 'Run outcomes' });
  await expect(outcomes).toBeVisible();
  await expect(page.getByLabel('Loading run insights…')).toBeHidden();
  await expect(page.getByText('Oct 18 – Nov 16 · Europe/Lisbon')).toBeVisible();
  await expect(
    page.getByRole('img', { name: /Runs started each day/ })
  ).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Review' })).toBeVisible();
  await expectNoSideScroll(page);
  await shoot(page, 'insights', project);

  // The daily chart is explored by keyboard: focus picks today, the left
  // arrow the day before, and the picked day's runs are read out.
  const chart = page.getByRole('group', { name: /Runs by day chart/ });
  await chart.focus();
  await expect(chart.getByRole('status')).toContainText('Nov 16');
  await page.keyboard.press('ArrowLeft');
  await expect(chart.getByRole('status')).toContainText('Nov 15');

  // Only the code Flow: plan, then its own steps, and no reply.
  if (project === 'mobile') {
    // The shared filter drawer names its trigger itself.
    await page
      .getByRole('button', { name: /^(Open filters|Insights filters)$/ })
      .click();
    const drawer = page.getByRole('dialog', { name: 'Insights filters' });
    await drawer.getByRole('button', { name: 'Flow' }).click();
    await drawer.getByRole('radio', { name: 'Code' }).click();
    await shoot(page, 'insights-filters', project);
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();
  } else {
    await page.getByRole('button', { name: 'Insights filters' }).click();
    const flow = page.getByRole('button', { name: /All flows/ });
    await flow.click();
    await expect(page.getByRole('option', { name: 'Code' })).toBeVisible();
    await shoot(page, 'insights-filters', project);
    // The listbox takes typeahead: C picks Code.
    await page.keyboard.press('c');
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: /Code/ })).toBeVisible();
    await page.keyboard.press('Escape');
  }
  await expect(page.getByRole('heading', { name: 'Ci fix' })).toBeVisible();
  await expect(page.getByText('Reply', { exact: true })).toHaveCount(0);

  // A run where CI needed a fix opens as from its task.
  const stops = page.getByRole('list', { name: 'Runs at Ci', exact: true });
  await stops
    .getByRole('button', { name: /Add a due date column to the task list/ })
    .click();
  const run = page.getByRole('dialog', {
    name: 'Add a due date column to the task list',
  });
  await expect(run.getByText('Handed off').first()).toBeVisible();
  await shoot(page, 'insights-run', project);
  await page.keyboard.press('Escape');

  await expectNoSideScroll(page);
  expect(errors).toEqual([]);
});

test('says when insights could not be read, and reads them again', async ({
  page,
}, testInfo) => {
  await page.goto('/insights.html?error');

  const alert = page.getByRole('alert');
  await expect(alert).toContainText('Run insights couldn’t be loaded');
  await shoot(page, 'insights-error', testInfo.project.name);
  await alert.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByRole('list', { name: 'Run outcomes' })).toBeVisible();
  await expect(alert).toBeHidden();
});

test('says when a period has no runs', async ({ page }, testInfo) => {
  await page.goto('/insights.html?empty');

  await expect(
    page.getByText('No runs match this period and flow.', { exact: false })
  ).toBeVisible();
  await expect(
    page.getByText('No step failed or went again in this period.')
  ).toBeVisible();
  await expectNoSideScroll(page);
  await shoot(page, 'insights-empty', testInfo.project.name);
});
