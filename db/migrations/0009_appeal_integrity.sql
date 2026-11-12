CREATE FUNCTION protect_appeal_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Appeal history cannot be deleted'; END IF;
 IF ROW(NEW.id,NEW.user_id,NEW.post_id,NEW.reason,NEW.created_at) IS DISTINCT FROM ROW(OLD.id,OLD.user_id,OLD.post_id,OLD.reason,OLD.created_at) THEN
  RAISE EXCEPTION 'Appeal identity and evidence are immutable';
 END IF;
 IF OLD.status<>'pending' THEN RAISE EXCEPTION 'Reviewed appeals cannot be rewritten'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER appeals_protect_history BEFORE UPDATE OR DELETE ON moderation_appeals FOR EACH ROW EXECUTE FUNCTION protect_appeal_history();
