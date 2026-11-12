export const privacyStatuses = [
  'pending',
  'approved',
  'purging',
  'application_erased',
  'completed',
] as const;
export type PrivacyStatus = (typeof privacyStatuses)[number];
export interface PrivacyRequest {
  id: string;
  user_id: string;
  display_name: string;
  status: PrivacyStatus;
  created_at: string;
  not_before: string;
  legal_hold: boolean;
  retention: Record<
    | 'erasure_delay_days'
    | 'backup_days'
    | 'security_days'
    | 'financial_days'
    | 'policy_evidence_days'
    | 'moderation_evidence_days',
    number
  >;
  application_erased_at: string | null;
  completed_at: string | null;
  versions_erased_at: string | null;
  has_obligations: boolean;
  objects_pending: number;
  latest_action?: null | {
    action: string;
    reason: string;
    evidence: Record<string, string>;
    created_at: string;
  };
}
export interface PrivacyQueue {
  requests: PrivacyRequest[];
  next_cursor: string | null;
}
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const record = (value: unknown): value is Record<string, any> =>
  !!value && typeof value === 'object' && !Array.isArray(value);
const date = (value: unknown) => typeof value === 'string' && Number.isFinite(Date.parse(value));
export function parsePrivacyQueue(value: unknown): PrivacyQueue {
  if (
    !record(value) ||
    !Array.isArray(value.requests) ||
    value.requests.length > 50 ||
    !(
      value.next_cursor === null ||
      (typeof value.next_cursor === 'string' && value.next_cursor.length <= 90)
    )
  )
    throw new Error('Privacy requests could not be verified. Refresh to try again.');
  const ids = new Set<string>();
  for (const row of value.requests) {
    if (
      !record(row) ||
      !uuid.test(row.id) ||
      !uuid.test(row.user_id) ||
      ids.has(row.id) ||
      typeof row.display_name !== 'string' ||
      !privacyStatuses.includes(row.status) ||
      !date(row.created_at) ||
      !date(row.not_before) ||
      !['application_erased_at', 'completed_at', 'versions_erased_at'].every(
        (key) => row[key] === null || date(row[key]),
      ) ||
      typeof row.legal_hold !== 'boolean' ||
      typeof row.has_obligations !== 'boolean' ||
      !Number.isSafeInteger(row.objects_pending) ||
      row.objects_pending < 0 ||
      !record(row.retention) ||
      ![
        'erasure_delay_days',
        'backup_days',
        'security_days',
        'financial_days',
        'policy_evidence_days',
        'moderation_evidence_days',
      ].every((key) => Number.isSafeInteger(row.retention[key]) && row.retention[key] >= 0)
    )
      throw new Error('Privacy requests could not be verified. Refresh to try again.');
    ids.add(row.id);
    if (
      row.latest_action !== undefined &&
      row.latest_action !== null &&
      (!record(row.latest_action) ||
        !['requested', 'approved', 'hold', 'release', 'completed'].includes(
          row.latest_action.action,
        ) ||
        typeof row.latest_action.reason !== 'string' ||
        !date(row.latest_action.created_at) ||
        !record(row.latest_action.evidence))
    )
      throw new Error('Privacy review history could not be verified. Refresh to try again.');
  }
  return value as PrivacyQueue;
}
export function privacyEvidence(value: string): boolean {
  return /^[A-Za-z0-9_.:/-]{8,200}$/.test(value);
}
export function privacyReceipt(value: unknown): { id: string; status: 'pending' } {
  if (
    !record(value) ||
    !record(value.request) ||
    !uuid.test(value.request.id) ||
    value.request.status !== 'pending'
  )
    throw new Error(
      'Request receipt could not be verified. Contact support if your account has been disabled.',
    );
  return { id: value.request.id, status: 'pending' };
}
