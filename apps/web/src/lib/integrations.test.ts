import { describe, expect, it } from 'vitest';
import { hostedUrl, parseCheckout, parseIntegrationStatus } from './integrations.ts';

describe('hosted provider boundaries', () => {
  it('accepts exact hosted provider URLs, including official Stripe fragments', () => {
    expect(hostedUrl('https://checkout.stripe.com/c/pay/test#signed', 'checkout')).toContain(
      '#signed',
    );
    expect(hostedUrl('https://connect.stripe.com/setup/e/token', 'payout-account')).toContain(
      '/setup/',
    );
    expect(hostedUrl('https://inquiry.sumsub.com/websdk/p/token', 'verification')).toContain(
      '/websdk/',
    );
  });
  it.each([
    'http://checkout.stripe.com/c/pay/test',
    'https://checkout.stripe.com.evil.test/a',
    'https://evil.test/?next=https://checkout.stripe.com',
    'https://user@checkout.stripe.com/a',
    'https://checkout.stripe.com:8443/a',
    '//checkout.stripe.com/a',
    'javascript:alert(1)',
    'https://checkout.stripe.com\\@evil.test/a',
    ' https://checkout.stripe.com/a',
    'https://checkout.stripe.com/a\n',
  ])('rejects unsafe redirect %s', (url) => expect(() => hostedUrl(url, 'checkout')).toThrow());
  it('does not mix onboarding providers or permit their fragments', () => {
    expect(() => hostedUrl('https://checkout.stripe.com/a', 'verification')).toThrow();
    expect(() => hostedUrl('https://connect.stripe.com/a#token', 'payout-account')).toThrow();
  });
  const session = {
    id: '11111111-1111-4111-8111-111111111111',
    status: 'pending',
    amount_minor: 500,
    currency: 'USD',
    mode: 'sandbox',
  };
  it('requires the sandbox confirmation and rejects mode changes', () => {
    expect(
      parseCheckout({ checkout: session, confirmation_required: true }, 'sandbox').redirectUrl,
    ).toBeNull();
    expect(() =>
      parseCheckout({ checkout: session, confirmation_required: false }, 'sandbox'),
    ).toThrow();
    expect(() =>
      parseCheckout({ checkout: session, confirmation_required: true }, 'stripe'),
    ).toThrow();
  });
  it('requires an allowlisted Stripe redirect and does not mistake creation for payment', () => {
    const checkout = { ...session, mode: 'stripe' };
    expect(
      parseCheckout(
        {
          checkout,
          confirmation_required: false,
          redirect_url: 'https://checkout.stripe.com/c/pay/test',
        },
        'stripe',
      ).checkout.status,
    ).toBe('pending');
    expect(() =>
      parseCheckout(
        {
          checkout: { ...checkout, status: 'paid' },
          confirmation_required: false,
          redirect_url: 'https://checkout.stripe.com/c/pay/test',
        },
        'stripe',
      ),
    ).toThrow();
    expect(() =>
      parseCheckout(
        { checkout, confirmation_required: false, redirect_url: 'https://evil.test' },
        'stripe',
      ),
    ).toThrow();
  });
  it('requires explicit capability and content policy fields', () => {
    const status = {
      payments: 'stripe',
      identity: 'sumsub',
      content_policy: 'general',
      configured: { payments: false, identity: false, storage: true, scanner: true },
    };
    expect(parseIntegrationStatus(status).configured.payments).toBe(false);
    expect(() => parseIntegrationStatus({ ...status, content_policy: 'adult' })).toThrow();
    expect(() => parseIntegrationStatus({ ...status, configured: {} })).toThrow();
  });
});
