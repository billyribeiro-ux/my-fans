export const financeCollections = [
  'accounts',
  'issues',
  'checkouts',
  'disputes',
  'payouts',
] as const;
export type FinanceCollection = (typeof financeCollections)[number];
export interface FinanceAccount {
  id: string;
  account_id: string;
  kind: string;
  last_completed_at: string | null;
  next_at: string | null;
}
export interface FinanceIssue {
  id: string;
  account_id: string;
  object_id: string;
  code: string;
  first_seen_at: string;
  last_seen_at: string;
  acknowledged_at: string | null;
  resolved_at: null;
}
export interface FinanceCheckout {
  id: string;
  provider_account: string;
  provider_session: string | null;
  created_at: string;
  status: 'pending';
}
export interface FinanceDispute {
  id: string;
  account_id: string;
  dispute_id: string;
  charge_id: string;
  status: string;
  amount_minor: number;
  currency: string;
  due_by: string | null;
  checked_at: string | null;
  created_at: string;
}
export interface FinancePayout {
  id: string;
  account_id: string;
  payout_id: string;
  status: string;
  amount_minor: number;
  currency: string;
  arrival_at: string | null;
  checked_at: string | null;
  created_at: string;
  automatic: boolean;
  reconciliation_status: 'completed' | 'in_progress' | 'not_applicable';
  members_complete: boolean;
}
export interface AdminFinance {
  mode: 'sandbox' | 'stripe';
  accounts: FinanceAccount[];
  issues: FinanceIssue[];
  checkouts: FinanceCheckout[];
  disputes: FinanceDispute[];
  payouts: FinancePayout[];
  scanner: {
    open_issues: number;
    failed_jobs: number;
    unverified_receipts: number;
    next_at: string | null;
  };
  next_cursors: Record<FinanceCollection, string | null>;
}
const record = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === 'object' && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 2000;
const date = (v: unknown) =>
  v === null || (typeof v === 'string' && Number.isFinite(Date.parse(v)));
const money = (v: Record<string, unknown>) =>
  Number.isSafeInteger(v.amount_minor) &&
  Number(v.amount_minor) >= 0 &&
  typeof v.currency === 'string' &&
  /^[A-Za-z]{3}$/.test(v.currency);
export function parseAdminFinance(value: unknown): AdminFinance {
  if (
    !record(value) ||
    !['sandbox', 'stripe'].includes(String(value.mode)) ||
    !record(value.scanner) ||
    !record(value.next_cursors)
  )
    throw new Error('Financial reconciliation data is unavailable.');
  for (const key of financeCollections) {
    const items = value[key];
    if (
      !Array.isArray(items) ||
      items.length > 50 ||
      !items.every((v) => record(v) && text(v.id)) ||
      new Set(items.map((v) => v.id)).size !== items.length ||
      !(
        value.next_cursors[key] === null ||
        (text(value.next_cursors[key]) && value.next_cursors[key].length <= 90)
      )
    )
      throw new Error('Financial reconciliation data is unavailable.');
  }
  const scanner = value.scanner;
  if (
    !['open_issues', 'failed_jobs', 'unverified_receipts'].every(
      (k) => Number.isSafeInteger(scanner[k]) && Number(scanner[k]) >= 0,
    ) ||
    !date(scanner.next_at)
  )
    throw new Error('Financial reconciliation data is unavailable.');
  const data = value as unknown as AdminFinance;
  if (
    !data.accounts.every(
      (v) => text(v.account_id) && text(v.kind) && date(v.last_completed_at) && date(v.next_at),
    ) ||
    !data.issues.every(
      (v) =>
        text(v.account_id) &&
        text(v.object_id) &&
        text(v.code) &&
        date(v.first_seen_at) &&
        date(v.last_seen_at) &&
        date(v.acknowledged_at) &&
        v.resolved_at === null,
    ) ||
    !data.checkouts.every(
      (v) =>
        text(v.provider_account) &&
        (v.provider_session === null || text(v.provider_session)) &&
        date(v.created_at) &&
        v.status === 'pending',
    ) ||
    !data.disputes.every(
      (v) =>
        text(v.account_id) &&
        text(v.dispute_id) &&
        text(v.charge_id) &&
        text(v.status) &&
        money(v as unknown as Record<string, unknown>) &&
        date(v.due_by) &&
        date(v.checked_at) &&
        date(v.created_at),
    ) ||
    !data.payouts.every(
      (v) =>
        text(v.account_id) &&
        text(v.payout_id) &&
        text(v.status) &&
        money(v as unknown as Record<string, unknown>) &&
        date(v.arrival_at) &&
        date(v.checked_at) &&
        date(v.created_at) &&
        typeof v.automatic === 'boolean' &&
        typeof v.members_complete === 'boolean' &&
        ['completed', 'in_progress', 'not_applicable'].includes(v.reconciliation_status),
    )
  )
    throw new Error('Financial reconciliation data is unavailable.');
  return data;
}
