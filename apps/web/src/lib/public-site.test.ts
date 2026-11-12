import { describe, expect, it } from 'vitest';
import {
  policyKinds,
  parsePublicSite,
  registrationAvailable,
  registrationConsent,
  parseCreatorDirectory,
  type PublicSite,
} from './public-site.ts';

const site: PublicSite = {
  mode: 'sandbox',
  ready: true,
  operator: { legal_name: null, address: null, support_email: null, privacy_email: null },
  policies: policyKinds.map((kind) => ({
    kind,
    title: `Development ${kind}`,
    version: 'sandbox-1',
    updated_at: '2026-10-07T00:00:00Z',
    sections: [{ heading: 'Test fixture', body: 'Development test policy text only.' }],
  })),
  eligibility: {
    minimum_age: 18,
    registration_enabled: true,
    countries: [{ code: 'US', name: 'United States (sandbox fixture)' }],
  },
};
describe('public product boundaries', () => {
  it('preserves configured policy text and sends exact reviewed versions', () => {
    const configured = parsePublicSite(site);
    expect(configured.policies[0].sections[0].body).toBe(site.policies[0].sections[0].body);
    expect(registrationConsent(configured, 'US', true)).toEqual({
      adult_confirmed: true,
      country_code: 'US',
      terms_version: 'sandbox-1',
      privacy_version: 'sandbox-1',
      content_policy_version: 'sandbox-1',
    });
  });
  it('requires confirmation, eligible country, readiness, and all policies', () => {
    expect(() => registrationConsent(site, 'US', false)).toThrow();
    expect(() => registrationConsent(site, 'ZZ', true)).toThrow();
    expect(registrationAvailable({ ...site, ready: false })).toBe(false);
    expect(registrationAvailable({ ...site, policies: site.policies.slice(0, 3) })).toBe(false);
    expect(
      registrationAvailable({
        ...site,
        eligibility: { ...site.eligibility, registration_enabled: false },
      }),
    ).toBe(false);
  });
  it('does not silently permit production without actual operator details', () => {
    expect(registrationAvailable({ ...site, mode: 'production' })).toBe(false);
    expect(
      registrationAvailable({
        ...site,
        mode: 'production',
        operator: {
          legal_name: 'Configured test operator',
          address: 'Configured test address',
          support_email: 'support@example.test',
          privacy_email: 'privacy@example.test',
        },
      }),
    ).toBe(true);
  });
  it('rejects unsafe contact headers, underage thresholds, and duplicate documents/countries', () => {
    expect(() =>
      parsePublicSite({
        ...site,
        operator: {
          ...site.operator,
          support_email: 'user@example.test\r\nBcc:private@example.test',
        },
      }),
    ).toThrow();
    expect(() =>
      parsePublicSite({ ...site, eligibility: { ...site.eligibility, minimum_age: 17 } }),
    ).toThrow();
    expect(() =>
      parsePublicSite({ ...site, policies: [site.policies[0], site.policies[0]] }),
    ).toThrow();
    expect(() =>
      parsePublicSite({
        ...site,
        eligibility: {
          ...site.eligibility,
          countries: [site.eligibility.countries[0], site.eligibility.countries[0]],
        },
      }),
    ).toThrow();
  });
  it('keeps directory results bounded and validates cursor/category shape', () => {
    expect(
      parseCreatorDirectory({ creators: [], next_cursor: null, categories: ['Art'] }).categories,
    ).toEqual(['Art']);
    expect(() => parseCreatorDirectory({ creators: [], next_cursor: 5, categories: [] })).toThrow();
    expect(() =>
      parseCreatorDirectory({ creators: [], next_cursor: null, categories: ['Art', 'Art'] }),
    ).toThrow();
    expect(() =>
      parseCreatorDirectory({ creators: Array(51).fill({}), next_cursor: null, categories: [] }),
    ).toThrow();
  });
});
