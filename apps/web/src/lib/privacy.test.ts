import { describe, expect, it } from 'vitest';
import { parsePrivacyQueue, privacyEvidence, privacyReceipt } from './privacy.ts';
const id = 'aabbccdd-1234-5678-9000-aabbccddeeff';
const request = {
  id,
  user_id: id,
  display_name: 'Private account',
  status: 'application_erased',
  created_at: '2026-10-07T00:00:00Z',
  not_before: '2026-10-08T00:00:00Z',
  legal_hold: false,
  retention: {
    erasure_delay_days: 1,
    backup_days: 7,
    security_days: 30,
    financial_days: 365,
    policy_evidence_days: 100,
    moderation_evidence_days: 100,
  },
  application_erased_at: '2026-10-08T00:00:00Z',
  completed_at: null,
  versions_erased_at: null,
  has_obligations: false,
  objects_pending: 0,
};
describe('privacy request evidence', () => {
  it('keeps application erasure separate from external completion', () => {
    expect(
      parsePrivacyQueue({ requests: [request], next_cursor: null }).requests[0].completed_at,
    ).toBeNull();
    expect(privacyReceipt({ request: { id, status: 'pending' } })).toEqual({
      id,
      status: 'pending',
    });
    expect(() => privacyReceipt({ request: { id, status: 'completed' } })).toThrow();
  });
  it('rejects unsafe references, ambiguous queue rows and missing retention evidence', () => {
    expect(privacyEvidence('ticket:PRIVACY-1234')).toBe(true);
    expect(privacyEvidence('customer@example.com')).toBe(false);
    expect(privacyEvidence('notes about user identity')).toBe(false);
    expect(() => parsePrivacyQueue({ requests: [request, request], next_cursor: null })).toThrow();
    expect(() =>
      parsePrivacyQueue({ requests: [{ ...request, retention: {} }], next_cursor: null }),
    ).toThrow();
  });
});
