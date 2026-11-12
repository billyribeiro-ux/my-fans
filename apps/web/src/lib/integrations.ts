export interface IntegrationStatus {
  payments: 'sandbox' | 'stripe';
  identity: 'sandbox' | 'sumsub';
  content_policy: 'general';
  configured: { payments: boolean; identity: boolean; storage: boolean; scanner: boolean };
}

export function parseIntegrationStatus(value: unknown): IntegrationStatus {
  if (!value || typeof value !== 'object') throw new Error('Provider status is unavailable.');
  const status = value as IntegrationStatus;
  if (
    !['sandbox', 'stripe'].includes(status.payments) ||
    !['sandbox', 'sumsub'].includes(status.identity) ||
    status.content_policy !== 'general' ||
    !status.configured ||
    ['payments', 'identity', 'storage', 'scanner'].some(
      (key) => typeof status.configured[key as keyof IntegrationStatus['configured']] !== 'boolean',
    )
  )
    throw new Error('Provider status is unavailable.');
  return status;
}

export type HostedDestination = 'checkout' | 'payout-account' | 'verification';
const hosts: Record<HostedDestination, string> = {
  checkout: 'checkout.stripe.com',
  'payout-account': 'connect.stripe.com',
  verification: 'inquiry.sumsub.com',
};

export function hostedUrl(value: unknown, destination: HostedDestination): string {
  if (typeof value !== 'string' || value.length > 8192 || /[\s\u0000-\u001f\u007f\\]/.test(value))
    throw new Error('The provider returned an invalid link. Please try again.');
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error('The provider returned an invalid link. Please try again.');
  }
  if (
    url.protocol !== 'https:' ||
    url.hostname !== hosts[destination] ||
    url.port ||
    url.username ||
    url.password ||
    (destination !== 'checkout' && url.hash)
  )
    throw new Error('The provider returned an invalid link. Please try again.');
  // Stripe's official hosted Checkout Session URLs can contain a fragment.
  return url.href;
}

export interface CheckoutSession {
  id: string;
  status: string;
  amount_minor: number;
  currency: string;
  mode: 'sandbox' | 'stripe';
}
export function parseCheckout(value: unknown, expectedMode: 'sandbox' | 'stripe') {
  const result = value as {
    checkout?: CheckoutSession;
    confirmation_required?: boolean;
    redirect_url?: unknown;
  } | null;
  const session = result?.checkout;
  if (
    !session ||
    !/^[a-f0-9-]{36}$/.test(session.id) ||
    session.mode !== expectedMode ||
    session.status !== 'pending' ||
    !Number.isSafeInteger(session.amount_minor) ||
    session.amount_minor < 0 ||
    !/^[A-Z]{3}$/.test(session.currency)
  )
    throw new Error('The checkout response could not be verified. Please try again.');
  if (expectedMode === 'sandbox') {
    if (result?.confirmation_required !== true || result.redirect_url !== undefined)
      throw new Error('The checkout response could not be verified. Please try again.');
    return { checkout: session, redirectUrl: null };
  }
  if (result?.confirmation_required !== false)
    throw new Error('The checkout response could not be verified. Please try again.');
  return { checkout: session, redirectUrl: hostedUrl(result.redirect_url, 'checkout') };
}
