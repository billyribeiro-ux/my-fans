# Operator policy draft

[operator-policy.example.json](../deploy/operator-policy.example.json) contains complete general Terms, Privacy, Content and Refund policy drafts for MyFans. The draft covers the operator, creators who sell content, and subscribing or purchasing members. It matches the implemented general-content product: explicit adult content is prohibited, accounts are 18+, the initial eligible country is the United States, payments use Stripe Connect, and creators use Sumsub verification.

This is a general drafting baseline, not a determination of the operator's legal obligations. Have qualified counsel review it for the actual business and jurisdictions before publication. The US eligibility choice does not establish the operator's place of incorporation, governing law, tax status or provider approval. No corporate identity, address, designated legal agent or active contact inbox has been invented.

## What is supplied

- Terms: account eligibility and security, creator duties, 20% platform fee, monthly renewal and cancellation, content rights, enforcement, closure and preserved consumer rights.
- Privacy: actual information and providers, public/private audiences, essential storage, security and processing locations, access/export requests, erasure and the limits of retention and backup completion.
- Content: general-content boundaries, protection of minors, harassment/privacy, fraudulent transactions, rights and disclosures, upload safety, reports and appeals.
- Refunds: cancellation versus refunds, non-delivery and billing complaints, request evidence, provider processing, entitlement changes, disputes and creator adjustments.

These policies do not replace Stripe's or Sumsub's agreements, corporate formation records, provider contracts, or jurisdiction-specific notices required for the actual operator. Material policy-change notification and any necessary existing-user consent are operator-managed; the application captures current versions when a new account registers.

## Information still needed

Replace the four fields under `operator`: legal name, registered or service address, support email and privacy email. The reserved `replace.example` contacts deliberately fail the application's real-contact validation. Do not substitute an invented entity or an inbox you do not control.

Review the eligible countries and proposed retention periods for the actual business. Assign each document an approved version, such as `2026-10-08-1`, and its actual publication date when adopting it. Keep `registration_enabled` false until the account acceptance steps in [production activation](production-readiness.md) are complete; enabling registration is a separate configuration change.

## Proposed retention values

These values are operator choices for review, not claims that the law mandates a universal period. They are already present in the JSON so no retention field is empty.

| Field                      | Draft days | Meaning                                                                                                                                                                         |
| -------------------------- | ---------: | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `erasure_delay_days`       |          0 | No extra fixed delay; review, holds and obligations still govern processing.                                                                                                    |
| `backup_days`              |         35 | Minimum external-completion waiting window after application erasure, covering the configured RDS recovery window. Manual snapshots and provider evidence need separate review. |
| `security_days`            |         90 | Eligible personal security evidence after application erasure, also subject to the record's age.                                                                                |
| `financial_days`           |       2555 | Eligible personal financial review narratives; monetary journals, receipts and reconciliation identifiers remain.                                                               |
| `policy_evidence_days`     |       1095 | Eligible policy/consent evidence and privacy-review narratives, subject to the relevant completion checks.                                                                      |
| `moderation_evidence_days` |        365 | Eligible moderation evidence and identity-provider links.                                                                                                                       |

The exact enforcement and owner maintenance procedures are in [privacy operations](privacy-operations.md). The JSON schedule and privacy prose must stay consistent when edited. Do not present these application evidence periods as automatic deletion deadlines for all cloud logs, backups, provider records or financial journals. Existing erasure requests retain their captured schedule and policy version.

## Use the document

Copy the draft to a private local configuration file, replace the actual operator facts, and review the choices above. The application accepts this document through `OPERATOR_POLICY_FILE`. The AWS runtime injects the same document from the account's `policySecretArn`; see [AWS runtime](aws-runtime.md#supply-account-information). It is loaded at task launch and does not require a frontend rebuild. Updating the secret alone leaves existing tasks on the old document. After securely publishing an approved change, run `node scripts/deploy-aws.mjs reload-policy .local/aws-production.json .local/operator-policy.approved.json`; this replaces both services and verifies the complete public document and registration flag. Follow the [maintenance-window alternative](aws-runtime.md#updates-and-secret-changes) when old/new versions cannot overlap. Keep its UTF-8 payload within the AWS secret's 64 KiB limit.

The existing `/legal` route and registration flow consume that configuration. The unchanged local preview continues to identify itself as a development sandbox; this draft does not silently change the running deployment, its accepted versions, or the previously verified release archives. The earlier release evidence remains evidence for its recorded source snapshot; this policy revision is a subsequent configuration/documentation change.

## Basis checked on October 8, 2026

The policy text is original drafting based primarily on the implemented application behavior. The following sources informed the narrow external constraints; they do not approve this operator or certify legal compliance.

- Stripe lists content-creation platforms as restricted businesses and prohibits adult sexual content. Provider approval remains account-specific. [Stripe restricted businesses](https://stripe.com/legal/restricted-businesses), [Stripe's platform and adult-content FAQ](https://support.stripe.com/questions/prohibited-and-restricted-business-list-faqs).
- The FTC recommends limiting collection and retention to legitimate needs and documenting how retained information is protected and disposed of. This supports a reviewed retention schedule, not any particular number of days. [FTC guide for businesses](https://www.ftc.gov/business-guidance/resources/protecting-personal-information-guide-business).
- The application-specific payment, privacy and recovery boundaries are described in [provider setup](provider-setup.md), [finance](finance.md), [privacy operations](privacy-operations.md) and [privacy recovery](privacy-recovery.md).
