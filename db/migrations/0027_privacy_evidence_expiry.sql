-- Expire duplicate personal audit narratives alongside their primary evidence.
-- Preserve privacy decisions, timestamps, holds and external reference receipts.
CREATE OR REPLACE FUNCTION reject_policy_evidence_change() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
 IF TG_OP='UPDATE' AND TG_TABLE_NAME='privacy_actions' THEN
  IF current_setting('myfans.retention_owner_batch',true)='on'
  AND current_user=pg_get_userbyid((SELECT relowner FROM pg_class WHERE oid=TG_RELID))
  AND NEW.reason='Expired personal review narrative'
  AND (to_jsonb(NEW)-'reason')=(to_jsonb(OLD)-'reason') THEN RETURN NEW; END IF;
 END IF;
 IF TG_OP='DELETE' AND TG_TABLE_NAME IN ('policy_acceptances','moderation_evidence')
 AND current_setting('myfans.retention_owner_batch',true)='on'
 AND current_user=pg_get_userbyid((SELECT relowner FROM pg_class WHERE oid=TG_RELID)) THEN
  RETURN OLD;
 END IF;
 RAISE EXCEPTION 'Policy acceptance evidence is immutable' USING ERRCODE='23514';
END $$;
CREATE OR REPLACE FUNCTION privacy_retention_batch(request uuid) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,public,pg_temp AS $$
DECLARE r public.privacy_requests%ROWTYPE;
 security_cutoff timestamptz; financial_cutoff timestamptz;
 policy_cutoff timestamptz; moderation_cutoff timestamptz;
 security_count integer:=0; financial_count integer:=0;
 policy_count integer:=0; moderation_count integer:=0; identity_count integer:=0;
 financial_audit_count integer:=0; moderation_audit_count integer:=0; privacy_narrative_count integer:=0;
 result jsonb;
BEGIN
 -- No runtime EXECUTE grant exists; also reject indirect calls through another
 -- privileged function. This maintenance uses a direct owner login only.
 IF session_user<>pg_get_userbyid((SELECT relowner FROM pg_class WHERE oid='public.privacy_requests'::regclass)) THEN
  RAISE EXCEPTION 'Retention requires the schema owner login' USING ERRCODE='42501';
 END IF;
 -- Serialize owner batches before acquiring request/counterparty hold locks.
 PERFORM pg_advisory_xact_lock(17341926,25);
 SELECT * INTO STRICT r FROM public.privacy_requests WHERE id=request FOR UPDATE;
 IF r.status NOT IN ('application_erased','completed') OR r.legal_hold
 OR public.privacy_has_obligations(r.user_id) THEN
  RAISE EXCEPTION 'Retention requires erased data without holds or financial obligations' USING ERRCODE='23514';
 END IF;
 -- Evidence can concern both parties. Do not expire shared evidence while a
 -- related checkout participant or reporter is on hold. Lock before checking,
 -- so a concurrent review cannot race evidence deletion.
 PERFORM 1 FROM public.privacy_requests p WHERE p.user_id IN (
  SELECT c.user_id FROM public.checkouts c JOIN public.creators cr ON cr.id=c.creator_id WHERE cr.user_id=r.user_id
  UNION SELECT cr.user_id FROM public.checkouts c JOIN public.creators cr ON cr.id=c.creator_id WHERE c.user_id=r.user_id
  UNION SELECT rp.user_id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE cr.user_id=r.user_id
  UNION SELECT cr.user_id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE rp.user_id=r.user_id
 ) ORDER BY p.id FOR UPDATE;
 IF EXISTS(SELECT 1 FROM public.privacy_requests p WHERE p.legal_hold AND p.user_id IN (
  SELECT c.user_id FROM public.checkouts c JOIN public.creators cr ON cr.id=c.creator_id WHERE cr.user_id=r.user_id
  UNION SELECT cr.user_id FROM public.checkouts c JOIN public.creators cr ON cr.id=c.creator_id WHERE c.user_id=r.user_id
  UNION SELECT rp.user_id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE cr.user_id=r.user_id
  UNION SELECT cr.user_id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE rp.user_id=r.user_id
 )) THEN
  RAISE EXCEPTION 'Related evidence is on legal hold' USING ERRCODE='23514';
 END IF;
 security_cutoff:=now()-make_interval(days=>(r.retention->>'security_days')::integer);
 financial_cutoff:=now()-make_interval(days=>(r.retention->>'financial_days')::integer);
 policy_cutoff:=now()-make_interval(days=>(r.retention->>'policy_evidence_days')::integer);
 moderation_cutoff:=now()-make_interval(days=>(r.retention->>'moderation_evidence_days')::integer);
 IF security_cutoff IS NULL OR financial_cutoff IS NULL OR policy_cutoff IS NULL OR moderation_cutoff IS NULL THEN
  RAISE EXCEPTION 'Captured retention schedule is incomplete' USING ERRCODE='23514';
 END IF;
 PERFORM set_config('myfans.retention_owner_batch','on',true);
 -- A period starts no earlier than application erasure, even for older rows.
 IF r.application_erased_at<=security_cutoff THEN
  DELETE FROM public.audit_events WHERE id IN (
   SELECT id FROM public.audit_events WHERE actor_id=r.user_id AND created_at<=security_cutoff
   AND action IN ('account.exported','account.deactivated','password_changed','password_reset',
    'email.verification.resent','mfa.enabled','mfa.recovery.rotated','mfa.recovery.used')
   ORDER BY created_at,id LIMIT 100);
  GET DIAGNOSTICS security_count=ROW_COUNT;
 END IF;
 IF r.application_erased_at<=policy_cutoff THEN
  DELETE FROM public.policy_acceptances WHERE id IN (
   SELECT id FROM public.policy_acceptances WHERE user_id=r.user_id AND created_at<=policy_cutoff
   ORDER BY created_at,id LIMIT 100);
  GET DIAGNOSTICS policy_count=ROW_COUNT;
  IF r.status='completed' THEN
   UPDATE public.privacy_actions SET reason='Expired personal review narrative' WHERE id IN (
    SELECT id FROM public.privacy_actions WHERE request_id=r.id AND created_at<=policy_cutoff
    AND reason<>'Expired personal review narrative' ORDER BY created_at,id LIMIT 100);
   GET DIAGNOSTICS privacy_narrative_count=ROW_COUNT;
  END IF;
 END IF;
 IF r.application_erased_at<=financial_cutoff THEN
  DELETE FROM public.provider_finance_actions a WHERE a.id IN (
   SELECT a.id FROM public.provider_finance_actions a WHERE a.created_at<=financial_cutoff AND (
    (a.action IN ('checkout.attach','refund.request') AND a.target IN (
      SELECT c.id::text FROM public.checkouts c JOIN public.creators cr ON cr.id=c.creator_id WHERE c.user_id=r.user_id OR cr.user_id=r.user_id))
    OR (a.action IN ('dispute.refresh','dispute.restore') AND a.target IN (
      SELECT d.dispute_id FROM public.provider_disputes d JOIN public.provider_receipts pr ON pr.account_id=d.account_id AND pr.charge_id=d.charge_id
      JOIN public.checkouts c ON c.id=pr.checkout_id JOIN public.creators cr ON cr.id=c.creator_id WHERE c.user_id=r.user_id OR cr.user_id=r.user_id))
    OR (a.action='issue.acknowledge' AND a.target IN (
      SELECT i.id::text FROM public.provider_finance_issues i JOIN public.provider_accounts pa ON pa.account_id=i.account_id WHERE pa.user_id=r.user_id AND i.resolved_at IS NOT NULL))
   ) ORDER BY a.created_at,a.id LIMIT 100);
  GET DIAGNOSTICS financial_count=ROW_COUNT;
  DELETE FROM public.audit_events WHERE id IN (
   SELECT a.id FROM public.audit_events a WHERE a.action='review.reason' AND a.resource_type='payout'
   AND a.created_at<=financial_cutoff AND a.resource_id IN(SELECT id FROM public.payouts WHERE user_id=r.user_id)
   ORDER BY a.created_at,a.id LIMIT 100);
  GET DIAGNOSTICS financial_audit_count=ROW_COUNT;
 END IF;
 IF r.application_erased_at<=moderation_cutoff THEN
  DELETE FROM public.moderation_evidence e WHERE e.id IN (
   SELECT e.id FROM public.moderation_evidence e WHERE e.created_at<=moderation_cutoff AND (
    (e.resource_type='user' AND e.resource_id=r.user_id)
    OR (e.resource_type='creator' AND e.resource_id IN(SELECT id FROM public.creators WHERE user_id=r.user_id))
    OR (e.resource_type='media' AND e.resource_id IN(SELECT m.id FROM public.media_assets m JOIN public.creators c ON c.id=m.creator_id WHERE c.user_id=r.user_id))
    OR (e.resource_type='post' AND e.resource_id IN(SELECT p.id FROM public.posts p JOIN public.creators c ON c.id=p.creator_id WHERE c.user_id=r.user_id))
    OR (e.resource_type='report' AND e.resource_id IN(SELECT rp.id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE rp.user_id=r.user_id OR cr.user_id=r.user_id))
    OR (e.resource_type='appeal' AND e.resource_id IN(SELECT id FROM public.moderation_appeals WHERE user_id=r.user_id))
   ) ORDER BY e.created_at,e.id LIMIT 100);
  GET DIAGNOSTICS moderation_count=ROW_COUNT;
  DELETE FROM public.audit_events a WHERE a.id IN (
   SELECT a.id FROM public.audit_events a WHERE a.created_at<=moderation_cutoff
   AND a.action IN ('user.suspend','user.restore','post.moderation.restored','review.reason','appeal.accepted','appeal.rejected')
   AND (
    (a.resource_type='user' AND a.resource_id=r.user_id)
    OR (a.resource_type='creator' AND a.resource_id IN(SELECT id FROM public.creators WHERE user_id=r.user_id))
    OR (a.resource_type='media' AND a.resource_id IN(SELECT m.id FROM public.media_assets m JOIN public.creators c ON c.id=m.creator_id WHERE c.user_id=r.user_id))
    OR (a.resource_type='post' AND a.resource_id IN(SELECT p.id FROM public.posts p JOIN public.creators c ON c.id=p.creator_id WHERE c.user_id=r.user_id))
    OR (a.resource_type='report' AND a.resource_id IN(SELECT rp.id FROM public.reports rp JOIN public.posts p ON p.id=rp.post_id JOIN public.creators cr ON cr.id=p.creator_id WHERE rp.user_id=r.user_id OR cr.user_id=r.user_id))
    OR (a.resource_type='appeal' AND a.resource_id IN(SELECT id FROM public.moderation_appeals WHERE user_id=r.user_id))
   ) ORDER BY a.created_at,a.id LIMIT 100);
  GET DIAGNOSTICS moderation_audit_count=ROW_COUNT;
  UPDATE public.identity_checks SET applicant_id=NULL WHERE applicant_id IS NOT NULL
   AND created_at<=moderation_cutoff AND creator_id IN(SELECT id FROM public.creators WHERE user_id=r.user_id);
  GET DIAGNOSTICS identity_count=ROW_COUNT;
 END IF;
 PERFORM set_config('myfans.retention_owner_batch','off',true);
 result:=jsonb_build_object('security',security_count,'financial_personal_evidence',financial_count,
  'policy_evidence',policy_count,'moderation_evidence',moderation_count,'identity_links',identity_count,
  'financial_audit_narratives',financial_audit_count,'moderation_audit_narratives',moderation_audit_count,
  'privacy_review_narratives',privacy_narrative_count);
 INSERT INTO public.privacy_retention_runs(request_id,counts) VALUES(request,result);
 RETURN result;
END $$;
