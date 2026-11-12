\set ON_ERROR_STOP on
-- Only runs during initial database creation, as the bootstrap administrator.
-- Read mounted files inside PostgreSQL; passwords never appear in process arguments.
DO $$
DECLARE
 owner_password text := rtrim(pg_read_file('/run/secrets/owner_password', 0, 128), E'\r\n');
 runtime_password text := rtrim(pg_read_file('/run/secrets/runtime_password', 0, 128), E'\r\n');
BEGIN
 IF owner_password !~ '^[a-f0-9]{64}$' OR runtime_password !~ '^[a-f0-9]{64}$' THEN
  RAISE EXCEPTION 'Database password files must contain 32 random bytes encoded as hex';
 END IF;
 EXECUTE format('CREATE ROLE myfans_owner LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L', owner_password);
 CREATE ROLE myfans_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION;
 EXECUTE format('CREATE ROLE myfans_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L', runtime_password);
 GRANT myfans_runtime TO myfans_app;
END $$;
ALTER DATABASE myfans OWNER TO myfans_owner;
REVOKE ALL ON DATABASE myfans FROM PUBLIC;
GRANT CONNECT ON DATABASE myfans TO myfans_app;
ALTER SCHEMA public OWNER TO myfans_owner;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
