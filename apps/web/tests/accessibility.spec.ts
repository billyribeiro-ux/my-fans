import AxeBuilder from '@axe-core/playwright';
import { expect, type Page } from '@playwright/test';
import { test } from './fixtures.ts';

async function audit(page: Page) {
  await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0);
  const result = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
    .analyze();
  return result.violations.map(({ id, impact, nodes }) => ({
    url: page.url(),
    id,
    impact,
    elements: nodes.map((n) => ({ target: n.target, details: n.failureSummary })),
  }));
}

test('critical guest and authenticated pages pass automated accessibility checks', async ({
  page,
  baseURL,
}) => {
  test.setTimeout(90000);
  if (test.info().project.name === 'desktop') {
    await page.setViewportSize({ width: 1440, height: 1000 });
  }
  const failures: Awaited<ReturnType<typeof audit>> = [];
  for (const route of [
    '/',
    '/creators',
    '/auth?mode=register',
    '/studio',
    '/messages',
    '/notifications',
    '/settings',
    '/earnings',
    '/admin',
    '/legal',
  ]) {
    await page.goto(route);
    failures.push(...(await audit(page)));
  }
  const unique = crypto.randomUUID();
  const response = await page.request.post('/api/auth/register', {
    headers: { origin: baseURL! },
    data: {
      email: `a11y-${unique}@myfans.test`,
      display_name: 'Accessibility Test',
      password: 'BrowserTestOnly!2026',
    },
  });
  expect(response.ok()).toBe(true);
  for (const route of ['/settings', '/studio', '/messages', '/notifications', '/earnings']) {
    await page.goto(route);
    failures.push(...(await audit(page)));
  }
  await page.goto('/');
  await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
  await page.getByRole('button', { name: "View Ella Wild's profile", exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  failures.push(...(await audit(page)));
  expect(failures, 'Automated WCAG violations by route and element').toEqual([]);
});
