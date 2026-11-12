import { isCreator, type FeedCreator } from './contracts.ts';

export const policyKinds = ['terms', 'privacy', 'content', 'refunds'] as const;
export type PolicyKind = (typeof policyKinds)[number];
export interface PolicyDocument {
  kind: PolicyKind;
  title: string;
  version: string;
  updated_at: string;
  sections: { heading: string; body: string }[];
}
export interface PublicSite {
  mode: 'sandbox' | 'production';
  ready: boolean;
  operator: {
    legal_name: string | null;
    address: string | null;
    support_email: string | null;
    privacy_email: string | null;
  };
  policies: PolicyDocument[];
  eligibility: {
    minimum_age: number;
    countries: { code: string; name: string }[];
    registration_enabled: boolean;
  };
}
const record = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === 'object' && !Array.isArray(v);
const text = (v: unknown, max = 20000): v is string =>
  typeof v === 'string' && !!v.trim() && v.length <= max;
const email = (v: unknown) =>
  v === null || (text(v, 254) && /^[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+$/.test(v));
export function parsePublicSite(value: unknown): PublicSite {
  if (
    !record(value) ||
    !['sandbox', 'production'].includes(String(value.mode)) ||
    typeof value.ready !== 'boolean' ||
    !record(value.operator) ||
    !record(value.eligibility) ||
    !Array.isArray(value.policies)
  )
    throw new Error('Site information is unavailable.');
  const operator = value.operator,
    eligibility = value.eligibility;
  if (
    !['legal_name', 'address'].every((k) => operator[k] === null || text(operator[k], 4000)) ||
    !email(operator.support_email) ||
    !email(operator.privacy_email) ||
    !Number.isInteger(eligibility.minimum_age) ||
    Number(eligibility.minimum_age) < 18 ||
    Number(eligibility.minimum_age) > 120 ||
    typeof eligibility.registration_enabled !== 'boolean' ||
    !Array.isArray(eligibility.countries) ||
    !eligibility.countries.every(
      (c) =>
        record(c) && typeof c.code === 'string' && /^[A-Z]{2}$/.test(c.code) && text(c.name, 100),
    ) ||
    new Set(eligibility.countries.map((c) => c.code)).size !== eligibility.countries.length ||
    !value.policies.every(
      (p) =>
        record(p) &&
        policyKinds.includes(p.kind as PolicyKind) &&
        text(p.title, 160) &&
        text(p.version, 100) &&
        text(p.updated_at, 64) &&
        Number.isFinite(Date.parse(p.updated_at)) &&
        Array.isArray(p.sections) &&
        p.sections.length > 0 &&
        p.sections.length <= 100 &&
        p.sections.every((s) => record(s) && text(s.heading, 160) && text(s.body)),
    ) ||
    new Set(value.policies.map((p) => p.kind)).size !== value.policies.length
  )
    throw new Error('Site information is unavailable.');
  return value as unknown as PublicSite;
}
export function registrationAvailable(site: PublicSite | null): boolean {
  return (
    !!site &&
    site.ready &&
    site.eligibility.registration_enabled &&
    site.eligibility.countries.length > 0 &&
    policyKinds.every((kind) => site.policies.some((p) => p.kind === kind)) &&
    (site.mode === 'sandbox' || Object.values(site.operator).every((value) => text(value, 4000)))
  );
}
export function registrationConsent(
  site: PublicSite | null,
  countryCode: string,
  confirmed: boolean,
) {
  if (
    !registrationAvailable(site) ||
    !confirmed ||
    !site!.eligibility.countries.some((c) => c.code === countryCode)
  )
    throw new Error(
      'Confirm eligibility and review the current policies before creating an account.',
    );
  const version = (kind: PolicyKind) => site!.policies.find((p) => p.kind === kind)!.version;
  return {
    adult_confirmed: true,
    country_code: countryCode,
    terms_version: version('terms'),
    privacy_version: version('privacy'),
    content_policy_version: version('content'),
  };
}
export interface CreatorDirectory {
  creators: FeedCreator[];
  next_cursor: string | null;
  categories: string[];
}
export function parseCreatorDirectory(value: unknown): CreatorDirectory {
  if (
    !record(value) ||
    !Array.isArray(value.creators) ||
    value.creators.length > 50 ||
    !value.creators.every(isCreator) ||
    !Array.isArray(value.categories) ||
    !value.categories.every((c) => text(c, 100)) ||
    new Set(value.categories).size !== value.categories.length ||
    !(value.next_cursor === null || text(value.next_cursor, 90)) ||
    new Set(value.creators.map((c) => c.id)).size !== value.creators.length
  )
    throw new Error('The creator directory is unavailable.');
  return value as unknown as CreatorDirectory;
}
