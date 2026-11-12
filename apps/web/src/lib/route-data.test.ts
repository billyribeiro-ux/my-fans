import { describe, expect, it } from 'vitest';
import {
  parseAccountData,
  parseStudioData,
  parseEarningsData,
  parseAdminReviews,
  parseConversationsData,
  parseNotificationsData,
} from './route-data.ts';

const id = '10000000-0000-4000-8000-000000000001';
const other = '10000000-0000-4000-8000-000000000002';
const at = '2026-10-08T12:00:00+00:00';
const cursor = `${at}|${id}`;
const user = {
  id,
  email: 'owner@example.test',
  display_name: 'Owner',
  avatar_url: '',
  role: 'creator',
  email_verified: true,
  mfa_enabled: true,
  mfa_recovery_required: false,
};
const creator = {
  id: other,
  handle: 'artist',
  display_name: 'Artist',
  bio: '',
  category: 'Art',
  subscription_price_minor: 1000,
  verification_status: 'pending',
  provider_secret: 'not-for-pages',
};
const onboarding = {
  viewer_id: id,
  email_verified: true,
  mfa_enabled: true,
  mfa_recovery_required: false,
  identity: { status: 'not_started', checked_at: null },
  payments: { connected: false, charges_enabled: false, payouts_enabled: false, checked_at: null },
  review: { status: 'pending', reason: null, reviewed_at: null, resubmission_available: false },
};
const payment = {
  id,
  checkout_id: other,
  creator_id: other,
  creator_name: 'Artist',
  kind: 'subscription',
  status: 'partially_refunded',
  amount_minor: 1000,
  refunded_minor: 100,
  currency: 'USD',
  created_at: at,
  period_start: at,
  period_end: null,
  provider: 'stripe',
  dispute_recorded: true,
};
const account = {
  user,
  creator,
  onboarding,
  subscriptions: [
    {
      id,
      creator_id: other,
      display_name: 'Artist',
      status: 'past_due',
      current_period_end: null,
      cancel_at_period_end: true,
      created_at: at,
    },
  ],
  payments: [payment],
  recovery_codes_remaining: 8,
  mail_transport: 'smtp',
  mode: 'production',
  next_cursors: { subscriptions: cursor, payments: null },
};
const studio = {
  creator,
  onboarding,
  posts: [
    {
      id,
      caption: 'Draft',
      state: 'draft',
      visibility: 'public',
      price_minor: null,
      scheduled_at: null,
      media_id: null,
      created_at: at,
    },
  ],
  media: [
    {
      id: other,
      state: 'processing',
      preview_url: '/api/platform/media/' + other + '/preview',
      created_at: at,
      content_type: 'video/mp4',
      captions_ready: false,
      hls_url: null,
      captions_url: null,
      rejection_reason: null,
    },
  ],
  next_cursors: { posts: null, media: null },
};
const earnings = {
  available_minor: -100,
  pending_minor: 100,
  reserved_minor: 0,
  total_minor: 0,
  currency: 'USD',
  mode: 'stripe',
  transactions: [
    {
      id,
      transaction_id: other,
      kind: 'creator_available',
      amount_minor: -100,
      created_at: at,
      external_key: 'private-provider-reference',
    },
  ],
  payouts: [{ id: other, amount_minor: 100, status: 'paid', created_at: at }],
  next_cursors: { transactions: cursor, payouts: null },
};
const reviews = {
  creators: [{ ...creator, created_at: at, review_context: null }],
  media: [
    {
      id,
      display_name: 'Artist',
      state: 'quarantine',
      content_type: 'video/mp4',
      created_at: at,
      preview_url: '/preview',
      full_url: '/full',
    },
  ],
  reports: [{ id, post_id: other, reason: 'Review this', created_at: at }],
  payouts: [
    {
      id,
      display_name: 'Artist',
      amount_minor: 100,
      currency: 'USD',
      status: 'pending',
      created_at: at,
    },
  ],
  checkouts: [
    {
      id,
      buyer: 'Fan',
      creator: 'Artist',
      kind: 'tip',
      amount_minor: 100,
      currency: 'USD',
      status: 'paid',
      paid_at: null,
      provider: 'stripe',
    },
  ],
  next_cursors: { creators: null, media: null, reports: cursor, payouts: null, checkouts: null },
  mode: 'production',
};
const conversations = {
  next_cursor: null,
  conversations: [
    {
      id,
      recipient_id: other,
      display_name: 'Artist',
      last_message: null,
      updated_at: at,
      blocked: false,
      own_blocked: false,
    },
  ],
};
const notifications = {
  viewer_id: id,
  next_cursor: cursor,
  notifications: [{ id, title: 'Update', body: 'Ready', created_at: at, read_at: null }],
};

describe('validated server route DTOs', () => {
  it('preserves real nullable fields and financial records while projecting private extras out', () => {
    const decoded = parseAccountData(account);
    expect(decoded.user?.mfa_recovery_required).toBe(false);
    expect(decoded.onboarding).toEqual(onboarding);
    expect(decoded.subscriptions[0].current_period_end).toBeNull();
    expect(decoded.payments[0]).toEqual(payment);
    expect(decoded.creator).not.toHaveProperty('provider_secret');
    expect(parseStudioData(studio).media[0].hls_url).toBeNull();
    expect(parseEarningsData(earnings).transactions[0].amount_minor).toBe(-100);
    expect(parseEarningsData(earnings).transactions[0]).not.toHaveProperty('external_key');
    expect(parseAdminReviews(reviews).checkouts[0].paid_at).toBeNull();
    expect(parseAdminReviews(reviews).media[0]).not.toHaveProperty('captions_ready');
    expect(parseConversationsData(conversations)).toEqual(conversations);
    expect(parseNotificationsData(notifications)).toEqual(notifications);
  });
  it('accepts the anonymous account and a member with no creator profile', () => {
    const empty = {
      ...account,
      user: null,
      creator: null,
      onboarding: null,
      subscriptions: [],
      payments: [],
      recovery_codes_remaining: 0,
      next_cursors: { subscriptions: null, payments: null },
    };
    expect(parseAccountData(empty).user).toBeNull();
    expect(
      parseStudioData({ ...studio, creator: null, onboarding: null, posts: [], media: [] }).creator,
    ).toBeNull();
  });
  it.each([
    ['account', parseAccountData, account],
    ['studio', parseStudioData, studio],
    ['earnings', parseEarningsData, earnings],
    ['reviews', parseAdminReviews, reviews],
    ['conversations', parseConversationsData, conversations],
    ['notifications', parseNotificationsData, notifications],
  ] as const)('rejects missing or incorrectly typed %s envelopes', (_, parse, valid) => {
    expect(() => parse(null)).toThrow();
    expect(() => parse([])).toThrow();
    expect(() => parse({})).toThrow();
    expect(() => parse({ ...valid, next_cursors: null, next_cursor: 123 })).toThrow();
  });
  it('rejects invalid assurance/ownership, monetary values and fabricated payment states', () => {
    expect(() => parseAccountData({ ...account, user: { ...user, mfa_enabled: 'yes' } })).toThrow();
    expect(() =>
      parseAccountData({ ...account, onboarding: { ...onboarding, viewer_id: other } }),
    ).toThrow();
    for (const changed of [
      { amount_minor: 1.5 },
      { amount_minor: Number.MAX_SAFE_INTEGER + 1 },
      { refunded_minor: 1001 },
      { status: 'settled' },
      { dispute_recorded: 1 },
      { created_at: 'invalid' },
    ]) {
      expect(() =>
        parseAccountData({ ...account, payments: [{ ...payment, ...changed }] }),
      ).toThrow();
    }
    expect(() =>
      parseStudioData({ ...studio, media: [{ ...studio.media[0], captions_ready: 'false' }] }),
    ).toThrow();
    expect(() =>
      parseEarningsData({
        ...earnings,
        transactions: [{ ...earnings.transactions[0], amount_minor: '-100' }],
      }),
    ).toThrow();
    expect(() =>
      parseAdminReviews({
        ...reviews,
        checkouts: [{ ...reviews.checkouts[0], provider: 'unknown' }],
      }),
    ).toThrow();
  });
  it('rejects duplicate/oversized pages, malformed cursors, and malformed live records', () => {
    expect(() => parseAccountData({ ...account, payments: [payment, payment] })).toThrow();
    expect(() =>
      parseConversationsData({
        ...conversations,
        conversations: Array(51).fill(conversations.conversations[0]),
      }),
    ).toThrow();
    expect(() =>
      parseNotificationsData({ ...notifications, next_cursor: 'not-a-cursor' }),
    ).toThrow();
    expect(() =>
      parseConversationsData({
        ...conversations,
        conversations: [{ ...conversations.conversations[0], blocked: 0 }],
      }),
    ).toThrow();
    expect(() =>
      parseNotificationsData({
        ...notifications,
        notifications: [{ ...notifications.notifications[0], read_at: false }],
      }),
    ).toThrow();
  });
});
