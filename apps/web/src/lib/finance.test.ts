import { describe, expect, it } from 'vitest';
import { parseAdminFinance, financeCollections, type AdminFinance } from './finance.ts';
const fixture: AdminFinance = {
  mode: 'stripe',
  accounts: [],
  issues: [],
  checkouts: [],
  disputes: [],
  payouts: [],
  scanner: { open_issues: 0, failed_jobs: 0, unverified_receipts: 0, next_at: null },
  next_cursors: Object.fromEntries(
    financeCollections.map((key) => [key, null]),
  ) as AdminFinance['next_cursors'],
};
describe('administrator financial boundaries', () => {
  it('accepts explicit provider mode and empty reconciled queues', () => {
    expect(parseAdminFinance(fixture).mode).toBe('stripe');
  });
  it('does not treat acknowledgment as issue resolution', () => {
    const issue = {
      id: 'issue',
      account_id: 'acct_test',
      object_id: 'object',
      code: 'unverified_receipt',
      first_seen_at: '2026-10-07T00:00:00Z',
      last_seen_at: '2026-10-07T00:00:00Z',
      acknowledged_at: '2026-10-07T00:00:00Z',
      resolved_at: null,
    };
    expect(parseAdminFinance({ ...fixture, issues: [issue] }).issues[0].resolved_at).toBeNull();
    expect(() =>
      parseAdminFinance({
        ...fixture,
        issues: [{ ...issue, resolved_at: '2026-10-07T00:00:00Z' }],
      }),
    ).toThrow();
  });
  it('requires bounded queues, cursor metadata and nonnegative scanner counts', () => {
    expect(() => parseAdminFinance({ ...fixture, next_cursors: {} })).toThrow();
    expect(() =>
      parseAdminFinance({ ...fixture, scanner: { ...fixture.scanner, open_issues: -1 } }),
    ).toThrow();
    expect(() =>
      parseAdminFinance({ ...fixture, checkouts: Array(51).fill({ id: 'same' }) }),
    ).toThrow();
  });
  it('requires provider payout status and completed-membership flags explicitly', () => {
    const payout = {
      id: 'payout',
      account_id: 'acct_test',
      payout_id: 'po_test',
      status: 'paid',
      amount_minor: 100,
      currency: 'usd',
      arrival_at: '2026-10-07T00:00:00Z',
      checked_at: '2026-10-07T00:00:00Z',
      created_at: '2026-10-07T00:00:00Z',
      automatic: true,
      members_complete: false,
      reconciliation_status: 'in_progress',
    };
    expect(parseAdminFinance({ ...fixture, payouts: [payout] }).payouts[0].members_complete).toBe(
      false,
    );
    expect(() =>
      parseAdminFinance({ ...fixture, payouts: [{ ...payout, members_complete: undefined }] }),
    ).toThrow();
    expect(() =>
      parseAdminFinance({ ...fixture, payouts: [{ ...payout, amount_minor: -100 }] }),
    ).toThrow();
  });
});
