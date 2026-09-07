-- ── Server-owned auth + blog schema (skb-server only; applied after
-- 001_init.surql via KnowledgeBase::query_surql, idempotent) ──

DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS email ON user TYPE string;
DEFINE FIELD IF NOT EXISTS password_hash ON user TYPE string;
DEFINE FIELD IF NOT EXISTS role ON user TYPE string
    ASSERT $value IN ["reader", "author"];
DEFINE FIELD IF NOT EXISTS created_at ON user TYPE datetime DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS user_email_unique ON user FIELDS email UNIQUE;

DEFINE TABLE IF NOT EXISTS blog_post SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS document  ON blog_post TYPE record<document>;
DEFINE FIELD IF NOT EXISTS author    ON blog_post TYPE record<user>;
DEFINE FIELD IF NOT EXISTS published ON blog_post TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS created_at ON blog_post TYPE datetime DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS blog_post_document_unique ON blog_post FIELDS document UNIQUE;

-- Logout revocation list (CWE-613): the jti of every logged-out session so
-- the JWT's remaining 24h lifetime is worthless even if the cookie was
-- stolen. Non-unique on purpose — re-logging-out the same token must not
-- error. Rows whose `exp` has passed are dead weight; the logout handler
-- purges them.
DEFINE TABLE IF NOT EXISTS revoked_session SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS jti ON revoked_session TYPE string;
DEFINE FIELD IF NOT EXISTS exp ON revoked_session TYPE int;
DEFINE INDEX IF NOT EXISTS revoked_session_jti ON revoked_session FIELDS jti;
