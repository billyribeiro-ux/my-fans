import type { CreatorOnboarding } from './creator-onboarding.ts';
import type { Conversation, Notification } from './live-updates.ts';

// These projections follow the Rust account, studio, earnings and operations responses.
// Copy only validated UI fields: additional upstream columns must not silently reach SSR.
type Row = Record<string, unknown>;
function invalid(): never {
  throw new Error('Invalid route data.');
}
function record(value: unknown): Row {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return invalid();
  return value as Row;
}
function text(value: unknown): string {
  return typeof value === 'string' ? value : invalid();
}
function bool(value: unknown): boolean {
  return typeof value === 'boolean' ? value : invalid();
}
function integer(value: unknown): number {
  return typeof value === 'number' && Number.isSafeInteger(value) ? value : invalid();
}
function nonnegative(value: unknown): number {
  const parsed = integer(value);
  return parsed >= 0 ? parsed : invalid();
}
function uuid(value: unknown): string {
  const parsed = text(value);
  return /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(parsed)
    ? parsed
    : invalid();
}
function date(value: unknown): string {
  const parsed = text(value);
  return Number.isFinite(Date.parse(parsed)) ? parsed : invalid();
}
function oneOf<const T extends readonly string[]>(value: unknown, choices: T): T[number] {
  const parsed = text(value);
  return choices.includes(parsed) ? (parsed as T[number]) : invalid();
}
function nullable<T>(value: unknown, parse: (value: unknown) => T): T | null {
  return value === null ? null : parse(value);
}
function cursor(value: unknown): string | null {
  if (value === null) return null;
  const parsed = text(value);
  if (parsed.length > 90) return invalid();
  const parts = parsed.split('|');
  if (parts.length !== 2) return invalid();
  date(parts[0]);
  uuid(parts[1]);
  return parsed;
}
function rows<T extends { id: string }>(value: unknown, parse: (value: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > 50) return invalid();
  const parsed = value.map(parse);
  if (new Set(parsed.map((row) => row.id)).size !== parsed.length) return invalid();
  return parsed;
}
function currency(value: unknown): string {
  const parsed = text(value);
  return /^[A-Z]{3}$/.test(parsed) ? parsed : invalid();
}
const reviewStates = ['pending', 'approved', 'rejected'] as const;
const providers = ['sandbox', 'stripe'] as const;
function parseUser(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    email: text(row.email),
    display_name: text(row.display_name),
    avatar_url: text(row.avatar_url),
    role: oneOf(row.role, ['fan', 'creator', 'admin']),
    email_verified: bool(row.email_verified),
    mfa_enabled: bool(row.mfa_enabled),
    mfa_recovery_required: bool(row.mfa_recovery_required),
  };
}
function parseCreator(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    handle: text(row.handle),
    display_name: text(row.display_name),
    bio: text(row.bio),
    category: text(row.category),
    subscription_price_minor: nonnegative(row.subscription_price_minor),
    verification_status: oneOf(row.verification_status, reviewStates),
  };
}
function parseOnboarding(value: unknown): CreatorOnboarding {
  const row = record(value),
    identity = record(row.identity),
    payments = record(row.payments),
    review = record(row.review);
  return {
    viewer_id: uuid(row.viewer_id),
    email_verified: bool(row.email_verified),
    mfa_enabled: bool(row.mfa_enabled),
    mfa_recovery_required: bool(row.mfa_recovery_required),
    identity: {
      status: oneOf(identity.status, ['not_started', 'pending', 'verified', 'rejected']),
      checked_at: nullable(identity.checked_at, date),
    },
    payments: {
      connected: bool(payments.connected),
      charges_enabled: bool(payments.charges_enabled),
      payouts_enabled: bool(payments.payouts_enabled),
      checked_at: nullable(payments.checked_at, date),
    },
    review: {
      status: oneOf(review.status, reviewStates),
      reason: nullable(review.reason, text),
      reviewed_at: nullable(review.reviewed_at, date),
      resubmission_available: bool(review.resubmission_available),
    },
  };
}
function parseSubscription(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    creator_id: uuid(row.creator_id),
    display_name: text(row.display_name),
    status: text(row.status),
    current_period_end: nullable(row.current_period_end, date),
    cancel_at_period_end: bool(row.cancel_at_period_end),
    created_at: date(row.created_at),
  };
}
function parsePayment(value: unknown) {
  const row = record(value);
  const amount = nonnegative(row.amount_minor),
    refunded = nonnegative(row.refunded_minor);
  if (refunded > amount) return invalid();
  return {
    id: uuid(row.id),
    checkout_id: uuid(row.checkout_id),
    creator_id: uuid(row.creator_id),
    creator_name: text(row.creator_name),
    kind: oneOf(row.kind, ['subscription', 'purchase', 'tip']),
    status: oneOf(row.status, ['pending', 'paid', 'partially_refunded', 'refunded', 'failed']),
    amount_minor: amount,
    refunded_minor: refunded,
    currency: oneOf(row.currency, ['USD']),
    created_at: date(row.created_at),
    period_start: nullable(row.period_start, date),
    period_end: nullable(row.period_end, date),
    provider: oneOf(row.provider, providers),
    dispute_recorded: bool(row.dispute_recorded),
  };
}
export function parseAccountData(value: unknown) {
  const row = record(value),
    next = record(row.next_cursors);
  const user = nullable(row.user, parseUser),
    onboarding = nullable(row.onboarding, parseOnboarding);
  if (onboarding && onboarding.viewer_id !== user?.id) return invalid();
  return {
    user,
    creator: nullable(row.creator, parseCreator),
    onboarding,
    subscriptions: rows(row.subscriptions, parseSubscription),
    payments: rows(row.payments, parsePayment),
    recovery_codes_remaining: nonnegative(row.recovery_codes_remaining),
    mail_transport: oneOf(row.mail_transport, ['sandbox', 'smtp']),
    mode: oneOf(row.mode, ['sandbox', 'production']),
    next_cursors: { subscriptions: cursor(next.subscriptions), payments: cursor(next.payments) },
  };
}
function parseMedia(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    state: text(row.state),
    preview_url: text(row.preview_url),
    created_at: date(row.created_at),
    content_type: text(row.content_type),
    captions_ready: bool(row.captions_ready),
    hls_url: nullable(row.hls_url, text),
    captions_url: nullable(row.captions_url, text),
    rejection_reason: nullable(row.rejection_reason, text),
  };
}
function parsePost(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    caption: text(row.caption),
    state: text(row.state),
    visibility: oneOf(row.visibility, ['public', 'subscribers', 'purchase']),
    price_minor: nullable(row.price_minor, nonnegative),
    scheduled_at: nullable(row.scheduled_at, date),
    media_id: nullable(row.media_id, uuid),
    created_at: date(row.created_at),
  };
}
export function parseStudioData(value: unknown) {
  const row = record(value),
    next = record(row.next_cursors);
  return {
    creator: nullable(row.creator, parseCreator),
    onboarding: nullable(row.onboarding, parseOnboarding),
    posts: rows(row.posts, parsePost),
    media: rows(row.media, parseMedia),
    next_cursors: { posts: cursor(next.posts), media: cursor(next.media) },
  };
}
function parseTransaction(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    transaction_id: uuid(row.transaction_id),
    kind: text(row.kind),
    amount_minor: integer(row.amount_minor),
    created_at: date(row.created_at),
  };
}
function parsePayout(value: unknown) {
  const row = record(value);
  return {
    id: uuid(row.id),
    amount_minor: nonnegative(row.amount_minor),
    status: text(row.status),
    created_at: date(row.created_at),
  };
}
export function parseEarningsData(value: unknown) {
  const row = record(value),
    next = record(row.next_cursors);
  return {
    available_minor: integer(row.available_minor),
    pending_minor: integer(row.pending_minor),
    reserved_minor: integer(row.reserved_minor),
    total_minor: integer(row.total_minor),
    currency: currency(row.currency),
    mode: oneOf(row.mode, providers),
    transactions: rows(row.transactions, parseTransaction),
    payouts: rows(row.payouts, parsePayout),
    next_cursors: { transactions: cursor(next.transactions), payouts: cursor(next.payouts) },
  };
}

// Review queues have different columns. Their shared rendering contract is deliberately optional;
// each queue parser below still validates all fields actually returned for that queue.
export interface AdminReview {
  id: string;
  display_name?: string;
  handle?: string;
  state?: string;
  status?: string;
  verification_status?: string;
  reason?: string;
  post_id?: string;
  preview_url?: string;
  content_type?: string;
  full_url?: string;
  captions_ready?: boolean;
  amount_minor?: number;
  created_at?: string;
  buyer?: string;
  creator?: string;
  kind?: string;
  paid_at?: string | null;
  provider?: 'sandbox' | 'stripe';
  currency?: string;
  review_context?: { decision: string; reason: string; created_at: string } | null;
}
function reviewContext(value: unknown) {
  const row = record(value);
  return {
    decision: text(row.decision),
    reason: text(row.reason),
    created_at: date(row.created_at),
  };
}
export function parseAdminReviews(value: unknown) {
  const row = record(value),
    next = record(row.next_cursors);
  return {
    creators: rows(row.creators, (value): AdminReview => {
      const item = record(value);
      return {
        id: uuid(item.id),
        display_name: text(item.display_name),
        handle: text(item.handle),
        verification_status: oneOf(item.verification_status, reviewStates),
        created_at: date(item.created_at),
        review_context: nullable(item.review_context, reviewContext),
      };
    }),
    media: rows(row.media, (value): AdminReview => {
      const item = record(value);
      return {
        id: uuid(item.id),
        display_name: text(item.display_name),
        state: text(item.state),
        content_type: text(item.content_type),
        created_at: date(item.created_at),
        preview_url: text(item.preview_url),
        full_url: text(item.full_url),
        ...(item.captions_ready === undefined ? {} : { captions_ready: bool(item.captions_ready) }),
      };
    }),
    reports: rows(row.reports, (value): AdminReview => {
      const item = record(value);
      return {
        id: uuid(item.id),
        post_id: uuid(item.post_id),
        reason: text(item.reason),
        created_at: date(item.created_at),
      };
    }),
    payouts: rows(row.payouts, (value): AdminReview => {
      const item = record(value);
      return {
        ...parsePayout(item),
        display_name: text(item.display_name),
        currency: currency(item.currency),
      };
    }),
    checkouts: rows(row.checkouts, (value): AdminReview => {
      const item = record(value);
      return {
        id: uuid(item.id),
        buyer: text(item.buyer),
        creator: text(item.creator),
        kind: oneOf(item.kind, ['subscription', 'purchase', 'tip']),
        amount_minor: nonnegative(item.amount_minor),
        currency: currency(item.currency),
        status: text(item.status),
        paid_at: nullable(item.paid_at, date),
        provider: oneOf(item.provider, providers),
      };
    }),
    next_cursors: {
      creators: cursor(next.creators),
      media: cursor(next.media),
      reports: cursor(next.reports),
      payouts: cursor(next.payouts),
      checkouts: cursor(next.checkouts),
    },
    mode: oneOf(row.mode, ['sandbox', 'production']),
  };
}
export function parseConversationsData(value: unknown) {
  const row = record(value);
  return {
    next_cursor: cursor(row.next_cursor),
    conversations: rows(row.conversations, (value): Conversation => {
      const item = record(value);
      return {
        id: uuid(item.id),
        recipient_id: uuid(item.recipient_id),
        display_name: text(item.display_name),
        last_message: nullable(item.last_message, text),
        updated_at: date(item.updated_at),
        blocked: bool(item.blocked),
        own_blocked: bool(item.own_blocked),
      };
    }),
  };
}
export function parseNotificationsData(value: unknown) {
  const row = record(value);
  return {
    viewer_id: uuid(row.viewer_id),
    next_cursor: cursor(row.next_cursor),
    notifications: rows(row.notifications, (value): Notification => {
      const item = record(value);
      return {
        id: uuid(item.id),
        title: text(item.title),
        body: text(item.body),
        created_at: date(item.created_at),
        read_at: nullable(item.read_at, date),
      };
    }),
  };
}
export type AccountData = ReturnType<typeof parseAccountData>;
export type AccountUser = NonNullable<AccountData['user']>;
export type Subscription = AccountData['subscriptions'][number];
export type Payment = AccountData['payments'][number];
export type StudioData = ReturnType<typeof parseStudioData>;
export type StudioCreator = NonNullable<StudioData['creator']>;
export type StudioPost = StudioData['posts'][number];
export type StudioMedia = StudioData['media'][number];
export type EarningsData = ReturnType<typeof parseEarningsData>;
export type Transaction = EarningsData['transactions'][number];
export type Payout = EarningsData['payouts'][number];
export type AdminReviews = ReturnType<typeof parseAdminReviews>;
