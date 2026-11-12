import { test as base } from '@playwright/test';

// Exercise the actual application, database, and private media endpoints while
// keeping public sandbox stock photography/font services outside the test boundary.
// External asset availability and approved production imagery require separate acceptance.
export const test = base.extend<{ isolateSandboxAssets: void }>({
  isolateSandboxAssets: [
    async ({ context }, use) => {
      await context.route('https://fonts.googleapis.com/**', (route) =>
        route.fulfill({ contentType: 'text/css', body: '' }),
      );
      await context.route('https://fonts.gstatic.com/**', (route) => route.abort());
      await context.route('https://images.unsplash.com/**', (route) =>
        route.fulfill({
          contentType: 'image/svg+xml',
          body: '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="480"><rect width="640" height="480" fill="#e4e3db"/></svg>',
        }),
      );
      await use();
    },
    { auto: true },
  ],
});
