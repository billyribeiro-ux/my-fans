import { expect, type Page } from '@playwright/test';
import { test } from './fixtures.ts';
import { parsePublicSite, registrationConsent } from '../src/lib/public-site.ts';

const password = 'SvelteDraftFixture!2026';
const pageErrors = new WeakMap<Page, string[]>();

test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  pageErrors.set(page, errors);
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'warning' && message.text().includes('[svelte]'))
      errors.push(message.text());
  });
});
test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page)).toEqual([]);
});

async function register(page: Page) {
  await page.goto('/auth');
  await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
  const origin = new URL(page.url()).origin;
  const site = parsePublicSite(await (await page.request.get('/api/platform/site')).json());
  const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 20);
  const response = await page.request.post('/api/auth/register', {
    headers: { origin },
    data: {
      email: `svelte-${suffix}@myfans.test`,
      display_name: 'Saved account name',
      password,
      ...registrationConsent(site, 'US', true),
    },
  });
  expect(response.ok()).toBe(true);
  return { origin, suffix };
}

async function deactivate(page: Page, origin: string) {
  const response = await page.request.post('/api/platform/account/deactivate', {
    headers: { origin },
    data: { current_password: password },
  });
  expect(response.ok()).toBe(true);
}

async function createProfile(page: Page, origin: string, suffix: string) {
  const profile = {
    handle: `sv_${suffix}`,
    display_name: 'Saved creator name',
    bio: 'Saved biography from the real server.',
    category: 'Music',
    subscription_price_minor: 1250,
  };
  const response = await page.request.post('/api/platform/creators/profile', {
    headers: { origin },
    data: profile,
  });
  expect(response.ok()).toBe(true);
  return profile;
}

test('Settings renders the saved profile in SSR before JavaScript runs', async ({
  page,
  browser,
}) => {
  const { origin } = await register(page);
  const ssr = await browser.newContext({
    baseURL: origin,
    javaScriptEnabled: false,
    storageState: await page.context().storageState(),
  });
  try {
    const document = await ssr.newPage();
    await document.goto('/settings');
    await expect(document.getByLabel('Display name', { exact: true })).toHaveValue(
      'Saved account name',
    );
  } finally {
    await ssr.close();
    await deactivate(page, origin);
  }
});

test('Studio renders every saved profile field in SSR before JavaScript runs', async ({
  page,
  browser,
}) => {
  const { origin, suffix } = await register(page);
  const profile = await createProfile(page, origin, suffix);
  const ssr = await browser.newContext({
    baseURL: origin,
    javaScriptEnabled: false,
    storageState: await page.context().storageState(),
  });
  try {
    const document = await ssr.newPage();
    await document.goto('/studio');
    await expect(document.getByLabel('Creator name', { exact: true })).toHaveValue(
      profile.display_name,
    );
    await expect(document.getByLabel('Handle', { exact: true })).toHaveValue(profile.handle);
    await expect(document.getByRole('textbox', { name: 'About you', exact: true })).toHaveValue(
      profile.bio,
    );
    await expect(document.getByRole('combobox', { name: 'Category', exact: true })).toHaveValue(
      profile.category,
    );
    await expect(
      document.getByRole('spinbutton', { name: /^Monthly subscription in USD/ }),
    ).toHaveValue('12.5');
  } finally {
    await ssr.close();
    await deactivate(page, origin);
  }
});

test('Settings follows fresh server values but preserves dirty input and edits made during a save', async ({
  page,
}) => {
  const { origin } = await register(page);
  let release = () => {};
  try {
    await page.goto('/settings');
    await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
    const name = page.getByLabel('Display name', { exact: true });
    const update = await page.request.patch('/api/platform/account', {
      headers: { origin },
      data: { display_name: 'Updated on the server' },
    });
    expect(update.ok()).toBe(true);
    await page.getByRole('button', { name: 'Refresh provider status', exact: true }).click();
    await expect(name).toHaveValue('Updated on the server');

    await name.fill('Unsaved local name');
    await page.getByRole('button', { name: 'Refresh provider status', exact: true }).click();
    await expect(
      page.getByRole('button', { name: 'Refresh provider status', exact: true }),
    ).toBeEnabled();
    await expect(name).toHaveValue('Unsaved local name');

    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let committed!: () => void;
    const saved = new Promise<void>((resolve) => {
      committed = resolve;
    });
    await page.route('**/api/platform/account', async (route) => {
      if (route.request().method() !== 'PATCH') return route.continue();
      const response = await route.fetch();
      committed();
      await gate;
      await route.fulfill({ response });
    });
    await page.getByRole('button', { name: 'Save profile', exact: true }).click();
    await saved;
    await name.fill('Newer input during save');
    release();
    await expect(
      page.getByRole('status').filter({ hasText: 'Your profile has been updated.' }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save profile', exact: true })).toBeEnabled();
    await expect(name).toHaveValue('Newer input during save');
    const account = await (await page.request.get('/api/platform/account')).json();
    expect(account.user.display_name).toBe('Unsaved local name');
  } finally {
    release();
    await page.unrouteAll({ behavior: 'wait' });
    await deactivate(page, origin);
  }
});

test('Studio refreshes untouched profile fields while keeping dirty fields and later edits', async ({
  page,
}) => {
  const { origin, suffix } = await register(page);
  const profile = await createProfile(page, origin, suffix);
  let release = () => {};
  try {
    await page.goto('/studio');
    await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
    const name = page.getByLabel('Creator name', { exact: true });
    const bio = page.getByRole('textbox', { name: 'About you', exact: true });
    const price = page.getByRole('spinbutton', { name: /^Monthly subscription in USD/ });
    await name.fill('Unsaved creator name');
    const update = await page.request.post('/api/platform/creators/profile', {
      headers: { origin },
      data: { ...profile, bio: 'A newer server biography.', subscription_price_minor: 1750 },
    });
    expect(update.ok()).toBe(true);
    await page.getByRole('button', { name: 'Refresh processing status', exact: true }).click();
    await expect(bio).toHaveValue('A newer server biography.');
    await expect(price).toHaveValue('17.5');
    await expect(name).toHaveValue('Unsaved creator name');
    await price.fill('');
    await expect(price).toHaveValue('');
    expect(await price.evaluate((input: HTMLInputElement) => input.validity.valueMissing)).toBe(
      true,
    );
    await price.fill('18.5');

    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let committed!: () => void;
    const saved = new Promise<void>((resolve) => {
      committed = resolve;
    });
    await page.route('**/api/platform/creators/profile', async (route) => {
      if (route.request().method() !== 'POST') return route.continue();
      const response = await route.fetch();
      committed();
      await gate;
      await route.fulfill({ response });
    });
    await page.getByRole('button', { name: 'Save creator profile', exact: true }).click();
    await saved;
    await name.fill('Creator edit during save');
    release();
    await expect(
      page.getByRole('status').filter({ hasText: 'Creator profile saved.' }),
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Save creator profile', exact: true }),
    ).toBeEnabled();
    await expect(name).toHaveValue('Creator edit during save');
    const studio = await (await page.request.get('/api/platform/studio')).json();
    expect(studio.creator.display_name).toBe('Unsaved creator name');
    expect(studio.creator.subscription_price_minor).toBe(1850);
  } finally {
    release();
    await page.unrouteAll({ behavior: 'wait' });
    await deactivate(page, origin);
  }
});

test('reused Settings and Studio pages clear private drafts when the session account changes', async ({
  page,
  browser,
}) => {
  const first = await register(page);
  await createProfile(page, first.origin, first.suffix);
  const original = await browser.newContext({
    baseURL: first.origin,
    storageState: await page.context().storageState(),
  });
  const originalPage = await original.newPage();
  const second = await browser.newContext({ baseURL: first.origin });
  const secondPage = await second.newPage();
  const next = await register(secondPage);
  await createProfile(secondPage, next.origin, next.suffix);
  try {
    expect(
      (
        await secondPage.request.patch('/api/platform/account', {
          headers: { origin: next.origin },
          data: { display_name: 'Second account' },
        })
      ).ok(),
    ).toBe(true);
    await page.goto('/settings');
    await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
    await page.getByLabel('Display name', { exact: true }).fill('First account private draft');
    await page
      .getByLabel('Current password', { exact: true })
      .fill('First account private password');
    await page.getByLabel(/^New password/).fill('First account proposed password');
    expect(
      (
        await page.request.post('/api/auth/login', {
          headers: { origin: first.origin },
          data: { email: `svelte-${next.suffix}@myfans.test`, password },
        })
      ).ok(),
    ).toBe(true);
    // Refresh changes load data without unmounting the Settings route.
    await page.getByRole('button', { name: 'Refresh provider status', exact: true }).click();
    await expect(page.getByLabel('Display name', { exact: true })).toHaveValue('Second account');
    await expect(page.getByLabel('Current password', { exact: true })).toHaveValue('');
    await expect(page.getByLabel(/^New password/)).toHaveValue('');

    await page.goto('/studio');
    await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
    await page.getByLabel('Creator name', { exact: true }).fill('Second creator private draft');
    await page.getByLabel('Caption', { exact: true }).fill('Second account private post draft');
    expect(
      (
        await page.request.post('/api/auth/login', {
          headers: { origin: first.origin },
          data: { email: `svelte-${first.suffix}@myfans.test`, password },
        })
      ).ok(),
    ).toBe(true);
    await page.getByRole('button', { name: 'Refresh processing status', exact: true }).click();
    await expect(page.getByLabel('Handle', { exact: true })).toHaveValue(`sv_${first.suffix}`);
    await expect(page.getByLabel('Creator name', { exact: true })).toHaveValue(
      'Saved creator name',
    );
    await expect(page.getByLabel('Caption', { exact: true })).toHaveValue('');
  } finally {
    await deactivate(originalPage, first.origin);
    await deactivate(secondPage, next.origin);
    await original.close();
    await second.close();
  }
});

test('leaving Settings cancels an outstanding export and prevents a late private download', async ({
  page,
}) => {
  const { origin } = await register(page);
  let release = () => {};
  const downloads: string[] = [];
  page.on('download', (download) => downloads.push(download.suggestedFilename()));
  try {
    await page.goto('/settings');
    await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let received!: () => void;
    const started = new Promise<void>((resolve) => {
      received = resolve;
    });
    let completed!: () => void;
    const finished = new Promise<void>((resolve) => {
      completed = resolve;
    });
    await page.route('**/api/platform/account/export', async (route) => {
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      received();
      await gate;
      try {
        await route.fulfill({ response });
      } finally {
        completed();
      }
    });
    await page.getByRole('button', { name: 'Export account data', exact: true }).click();
    await started;
    const aborted = page.waitForEvent('requestfailed', {
      predicate: (request) => request.url().endsWith('/api/platform/account/export'),
    });
    await page.getByRole('link', { name: 'Creator Studio', exact: true }).click();
    await expect(page).toHaveURL(/\/studio$/);
    expect((await aborted).failure()?.errorText).toContain('ERR_ABORTED');
    release();
    await finished;
    // Wait through the browser's response/continuation window, rather than just inspecting immediately.
    await expect(page.waitForEvent('download', { timeout: 500 })).rejects.toThrow(/Timeout/);
    expect(downloads).toEqual([]);
    await expect(page).toHaveURL(/\/studio$/);
  } finally {
    release();
    await page.unrouteAll({ behavior: 'wait' });
    await deactivate(page, origin);
  }
});
