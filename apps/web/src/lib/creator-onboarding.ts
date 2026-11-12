export interface CreatorOnboarding {
  viewer_id: string;
  email_verified: boolean;
  mfa_enabled: boolean;
  mfa_recovery_required: boolean;
  identity: {
    status: 'not_started' | 'pending' | 'verified' | 'rejected';
    checked_at: string | null;
  };
  payments: {
    connected: boolean;
    charges_enabled: boolean;
    payouts_enabled: boolean;
    checked_at: string | null;
  };
  review: {
    status: 'pending' | 'approved' | 'rejected';
    reason: string | null;
    reviewed_at: string | null;
    resubmission_available: boolean;
  };
}
