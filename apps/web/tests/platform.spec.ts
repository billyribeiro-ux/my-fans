import { expect, type Page } from '@playwright/test';
import { test } from './fixtures.ts';
import { createHmac } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import AxeBuilder from '@axe-core/playwright';
import type { FeedResponse } from '../src/lib/contracts.ts';
import { parsePublicSite, registrationConsent } from '../src/lib/public-site.ts';

const pageErrors = new WeakMap<Page, string[]>();
const password = 'BrowserTestOnly!2026';

async function ready(page: Page) {
  await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
}
async function register(page: Page) {
  const unique = crypto.randomUUID().replaceAll('-', '');
  await page.goto('/auth?mode=register');
  await ready(page);
  await page.getByLabel('Your name', { exact: true }).fill('Browser Test');
  await page.getByLabel('Email address', { exact: true }).fill(`browser-${unique}@myfans.test`);
  await page.getByLabel('Password', { exact: true }).fill(password);
  await page.getByLabel('Country of residence', { exact: true }).selectOption('US');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await ready(page);
  return unique;
}
async function verify(page: Page) {
  await page.goto('/settings');
  await ready(page);
  await page.getByRole('button', { name: 'Open sandbox inbox', exact: true }).click();
  await page
    .getByRole('link', { name: 'Open verify your email link', exact: true })
    .first()
    .click();
  await ready(page);
  await page.getByRole('button', { name: 'Verify email', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Email verified');
  await page.goto('/');
  await ready(page);
}
async function feed(page: Page): Promise<FeedResponse> {
  const response = await page.request.get('/api/platform/feed');
  expect(response.ok()).toBe(true);
  expect(response.headers()['x-request-id']).toMatch(/^[a-f0-9-]{36}$/);
  return response.json();
}

function authenticatorCode(base32: string) {
  let bits = '';
  for (const character of base32.replace(/=+$/, ''))
    bits += 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(character).toString(2).padStart(5, '0');
  const secret = Buffer.from(bits.match(/.{8}/g)!.map((byte) => Number.parseInt(byte, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const hash = createHmac('sha1', secret).update(counter).digest();
  const offset = hash[hash.length - 1] & 15;
  return ((hash.readUInt32BE(offset) & 0x7fffffff) % 1000000).toString().padStart(6, '0');
}

test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  pageErrors.set(page, errors);
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await ready(page);
});
test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page)).toEqual([]);
});

test('guests can discover and search without receiving protected media', async ({ page }) => {
  const fonts = await page.evaluate(async () => {
    const loaded = await Promise.all([
      document.fonts.load('400 16px "DM Sans"'),
      document.fonts.load('400 16px "Manrope"'),
    ]);
    return {
      loaded: loaded.every(
        (faces) => faces.length > 0 && faces.every((face) => face.status === 'loaded'),
      ),
      resources: performance.getEntriesByType('resource').map((entry) => entry.name),
    };
  });
  expect(fonts.loaded).toBe(true);
  expect(fonts.resources).toContain(new URL('/fonts/dm-sans-variable.ttf', page.url()).href);
  expect(fonts.resources).toContain(new URL('/fonts/manrope-variable.ttf', page.url()).href);
  expect(
    fonts.resources.some((url) => /https:\/\/fonts\.(?:googleapis|gstatic)\.com\//.test(url)),
  ).toBe(false);
  const initial = await feed(page);
  expect(initial.viewer.role).toBe('guest');
  expect(initial.posts.length).toBeGreaterThan(0);
  for (const post of initial.posts.filter((item) => item.locked)) expect(post.media_url).toBeNull();
  await page.getByRole('searchbox', { name: 'Search this feed' }).fill('Marco');
  await page.getByRole('button', { name: 'Search posts', exact: true }).click();
  await expect(page).toHaveURL(/q=Marco/);
  await expect(page.locator('article.post')).toHaveCount(1);
  await expect(page.locator('article.post')).toContainText("Marco's Table");
  await page.getByRole('combobox', { name: 'Creator category', exact: true }).selectOption('Food');
  await page.getByRole('button', { name: 'Search posts', exact: true }).click();
  await expect(page).toHaveURL(/category=Food/);
  await page.reload();
  await ready(page);
  await expect(page.getByRole('searchbox', { name: 'Search this feed' })).toHaveValue('Marco');
  await expect(page.getByRole('combobox', { name: 'Creator category', exact: true })).toHaveValue(
    'Food',
  );
  const marco = initial.creators.find((creator) => creator.display_name === "Marco's Table")!;
  await page
    .getByRole('combobox', { name: 'Filter by creator', exact: true })
    .selectOption(marco.id);
  await page.getByRole('button', { name: 'Search posts', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`creator=${marco.id}`));
  await expect(page.locator('article.post')).toHaveCount(1);
  await page
    .locator('article.post')
    .getByRole('button', { name: 'Save post', exact: true })
    .click();
  await expect(page).toHaveURL(/\/auth$/);
});

test('creator directory searches server-side and preserves category filters across navigation', async ({
  page,
}) => {
  const response = await page.request.get('/api/platform/creators');
  expect(response.ok()).toBe(true);
  const initial = await response.json();
  expect(initial.creators.length).toBeGreaterThan(0);
  expect(initial.creators.length).toBeLessThanOrEqual(50);
  const creator = initial.creators[0];
  await page
    .getByRole('navigation', {
      name: (page.viewportSize()?.width ?? 1440) <= 760 ? 'Mobile navigation' : 'Main navigation',
      exact: true,
    })
    .getByRole('link', { name: 'Discover', exact: true })
    .click();
  await expect(page).toHaveURL(/\/creators$/);
  await ready(page);
  await page.getByRole('searchbox', { name: 'Search creators', exact: true }).fill(creator.handle);
  await page
    .getByRole('combobox', { name: 'Creator category', exact: true })
    .selectOption(creator.category);
  await page.getByRole('button', { name: 'Search creators', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`q=${creator.handle}`));
  await expect(page).toHaveURL(new RegExp(`category=${encodeURIComponent(creator.category)}`));
  await page.reload();
  await ready(page);
  await expect(page.getByRole('searchbox', { name: 'Search creators', exact: true })).toHaveValue(
    creator.handle,
  );
  await expect(page.getByRole('combobox', { name: 'Creator category', exact: true })).toHaveValue(
    creator.category,
  );
  await page.getByRole('link', { name: `Explore ${creator.display_name}`, exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`creator=${creator.id}`));
  await ready(page);
  await page.goto('/creators?q=no-match-' + crypto.randomUUID());
  await ready(page);
  await expect(
    page.getByRole('heading', { name: 'No creators match yet.', exact: true }),
  ).toBeVisible();
});

test('registration uses configured policies and records explicit eligible adult consent', async ({
  page,
}) => {
  const response = await page.request.get('/api/platform/site');
  expect(response.ok()).toBe(true);
  const site = parsePublicSite(await response.json());
  await page.goto('/legal');
  await ready(page);
  for (const policy of site.policies) {
    await expect(page.locator(`#${policy.kind}`)).toContainText(policy.version);
    await expect(page.locator(`#${policy.kind}`)).toContainText(policy.sections[0].body);
  }
  await page.goto('/auth?mode=register');
  await ready(page);
  await expect(page.getByRole('button', { name: 'Create account', exact: true })).toBeDisabled();
  const request = page.waitForRequest(
    (request) => request.url().endsWith('/api/auth/register') && request.method() === 'POST',
  );
  await register(page);
  expect((await request).postDataJSON()).toMatchObject(registrationConsent(site, 'US', true));
});

test('registration, local verification, profile changes, and sign out persist correctly', async ({
  page,
  browser,
}) => {
  const unique = await register(page);
  await verify(page);
  await page.goto('/settings');
  await ready(page);
  await expect(page.getByText('Email verified', { exact: true })).toBeVisible();
  const newName = `Fan ${unique.slice(0, 8)}`;
  await page.getByLabel('Display name', { exact: true }).fill(newName);
  await page.getByRole('button', { name: 'Save profile', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('profile has been updated');
  await page.reload();
  await ready(page);
  await expect(page.getByLabel('Display name', { exact: true })).toHaveValue(newName);
  const oldSession = (await page.context().cookies()).find(
    (cookie) => cookie.name === 'myfans_session',
  )!;
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  expect((await feed(page)).viewer.id).toBe('');
  const replay = await browser.newContext();
  try {
    await replay.addCookies([oldSession]);
    expect(
      (
        await replay.request.get(`${new URL(page.url()).origin}/api/platform/account/export`)
      ).status(),
    ).toBe(401);
  } finally {
    await replay.close();
  }
});

test('verification resend retries throttled requests and replaces old sandbox verification links', async ({
  page,
}) => {
  await register(page);
  const initial = await (await page.request.get('/api/platform/account/mail')).json();
  const oldLink = initial.deliveries.find(
    (item: { subject: string }) => item.subject === 'Verify your email',
  ).body;
  await page.goto('/settings');
  await ready(page);
  await expect(page.getByText('Email verification pending', { exact: true })).toBeVisible();
  let releaseFailure = () => {};
  const delayed = new Promise<void>((resolve) => {
    releaseFailure = resolve;
  });
  await page.route(
    '**/api/platform/account/verification/resend',
    async (route) => {
      await delayed;
      await route.fulfill({
        status: 429,
        json: { message: 'Too many requests. Wait a little, then try again.' },
      });
    },
    { times: 1 },
  );
  const sending = page.waitForRequest((request) =>
    request.url().endsWith('/api/platform/account/verification/resend'),
  );
  await page.getByRole('button', { name: 'Resend verification email', exact: true }).click();
  await sending;
  await expect(
    page.getByRole('button', { name: 'Sending verification email…', exact: true }),
  ).toBeDisabled();
  releaseFailure();
  await expect(page.getByRole('alert')).toContainText('Too many requests');
  await expect(
    page.getByRole('button', { name: 'Resend verification email', exact: true }),
  ).toBeEnabled();
  await page.getByRole('button', { name: 'Resend verification email', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('a message has been queued');
  const current = await (await page.request.get('/api/platform/account/mail')).json();
  const newLink = current.deliveries.find(
    (item: { subject: string; body: string }) =>
      item.subject === 'Verify your email' && item.body !== oldLink,
  ).body;
  await page.getByRole('button', { name: 'Open sandbox inbox', exact: true }).click();
  const newestLink = page
    .getByRole('link', { name: 'Open verify your email link', exact: true })
    .first();
  await expect(newestLink).toHaveAttribute('href', newLink);
  await page.goto(oldLink);
  await ready(page);
  await page.getByRole('button', { name: 'Verify email', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('invalid or expired');
  await page.goto(newLink);
  await ready(page);
  await page.getByRole('button', { name: 'Verify email', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Email verified');
  await page.goto('/settings');
  await ready(page);
  await expect(page.getByText('Email verified', { exact: true })).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Resend verification email', exact: true }),
  ).toHaveCount(0);
  expect((await (await page.request.get('/api/platform/account')).json()).user.email_verified).toBe(
    true,
  );
  expect(
    (await (await page.request.get('/api/platform/account/mail')).json()).deliveries,
  ).toHaveLength(current.deliveries.length);
});

test('SMTP inbox response removes local verification controls and directs users to email', async ({
  page,
}) => {
  await register(page);
  await page.goto('/settings');
  await ready(page);
  await page.route('**/api/platform/account/mail', (route) =>
    route.fulfill({
      json: {
        deliveries: [],
        next_cursor: null,
        mode: 'smtp',
        warning: 'Check your email inbox. Verification and reset links are never available here.',
      },
    }),
  );
  await page.getByRole('button', { name: 'Open sandbox inbox', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Email delivery', exact: true })).toBeVisible();
  await expect(
    page.getByText(
      'Check your email inbox. Verification and reset links are never available here.',
      { exact: true },
    ),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Refresh inbox', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Open sandbox inbox', exact: true })).toHaveCount(
    0,
  );
  await expect(page.getByRole('link', { name: /^Open .* link$/ })).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
});

test('account export excludes authentication secrets and deactivation revokes sessions', async ({
  page,
  browser,
  context,
}) => {
  const unique = await register(page);
  const email = `browser-${unique}@myfans.test`;
  const otherSession = await browser.newContext();
  try {
    const origin = new URL(page.url()).origin;
    const login = await otherSession.request.post(`${origin}/api/auth/login`, {
      headers: { Origin: origin },
      data: { email, password },
    });
    expect(login.ok()).toBe(true);
    await page.goto('/settings');
    await ready(page);
    const downloadEvent = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export account data', exact: true }).click();
    const download = await downloadEvent;
    expect(download.suggestedFilename()).toMatch(/^myfans-account-\d{4}-\d{2}-\d{2}\.json$/);
    const stream = await download.createReadStream();
    expect(stream).not.toBeNull();
    const chunks: Buffer[] = [];
    for await (const chunk of stream!) chunks.push(Buffer.from(chunk));
    const text = Buffer.concat(chunks).toString('utf8');
    const exported = JSON.parse(text);
    expect(exported.user.email).toBe(email);
    expect(exported.mode).toBe('sandbox');
    expect(exported.media_note).toContain('Creator Studio');
    for (const forbidden of ['password_hash', 'mfa_secret', 'token_digest', password])
      expect(text).not.toContain(forbidden);
    const session = (await context.cookies()).find((cookie) => cookie.name === 'myfans_session');
    expect(text).not.toContain(session!.value);
    await page.getByRole('button', { name: 'Deactivate account', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('does not erase account data');
    await dialog.getByLabel('Confirm current password', { exact: true }).fill(password);
    await dialog.getByRole('button', { name: 'Confirm deactivation', exact: true }).click();
    await expect(page).toHaveURL(/\/auth\?deactivated=1$/);
    await ready(page);
    await expect(page.getByRole('status')).toContainText('deactivation does not erase your data');
    const revokedAccount = await otherSession.request.get(`${origin}/api/platform/account`);
    expect((await revokedAccount.json()).user).toBeNull();
    expect((await otherSession.request.get(`${origin}/api/platform/account/export`)).status()).toBe(
      401,
    );
    await page.getByLabel('Email address', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Email or password is incorrect');
    expect((await feed(page)).viewer.id).toBe('');
  } finally {
    await otherSession.close();
  }
});

test('recovery codes support sign-in, password reset, replacement, and explicit saved acknowledgment', async ({
  page,
  browser,
}) => {
  test.setTimeout(120000);
  const unique = await register(page);
  const email = `browser-${unique}@myfans.test`;
  const origin = new URL(page.url()).origin;
  const otherSession = await browser.newContext();
  const replay = await browser.newContext();
  try {
    expect(
      (
        await otherSession.request.post(`${origin}/api/auth/login`, {
          headers: { Origin: origin },
          data: { email, password },
        })
      ).ok(),
    ).toBe(true);
    await page.goto('/settings');
    await ready(page);
    await page.getByRole('button', { name: 'Set up authenticator', exact: true }).click();
    await expect(page.locator('.mfa-secret')).toBeVisible();
    const secret = (await page.locator('.mfa-secret').textContent())!.trim();
    await page.getByLabel('Authenticator code', { exact: true }).fill(authenticatorCode(secret));
    await page.getByRole('button', { name: 'Verify and enable', exact: true }).click();
    let dialog = page.getByRole('dialog');
    await expect(
      dialog.getByRole('heading', { name: 'Save your recovery codes', exact: true }),
    ).toBeVisible();
    await expect(dialog.locator('.recovery-codes code')).toHaveCount(10);
    const codes = await dialog.locator('.recovery-codes code').allTextContents();
    await expect(
      dialog.getByRole('button', { name: 'Done, codes saved', exact: true }),
    ).toBeDisabled();
    const downloaded = page.waitForEvent('download');
    await dialog.getByRole('button', { name: 'Download recovery codes', exact: true }).click();
    const download = await downloaded;
    expect(download.suggestedFilename()).toMatch(/^myfans-recovery-codes-\d{4}-\d{2}-\d{2}\.txt$/);
    const chunks: Buffer[] = [];
    for await (const chunk of (await download.createReadStream())!) chunks.push(Buffer.from(chunk));
    const file = Buffer.concat(chunks).toString('utf8');
    expect(codes.every((code) => file.includes(code))).toBe(true);
    await expect(
      dialog.getByRole('button', { name: 'Done, codes saved', exact: true }),
    ).toBeDisabled();
    expect(
      (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze())
        .violations,
    ).toEqual([]);
    await dialog
      .getByRole('checkbox', {
        name: 'I have saved my recovery codes somewhere safe.',
        exact: true,
      })
      .check();
    await dialog.getByRole('button', { name: 'Done, codes saved', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await page.reload();
    await ready(page);
    await expect(page.locator('.recovery-codes')).toHaveCount(0);
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.goto('/auth');
    await ready(page);
    await page.getByLabel('Email address', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByText('Authenticator or recovery code, if enabled', { exact: true }).click();
    await page.getByLabel('Six-digit code', { exact: true }).fill('123456');
    await page.getByLabel('Verification method', { exact: true }).selectOption('recovery');
    await page.getByLabel('Recovery code', { exact: true }).fill(codes[0]);
    const loginRequest = page.waitForRequest(
      (request) => request.url().endsWith('/api/auth/login') && request.method() === 'POST',
    );
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const loginBody = (await loginRequest).postDataJSON();
    expect(loginBody.recovery_code).toBe(codes[0]);
    expect(loginBody).not.toHaveProperty('mfa_code');
    await expect(page).toHaveURL(/\/settings$/);
    await ready(page);
    await expect(page.getByText('Replace your authenticator.', { exact: true })).toBeVisible();
    const account = await (await page.request.get('/api/platform/account')).json();
    expect(account.user.mfa_enabled).toBe(true);
    expect(account.user.mfa_recovery_required).toBe(true);
    expect(account.recovery_codes_remaining).toBe(9);
    expect(
      (await (await otherSession.request.get(`${origin}/api/platform/account`)).json()).user,
    ).toBeNull();
    expect(
      (
        await replay.request.post(`${origin}/api/auth/login`, {
          headers: { Origin: origin },
          data: { email, password, recovery_code: codes[0] },
        })
      ).status(),
    ).toBe(401);
    await page.request.post('/api/auth/recover', { headers: { Origin: origin }, data: { email } });
    const inbox = await (await page.request.get('/api/platform/account/mail')).json();
    const resetLink = inbox.deliveries.find(
      (item: { subject: string }) => item.subject === 'Reset your password',
    ).body;
    const newPassword = 'RecoveredBrowserOnly!2026';
    await page.goto(resetLink);
    await ready(page);
    await page.getByLabel('Password', { exact: true }).fill(newPassword);
    await page.getByText('Authenticator or recovery code, if enabled', { exact: true }).click();
    await page.getByLabel('Verification method', { exact: true }).selectOption('recovery');
    await page.getByLabel('Recovery code', { exact: true }).fill(codes[1]);
    const resetRequest = page.waitForRequest(
      (request) => request.url().endsWith('/api/auth/reset') && request.method() === 'POST',
    );
    await page.getByRole('button', { name: 'Reset password', exact: true }).click();
    expect((await resetRequest).postDataJSON()).not.toHaveProperty('mfa_code');
    await expect(page.getByRole('status')).toContainText('unused recovery code');
    await page.getByRole('button', { name: 'Back to sign in', exact: true }).click();
    await expect(page.getByLabel('Password', { exact: true })).toHaveValue('');
    await page.getByLabel('Email address', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill(newPassword);
    await page.getByText('Authenticator or recovery code, if enabled', { exact: true }).click();
    await page.getByLabel('Verification method', { exact: true }).selectOption('recovery');
    await page.getByLabel('Recovery code', { exact: true }).fill(codes[2]);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page).toHaveURL(/\/settings$/);
    await page
      .getByRole('button', { name: 'Set up replacement authenticator', exact: true })
      .click();
    await expect(page.locator('.mfa-secret')).toBeVisible();
    const replacementSecret = (await page.locator('.mfa-secret').textContent())!.trim();
    const replacementCode = authenticatorCode(replacementSecret);
    await page.getByLabel('Authenticator code', { exact: true }).fill(replacementCode);
    await page.getByRole('button', { name: 'Verify and enable', exact: true }).click();
    dialog = page.getByRole('dialog');
    await expect(dialog.locator('.recovery-codes code')).toHaveCount(10);
    const newCodes = await dialog.locator('.recovery-codes code').allTextContents();
    expect(newCodes.some((code) => codes.includes(code))).toBe(false);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByText('Replace your authenticator.', { exact: true })).toHaveCount(0);
    const restored = await (await page.request.get('/api/platform/account')).json();
    expect(restored.user.mfa_recovery_required).toBe(false);
    expect(restored.recovery_codes_remaining).toBe(10);
    const exported = await (await page.request.get('/api/platform/account/export')).text();
    expect(
      [...codes, ...newCodes, secret, replacementSecret].some((value) => exported.includes(value)),
    ).toBe(false);
    expect(await page.evaluate(() => Object.keys(localStorage).length)).toBe(0);
    await page.getByRole('button', { name: 'Rotate recovery codes', exact: true }).click();
    dialog = page.getByRole('dialog');
    await dialog.getByLabel('Current password', { exact: true }).fill(newPassword);
    await expect
      .poll(() => authenticatorCode(replacementSecret), { timeout: 35000, intervals: [1000] })
      .not.toBe(replacementCode);
    await dialog
      .getByLabel('Authenticator code', { exact: true })
      .fill(authenticatorCode(replacementSecret));
    await dialog.getByRole('button', { name: 'Generate new recovery codes', exact: true }).click();
    await expect(
      page
        .getByRole('dialog')
        .getByRole('heading', { name: 'Save your recovery codes', exact: true }),
    ).toBeVisible();
    await expect(page.getByRole('dialog').locator('.recovery-codes code')).toHaveCount(10);
    await page.getByRole('dialog').getByRole('checkbox').check();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Done, codes saved', exact: true })
      .click();
    expect(
      (
        await replay.request.post(`${origin}/api/auth/login`, {
          headers: { Origin: origin },
          data: { email, password: newPassword, recovery_code: newCodes[0] },
        })
      ).status(),
    ).toBe(401);
  } finally {
    await otherSession.close();
    await replay.close();
  }
});

test('likes, bookmarks, and comments survive reloads and remain account scoped', async ({
  page,
  browser,
}) => {
  const unique = await register(page);
  await verify(page);
  const initial = await feed(page);
  const post = initial.posts.find((item) => item.visibility === 'public')!;
  const card = page.locator(`#post-${post.id}`);
  await card.getByRole('button', { name: 'Save post', exact: true }).click();
  await expect(card.getByRole('button', { name: 'Unsave post', exact: true })).toBeVisible();
  await card.getByRole('button', { name: /^Like .*post$/ }).click();
  await expect(card.getByRole('button', { name: /^Unlike .*post$/ })).toBeVisible();
  await card.getByRole('button', { name: 'Show comments', exact: true }).click();
  const comment = `A thoughtful comment ${unique.slice(0, 8)}`;
  await card.getByRole('textbox', { name: 'Write a comment', exact: true }).fill(comment);
  await card.getByRole('button', { name: 'Add comment', exact: true }).click();
  await expect(card.locator('.local-comment').filter({ hasText: comment })).toBeVisible();
  await page.reload();
  await ready(page);
  await expect(card.getByRole('button', { name: 'Unsave post', exact: true })).toBeVisible();
  await expect(card.getByRole('button', { name: /^Unlike .*post$/ })).toBeVisible();
  await page.getByRole('tab', { name: 'Saved', exact: true }).click();
  await expect(page).toHaveURL(/tab=saved/);
  await expect(page.locator('article.post')).toHaveCount(1);
  const otherContext = await browser.newContext();
  const anonymous = await otherContext.request.get(
    `${new URL(page.url()).origin}/api/platform/feed`,
  );
  const otherFeed: FeedResponse = await anonymous.json();
  expect(otherFeed.posts.find((item) => item.id === post.id)?.saved).toBe(false);
  expect(otherFeed.posts.find((item) => item.id === post.id)?.liked).toBe(false);
  await otherContext.close();
});

test('sandbox checkout requires explicit confirmation before granting subscriber access', async ({
  page,
  context,
}) => {
  await register(page);
  await verify(page);
  const initial = await feed(page);
  const post = initial.posts.find((item) => item.visibility === 'subscribers' && item.locked)!;
  const card = page.locator(`#post-${post.id}`);
  expect(post.media_url).toBeNull();
  await card.getByRole('button', { name: 'Join their circle', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText('Sandbox payment', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Review sandbox checkout', exact: true }).click();
  await expect(dialog.getByRole('button', { name: /Confirm simulated payment/ })).toBeVisible();
  expect((await feed(page)).posts.find((item) => item.id === post.id)?.locked).toBe(true);
  await dialog.getByRole('button', { name: /Confirm simulated payment/ }).click();
  await expect(dialog.getByRole('status')).toContainText('confirmed');
  await dialog.getByRole('button', { name: 'Back to your space', exact: true }).click();
  await expect(card.locator('.locked-content')).toHaveCount(0);
  await page.reload();
  await ready(page);
  await expect(card.locator('.locked-content')).toHaveCount(0);
  const session = (await context.cookies()).find((cookie) => cookie.name === 'myfans_session');
  expect(session?.httpOnly).toBe(true);
  expect(session?.sameSite).toBe('Lax');
  expect(await page.content()).not.toContain(session!.value);
  await page.getByRole('tab', { name: 'Following', exact: true }).click();
  await expect(page).toHaveURL(/tab=following/);
  await expect(page.locator('article.post')).toHaveCount(
    initial.posts.filter((item) => item.creator_id === post.creator_id).length,
  );
  await page.goto('/settings');
  await ready(page);
  const subscription = page
    .locator('.subscription-row')
    .filter({ hasText: 'Cancels at period end' });
  await expect(subscription).toHaveCount(1);
  await expect(subscription).toContainText('Current period ends');
  await expect(subscription).not.toContainText('Invalid Date');
  await expect(
    subscription.getByRole('button', { name: 'Cancel at period end', exact: true }),
  ).toHaveCount(0);
  const history = page.getByRole('region', { name: 'Payment history', exact: true });
  await expect(history.locator('article')).toHaveCount(1);
  await expect(history).toContainText('Sandbox simulation; no real charge');
  await expect(history).toContainText('Receipt reference:');
  await expect(history).toContainText('No subscription period is recorded on this payment.');
  const account = await (await page.request.get('/api/platform/account')).json();
  expect(account.payments).toHaveLength(1);
  expect(account.payments[0]).toMatchObject({
    creator_id: post.creator_id,
    kind: 'subscription',
    status: 'paid',
    provider: 'sandbox',
    refunded_minor: 0,
    dispute_recorded: false,
  });
});

test('checkout return hints never confirm payment or grant access', async ({ page }) => {
  await register(page);
  await verify(page);
  const before = await feed(page);
  const locked = before.posts.find((post) => post.visibility === 'subscribers' && post.locked)!;
  await page.goto('/settings?checkout=returned');
  await ready(page);
  await expect(
    page.getByRole('complementary', { name: 'Checkout return', exact: true }),
  ).toContainText('does not confirm payment or grant access');
  await expect(page.getByRole('region', { name: 'Payment history', exact: true })).toContainText(
    'No payment records',
  );
  const account = await (await page.request.get('/api/platform/account')).json();
  expect(account.payments).toEqual([]);
  expect((await feed(page)).posts.find((post) => post.id === locked.id)?.locked).toBe(true);
  await page.goto('/?checkout=canceled');
  await ready(page);
  await expect(
    page.getByRole('complementary', { name: 'Checkout return', exact: true }),
  ).toContainText('if you are unsure whether a payment was completed');
  expect((await feed(page)).posts.find((post) => post.id === locked.id)?.locked).toBe(true);
});

test('resumable uploads verify server offsets after reload without creating duplicate media', async ({
  page,
}, testInfo) => {
  const unique = await register(page);
  await verify(page);
  const creator = await page.request.post('/api/platform/creators/profile', {
    headers: { origin: new URL(page.url()).origin },
    data: {
      handle: `resume_${unique.slice(0, 12)}`,
      display_name: 'Resumable Studio',
      bio: 'Isolated browser upload fixture.',
      category: 'Art',
      subscription_price_minor: 1000,
    },
  });
  expect(creator.ok()).toBe(true);
  await page.goto('/studio');
  await ready(page);
  const file = testInfo.outputPath('resumable.png');
  await mkdir(dirname(file), { recursive: true });
  await writeFile(
    file,
    Buffer.concat([
      Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNw6VACAAICAO8aPpvKAAAAAElFTkSuQmCC',
        'base64',
      ),
      Buffer.alloc(6 * 1024 * 1024),
    ]),
  );
  let interrupted = false;
  const offsets: number[] = [];
  await page.route('**/api/platform/media/uploads/*', async (route) => {
    if (route.request().method() !== 'PATCH') {
      await route.continue();
      return;
    }
    const offset = Number(route.request().headers()['upload-offset']);
    offsets.push(offset);
    if (offset > 0 && !interrupted) {
      interrupted = true;
      await route.fulfill({ status: 503, json: { message: 'Upload interruption fixture' } });
    } else await route.continue();
  });
  await page.getByLabel('Choose media', { exact: true }).setInputFiles(file);
  await page.getByRole('button', { name: 'Upload to private library', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Upload interruption fixture');
  const saved = await page.evaluate(
    () => Object.entries(localStorage).find(([key]) => key.startsWith('myfans:upload:'))?.[1],
  );
  expect(saved).toBeTruthy();
  expect(saved).not.toContain('resumable.png');
  const metadata = JSON.parse(saved!);
  expect(metadata).not.toHaveProperty('content');
  await page.reload();
  await ready(page);
  await expect(page.getByRole('status').filter({ hasText: 'Saved upload found' })).toBeVisible();
  await page.getByLabel('Choose media', { exact: true }).setInputFiles({
    name: 'other.png',
    mimeType: 'image/png',
    buffer: Buffer.alloc(6 * 1024 * 1024),
  });
  await page.getByRole('button', { name: 'Resume upload', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Choose the original file');
  await page.getByLabel('Choose media', { exact: true }).setInputFiles(file);
  await page.getByRole('button', { name: 'Resume upload', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Upload received' })).toBeVisible();
  expect(offsets).toEqual([0, 4194304, 4194304]);
  const studio = await page.request.get('/api/platform/studio');
  const media = (await studio.json()).media;
  expect(media).toHaveLength(1);
  expect(media[0].id).toBe(metadata.id);
  expect(
    await page.evaluate(() =>
      Object.keys(localStorage).some((key) => key.startsWith('myfans:upload:')),
    ),
  ).toBe(false);
});

test('erasure requests require explicit confirmation and retain a pending receipt before sign-out', async ({
  page,
}) => {
  const unique = await register(page);
  await verify(page);
  await page.goto('/settings');
  await ready(page);
  await page.getByRole('button', { name: 'Request erasure', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Erasure is reviewed separately');
  await dialog.getByLabel('Erasure current password', { exact: true }).fill(password);
  await dialog.getByLabel('Type ERASE to confirm', { exact: true }).fill('erase');
  await expect(
    dialog.getByRole('button', { name: 'Confirm erasure request', exact: true }),
  ).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await page.getByRole('button', { name: 'Request erasure', exact: true }).click();
  await expect(dialog.getByLabel('Erasure current password', { exact: true })).toHaveValue('');
  await dialog.getByLabel('Erasure current password', { exact: true }).fill(password);
  await dialog.getByLabel('Type ERASE to confirm', { exact: true }).fill('ERASE');
  await dialog.getByRole('button', { name: 'Confirm erasure request', exact: true }).click();
  await expect(
    dialog.getByRole('heading', { name: 'Erasure request received', exact: true }),
  ).toBeVisible();
  await expect(dialog).toContainText('pending request');
  await expect(dialog).toContainText('does not certify erasure');
  const download = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Download request receipt', exact: true }).click();
  expect((await download).suggestedFilename()).toBe('myfans-erasure-request.json');
  await dialog.getByRole('button', { name: 'Continue to sign in', exact: true }).click();
  await expect(page).toHaveURL(/\/auth\?erasure=requested/);
  const account = await page.request.get('/api/platform/account');
  expect((await account.json()).user).toBeNull();
  const login = await page.request.post('/api/auth/login', {
    headers: { origin: new URL(page.url()).origin },
    data: { email: `browser-${unique}@myfans.test`, password },
  });
  expect(login.ok()).toBe(false);
});

test('creator onboarding and drafts enforce review before publication', async ({ page }) => {
  const unique = await register(page);
  await verify(page);
  await page.goto('/studio');
  await ready(page);
  await page.getByLabel('Creator name', { exact: true }).fill('Browser Studio');
  await page.getByLabel('Handle', { exact: true }).fill(`studio_${unique.slice(0, 12)}`);
  await page
    .getByLabel('About you', { exact: true })
    .fill('An independent creator studio for browser verification.');
  await page.getByRole('button', { name: 'Save creator profile', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Creator profile saved');
  const progress = page.getByRole('region', { name: 'Creator setup progress', exact: true });
  await expect(progress).toContainText('Verified');
  await expect(progress).toContainText('Sandbox review; no identity verification');
  await expect(progress).toContainText('Sandbox payments; no bank account connection');
  await expect(progress).toContainText('pending');
  await expect(
    progress.getByRole('button', { name: 'Request another creator review', exact: true }),
  ).toHaveCount(0);
  const creatorAccount = await (await page.request.get('/api/platform/studio')).json();
  expect(creatorAccount.onboarding.viewer_id).toBe((await feed(page)).viewer.id);
  expect(creatorAccount.onboarding.email_verified).toBe(true);
  await page.getByLabel('Choose media', { exact: true }).setInputFiles({
    name: 'tiny.png',
    mimeType: 'image/png',
    // Exceed adapter-node's default 512 KB limit and verify that decoding strips padding.
    buffer: Buffer.concat([
      Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNw6VACAAICAO8aPpvKAAAAAElFTkSuQmCC',
        'base64',
      ),
      Buffer.alloc(600 * 1024),
    ]),
  });
  await page.getByRole('button', { name: 'Upload to private library', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Upload received');
  const mediaResponse = await page.request.get('/api/platform/studio');
  const studio = await mediaResponse.json();
  expect(studio.media).toHaveLength(1);
  expect(['processing', 'quarantine']).toContain(studio.media[0].state);
  await expect
    .poll(
      async () => {
        const result = await page.request.get('/api/platform/studio');
        return (await result.json()).media[0].state;
      },
      { timeout: 15000 },
    )
    .toBe('quarantine');
  await expect(
    page.getByRole('combobox', { name: 'Attach approved media', exact: true }).locator('option'),
  ).toHaveCount(1);
  const caption = `My first persistent draft ${unique.slice(0, 8)}`;
  await page.getByLabel('Caption', { exact: true }).fill(caption);
  await page.getByRole('button', { name: 'Save draft', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Draft saved');
  await page.reload();
  await ready(page);
  await expect(page.getByText(caption, { exact: true })).toBeVisible();
  const postRow = page.locator('.studio-posts .list-row').filter({ hasText: caption });
  await postRow.getByRole('button', { name: 'Publish', exact: true }).click();
  await expect(page.getByRole('alert')).toBeVisible();
  await expect(postRow.getByText('draft', { exact: true })).toBeVisible();
});

test('private conversation messages are persisted and shown only to participants', async ({
  page,
  browser,
}) => {
  const unique = await register(page);
  await verify(page);
  const creator = (await feed(page)).creators[0];
  await page.goto(`/messages?recipient=${creator.user_id}`);
  await ready(page);
  await expect(
    page.getByRole('heading', { name: creator.display_name, exact: true }),
  ).toBeVisible();
  const body = `Hello from my circle ${unique.slice(0, 8)}`;
  await page.getByRole('textbox', { name: 'Your message', exact: true }).fill(body);
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(page.locator('.message-bubble').filter({ hasText: body })).toBeVisible();
  await page.reload();
  await ready(page);
  await expect(page.locator('.message-bubble').filter({ hasText: body })).toBeVisible();
  const response = await page.request.get('/api/platform/conversations');
  const { conversations } = await response.json();
  expect(conversations).toHaveLength(1);
  const unrelatedContext = await browser.newContext();
  const origin = new URL(page.url()).origin;
  const registration = await unrelatedContext.request.post(`${origin}/api/auth/register`, {
    headers: { origin },
    data: { email: `unrelated-${unique}@myfans.test`, display_name: 'Unrelated reader', password },
  });
  expect(registration.ok()).toBe(true);
  const forbidden = await unrelatedContext.request.get(
    `${origin}/api/platform/conversations/${conversations[0].id}/messages`,
  );
  expect([403, 404]).toContain(forbidden.status());
  await unrelatedContext.close();
  const otherCreator = (await feed(page)).creators.find((item) => item.id !== creator.id)!;
  const started = await page.request.post('/api/platform/conversations', {
    headers: { origin },
    data: { recipient_id: otherCreator.user_id },
  });
  expect(started.ok()).toBe(true);
  await page.reload();
  await ready(page);
  const choose = async (name: string) => {
    await page.locator('.conversation').filter({ hasText: name }).click();
    await expect(page.locator('.thread-heading h2')).toHaveText(name);
  };
  const draftField = page.getByRole('textbox', { name: 'Your message', exact: true });
  await choose(creator.display_name);
  const firstDraft = `Draft only for ${creator.display_name} ${unique}`;
  await draftField.fill(firstDraft);
  await choose(otherCreator.display_name);
  await expect(draftField).toHaveValue('');
  const secondDraft = `Draft only for ${otherCreator.display_name} ${unique}`;
  await draftField.fill(secondDraft);
  await choose(creator.display_name);
  await expect(draftField).toHaveValue(firstDraft);
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(page.locator('.message-bubble').filter({ hasText: firstDraft })).toBeVisible();
  await choose(otherCreator.display_name);
  await expect(draftField).toHaveValue(secondDraft);
  await expect(page.locator('.message-bubble').filter({ hasText: firstDraft })).toHaveCount(0);
  await choose(creator.display_name);
  await expect(draftField).toHaveValue('');
});

test('shared post links resolve directly and unavailable IDs disclose no post', async ({
  page,
}) => {
  const initial = await feed(page);
  const post = initial.posts[initial.posts.length - 1];
  expect((await page.request.get(`/api/platform/admin/posts/${post.id}`)).status()).toBe(401);
  await page.goto(`/?creator=${post.creator_id}`);
  await ready(page);
  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, 'writeText', {
      configurable: true,
      value: async (value: string) => {
        (window as Window & { copiedPostLink?: string }).copiedPostLink = value;
      },
    });
  });
  await page.locator(`#post-${post.id}`).getByRole('button', { name: 'Copy post link' }).click();
  const copied = await page.evaluate(
    () => (window as Window & { copiedPostLink?: string }).copiedPostLink,
  );
  expect(copied).toBe(`${new URL(page.url()).origin}/?post=${post.id}#post-${post.id}`);
  await page.goto(copied!);
  await ready(page);
  await expect(page.locator('.post')).toHaveCount(1);
  await expect(page.locator(`#post-${post.id} .caption`)).toHaveText(post.caption);
  if (post.locked) await expect(page.locator('.locked-content')).toBeVisible();
  const direct = await page.request.get(`/api/platform/feed?post=${post.id}`);
  expect(direct.ok()).toBe(true);
  const result = (await direct.json()) as FeedResponse;
  expect(result.posts).toHaveLength(1);
  expect(result.posts[0].locked).toBe(post.locked);
  if (post.locked) expect(result.posts[0].media_url).toBeNull();
  await page.goto(`/?post=${crypto.randomUUID()}`);
  await ready(page);
  await expect(
    page.getByRole('heading', { name: 'This post is unavailable.', exact: true }),
  ).toBeVisible();
  await expect(page.locator('.post')).toHaveCount(0);
  await page.getByRole('link', { name: 'Back to the feed', exact: true }).click();
  await expect(page).toHaveURL(/\/#feed-panel$/);
  const directoryControl = page.getByRole('link', { name: 'Explore all creators', exact: true });
  if (await directoryControl.isVisible()) await directoryControl.click();
  else await page.getByRole('link', { name: 'Find your people', exact: true }).click();
  await expect(page).toHaveURL(/\/creators$/);
  await register(page);
  expect((await page.request.get(`/api/platform/admin/posts/${post.id}`)).status()).toBe(403);
});

test('blocking hides discovery and restricts new messages while preserving readable history', async ({
  page,
}) => {
  const unique = await register(page);
  await verify(page);
  const creator = (await feed(page)).creators[0];
  await page.goto(`/messages?recipient=${creator.user_id}`);
  await ready(page);
  await expect(
    page.getByRole('heading', { name: creator.display_name, exact: true }),
  ).toBeVisible();
  const body = `Readable history ${unique.slice(0, 8)}`;
  await page.getByRole('textbox', { name: 'Your message', exact: true }).fill(body);
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(page.locator('.message-bubble').filter({ hasText: body })).toBeVisible();
  await page.getByRole('button', { name: 'Block user', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('no financial changes');
  await dialog.getByRole('button', { name: 'Confirm block', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'User blocked' })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Your message', exact: true })).toBeDisabled();
  await expect(page.locator('.message-bubble').filter({ hasText: body })).toBeVisible();
  const blockedFeed = await feed(page);
  expect(blockedFeed.posts.some((post) => post.creator_id === creator.id)).toBe(false);
  expect(blockedFeed.creators.some((person) => person.id === creator.id)).toBe(false);
  await page.goto('/settings');
  await ready(page);
  const blockRow = page.locator('.safety-row').filter({ hasText: creator.display_name });
  await blockRow.getByRole('button', { name: 'Unblock user', exact: true }).click();
  await dialog.getByRole('button', { name: 'Confirm unblock', exact: true }).click();
  await expect(
    page.getByRole('status').filter({ hasText: 'Your block has been removed' }),
  ).toBeVisible();
  expect((await feed(page)).creators.some((person) => person.id === creator.id)).toBe(true);
  await page.goto('/messages');
  await ready(page);
  await expect(page.getByRole('textbox', { name: 'Your message', exact: true })).toBeEnabled();
  await expect(page.locator('.message-bubble').filter({ hasText: body })).toBeVisible();
});

test('automatic updates preserve message drafts and pause while reading older history', async ({
  page,
}) => {
  test.setTimeout(60000);
  const unique = await register(page);
  await verify(page);
  const creator = (await feed(page)).creators[0];
  await page.goto(`/messages?recipient=${creator.user_id}`);
  await ready(page);
  await expect(
    page.getByRole('heading', { name: creator.display_name, exact: true }),
  ).toBeVisible();
  const response = await page.request.get('/api/platform/conversations');
  const threadId = (await response.json()).conversations[0].id;
  const cursor = `2026-10-07T00:00:00Z|${crypto.randomUUID()}`;
  const newest = Array.from({ length: 50 }, (_, index) => ({
    id: crypto.randomUUID(),
    sender_id: creator.user_id,
    body: `New page ${index} ${unique}`,
    created_at: new Date(Date.UTC(2026, 9, 7, 1, index)).toISOString(),
  }));
  const older = Array.from({ length: 5 }, (_, index) => ({
    id: crypto.randomUUID(),
    sender_id: creator.user_id,
    body: `Older page ${index} ${unique}`,
    created_at: new Date(Date.UTC(2026, 9, 7, 0, index)).toISOString(),
  }));
  // The real backend authorizes each read; bounded response fixtures exercise
  // background rendering and history retention without writing to shared creators.
  await page.route(`**/api/platform/conversations/${threadId}/messages*`, async (route) => {
    const actual = await route.fetch();
    expect(actual.ok()).toBe(true);
    const result = await actual.json();
    const isOlder = new URL(route.request().url()).searchParams.has('cursor');
    await route.fulfill({
      response: actual,
      json: { ...result, messages: isOlder ? older : newest, next_cursor: isOlder ? null : cursor },
    });
  });
  const draft = `Unsent draft ${unique}`;
  await page.getByRole('textbox', { name: 'Your message', exact: true }).fill(draft);
  await expect(page.locator('.message-bubble')).toHaveCount(50, { timeout: 16000 });
  await expect(page.getByRole('textbox', { name: 'Your message', exact: true })).toHaveValue(draft);
  await page.getByRole('button', { name: 'Older messages', exact: true }).click();
  await expect(page.locator('.message-bubble')).toHaveCount(55);
  await expect(
    page.getByRole('status').filter({ hasText: 'Automatic updates pause' }),
  ).toBeVisible();
  const unexpected = `Later message ${unique}`;
  newest[49] = { ...newest[49], body: unexpected };
  await page.waitForTimeout(11000);
  await expect(page.locator('.message-bubble')).toHaveCount(55);
  await expect(page.getByText(unexpected, { exact: true })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Your message', exact: true })).toHaveValue(draft);
  await page.route('**/api/platform/notifications', async (route) => {
    const actual = await route.fetch();
    expect(actual.ok()).toBe(true);
    const original = await actual.json();
    await route.fulfill({
      response: actual,
      json: {
        viewer_id: original.viewer_id,
        notifications: [
          {
            id: crypto.randomUUID(),
            title: `Live update ${unique}`,
            body: 'New notification fixture.',
            created_at: new Date().toISOString(),
            read_at: null,
          },
        ],
        next_cursor: null,
      },
    });
  });
  await page.goto('/notifications');
  await ready(page);
  await expect(page.getByText(`Live update ${unique}`, { exact: true })).toBeVisible({
    timeout: 22000,
  });
});

test('keyboard navigation, native dialog dismissal, and all routes fit the viewport', async ({
  page,
}) => {
  await page.getByRole('tab', { name: 'Discover', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Saved', exact: true })).toBeFocused();
  await page.getByRole('tab', { name: 'Discover', exact: true }).click();
  await page.getByRole('button', { name: "View Ella Wild's profile", exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  for (const path of [
    '/',
    '/auth?mode=register',
    '/studio',
    '/messages',
    '/notifications',
    '/settings',
    '/earnings',
    '/admin',
    '/legal',
  ]) {
    await page.goto(path);
    await ready(page);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
    await expect(page.locator('#main-content')).toBeVisible();
  }
});

test('older comments preserve current records, retry failures, and deduplicate page boundaries', async ({
  page,
}) => {
  await register(page);
  const publicPost = (await feed(page)).posts.find((post) => post.visibility === 'public')!;
  const fixture = Array.from({ length: 52 }, (_, index) => ({
    id: crypto.randomUUID(),
    display_name: 'Pagination fixture',
    body: `Comment history ${index + 1}`,
    created_at: new Date(Date.now() + index * 1000).toISOString(),
  }));
  const cursor = `${fixture[2].created_at}|${fixture[2].id}`;
  let failOlder = true;
  await page.route(`**/api/platform/posts/${publicPost.id}/comments*`, async (route) => {
    const older = new URL(route.request().url()).searchParams.has('cursor');
    if (older && failOlder) {
      failOlder = false;
      await route.fulfill({ status: 503, json: { message: 'Temporary history outage' } });
    } else
      await route.fulfill({
        json: {
          comments: older ? fixture.slice(0, 3) : fixture.slice(2),
          next_cursor: older ? null : cursor,
        },
      });
  });
  const card = page.locator(`#post-${publicPost.id}`);
  await card.getByRole('button', { name: 'Show comments', exact: true }).click();
  await expect(card.locator('.local-comment')).toHaveCount(50);
  await card.getByRole('button', { name: 'Older comments', exact: true }).click();
  await expect(card.getByRole('alert')).toContainText('Temporary history outage');
  await expect(card.locator('.local-comment')).toHaveCount(50);
  await card.getByRole('button', { name: 'Older comments', exact: true }).click();
  await expect(card.locator('.local-comment')).toHaveCount(52);
  await expect(card.locator('.local-comment').first()).toContainText('Comment history 1');
  await expect(card.locator('.local-comment').last()).toContainText('Comment history 52');
  await expect(card.getByRole('button', { name: 'Older comments', exact: true })).toHaveCount(0);
  await card.getByRole('button', { name: 'Show comments', exact: true }).click();
  await expect(card.locator('.local-comment')).toHaveCount(0);
  await card.getByRole('button', { name: 'Show comments', exact: true }).click();
  await expect(card.locator('.local-comment')).toHaveCount(50);
});

test('BFF rejects cross-origin writes, oversized JSON, and unknown proxy routes', async ({
  request,
  baseURL,
}) => {
  const payload = {
    kind: 'subscription',
    creator_id: 'a1111111-1111-4111-8111-111111111111',
    idempotency_key: crypto.randomUUID(),
  };
  const blocked = await request.post('/api/platform/checkout', {
    headers: { origin: 'https://untrusted.example' },
    data: payload,
  });
  expect(blocked.status()).toBe(403);
  const oversized = await request.post('/api/platform/checkout', {
    headers: { origin: baseURL!, 'content-type': 'application/json' },
    data: JSON.stringify({ ...payload, excess: 'x'.repeat(20000) }),
  });
  expect(oversized.status()).toBe(413);
  expect((await request.get('/api/platform/auth/login')).status()).toBe(404);
  const chunk = '/api/platform/media/uploads/a1111111-1111-4111-8111-111111111111';
  for (const offset of ['-1', '00', 'NaN', '268435457']) {
    const invalid = await request.patch(chunk, {
      headers: {
        origin: baseURL!,
        'content-type': 'application/octet-stream',
        'upload-offset': offset,
      },
      data: Buffer.from('chunk'),
    });
    expect(invalid.status()).toBe(400);
  }
  const oversizedChunk = await request.patch(chunk, {
    headers: { origin: baseURL!, 'content-type': 'application/octet-stream', 'upload-offset': '0' },
    data: Buffer.alloc(4194305),
  });
  expect(oversizedChunk.status()).toBe(413);
  expect(
    (
      await request.get(
        '/api/platform/media/a1111111-1111-4111-8111-111111111111/hls/a2222222-2222-4222-8222-222222222222.m3u8',
      )
    ).status(),
  ).toBe(404);
});
