import { expect } from '@playwright/test';
import { test } from './fixtures.ts';

test.use({ javaScriptEnabled: false });

test('discovery navigation and directory search work before JavaScript loads', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.app-shell')).toHaveAttribute('data-hydrated', 'false');
  const discover = page.getByRole('link', { name: 'Find your people', exact: true });
  await expect(discover).toHaveAttribute('href', '/creators');
  await discover.click();
  await expect(page).toHaveURL(/\/creators$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Find your kind of people.');

  await page.getByRole('searchbox', { name: 'Search creators', exact: true }).fill('Marco');
  await page.getByRole('button', { name: 'Search creators', exact: true }).click();
  await expect(page).toHaveURL(/q=Marco/);
  await expect(page.getByRole('searchbox', { name: 'Search creators', exact: true })).toHaveValue(
    'Marco',
  );
  await expect(page.locator('.creator-card')).toHaveCount(1);
  await expect(page.locator('.creator-card')).toContainText("Marco's Table");
});

test('primary navigation retains browser link behavior without JavaScript', async ({ page }) => {
  await page.goto('/');
  const navigation = page.getByRole('navigation', {
    name: (page.viewportSize()?.width ?? 1440) <= 760 ? 'Mobile navigation' : 'Main navigation',
    exact: true,
  });
  const discover = navigation.getByRole('link', { name: 'Discover', exact: true });
  await expect(discover).toHaveAttribute('href', '/creators');
  await discover.click();
  await expect(page).toHaveURL(/\/creators$/);
  await page.goBack();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(
    'Your daily dose of inspiration.',
  );
});
