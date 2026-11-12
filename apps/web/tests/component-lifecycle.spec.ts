import { expect, type Page, type Route } from '@playwright/test';
import { test } from './fixtures.ts';

const password = 'LifecycleTestOnly!2026';

async function ready(page: Page) {
  await expect(page.locator('[data-hydrated="true"]')).toBeVisible();
}

async function account(page: Page) {
  await page.goto('/auth?mode=register');
  await ready(page);
  await page.getByLabel('Your name', { exact: true }).fill('Lifecycle Test');
  await page
    .getByLabel('Email address', { exact: true })
    .fill(`lifecycle-${crypto.randomUUID()}@myfans.test`);
  await page.getByLabel('Password', { exact: true }).fill(password);
  await page.getByLabel('Country of residence', { exact: true }).selectOption('US');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
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
}

function deferredMutation(page: Page, path: string) {
  let release!: () => void;
  let arrived!: () => void;
  const pending = new Promise<void>((resolve) => (release = resolve));
  const received = new Promise<void>((resolve) => (arrived = resolve));
  let completed: Promise<void> = Promise.resolve();
  let readsAfterLeaving = 0;
  let leaving = false;
  page.on('request', (request) => {
    if (leaving && request.method() === 'GET' && new URL(request.url()).pathname === path)
      readsAfterLeaving++;
  });
  const handler = async (route: Route) => {
    if (route.request().method() !== 'POST') return route.continue();
    arrived();
    completed = (async () => {
      await pending;
      // This response is deliberately held at the browser boundary. No server
      // mutation is sent, and cancellation makes no claim about committed writes.
      await route.fulfill({ json: { status: 'created' } }).catch(() => {});
    })();
    await completed;
  };
  return {
    handler,
    received,
    leave: () => (leaving = true),
    release,
    settle: () => completed,
    reads: () => readsAfterLeaving,
  };
}

async function discover(page: Page) {
  const name =
    (page.viewportSize()?.width ?? 1440) <= 760 ? 'Mobile navigation' : 'Main navigation';
  await page
    .getByRole('navigation', { name, exact: true })
    .getByRole('link', { name: 'Discover', exact: true })
    .click();
  await expect(page).toHaveURL(/\/creators$/);
  await ready(page);
}

for (const kind of ['message', 'comment'] as const) {
  test(`abandoned ${kind} responses do not restart private loads after navigation`, async ({
    page,
  }) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    let mutation: ReturnType<typeof deferredMutation> | undefined;
    try {
      await account(page);
      const response = await page.request.get('/api/platform/feed');
      expect(response.ok()).toBe(true);
      const feed = await response.json();
      let path: string;
      if (kind === 'message') {
        const creator = feed.creators[0];
        await page.goto(`/messages?recipient=${creator.user_id}`);
        await ready(page);
        await expect(
          page.getByRole('textbox', { name: 'Your message', exact: true }),
        ).toBeEnabled();
        const list = await page.request.get('/api/platform/conversations');
        const { conversations } = await list.json();
        expect(conversations).toHaveLength(1);
        path = `/api/platform/conversations/${conversations[0].id}/messages`;
        await page
          .getByRole('textbox', { name: 'Your message', exact: true })
          .fill('A private unsent fixture');
      } else {
        const post = feed.posts.find(
          (item: { locked: boolean; visibility: string }) =>
            !item.locked && item.visibility === 'public',
        );
        expect(post).toBeTruthy();
        await page.goto(`/?creator=${post.creator_id}`);
        await ready(page);
        // The post ID is the article ID, rather than a caption or list position.
        const article = page.locator(`article#post-${post.id}`);
        await article.getByRole('button', { name: 'Show comments', exact: true }).click();
        await article
          .getByLabel('Write a comment', { exact: true })
          .fill('A private unsent fixture');
        path = `/api/platform/posts/${post.id}/comments`;
      }
      mutation = deferredMutation(page, path);
      await page.route(`**${path}`, mutation.handler);
      await page.clock.install();
      if (kind === 'message')
        await page.getByRole('button', { name: 'Send message', exact: true }).click();
      else await page.getByRole('button', { name: 'Add comment', exact: true }).click();
      await mutation.received;
      // Prove this is client-side unmounting, rather than document replacement.
      await page.evaluate(
        () =>
          ((window as unknown as { lifecycleMarker: string }).lifecycleMarker = 'same document'),
      );
      mutation.leave();
      await discover(page);
      mutation.release();
      await mutation.settle();
      await page.waitForTimeout(150);
      await page.clock.fastForward(30000);
      await page.waitForTimeout(150);
      await expect(page).toHaveURL(/\/creators$/);
      expect(
        await page.evaluate(
          () => (window as unknown as { lifecycleMarker?: string }).lifecycleMarker,
        ),
      ).toBe('same document');
      expect(mutation.reads()).toBe(0);
      expect(errors).toEqual([]);
    } finally {
      mutation?.release();
      await mutation?.settle();
      const result = await page.request.post('/api/platform/account/deactivate', {
        headers: { Origin: new URL(page.url()).origin },
        data: { current_password: password },
      });
      expect(result.ok(), 'The isolated account is disabled and all sessions revoked.').toBe(true);
    }
  });
}
