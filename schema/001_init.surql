-- ── Document DB 層 ──────────────────────────────
DEFINE TABLE IF NOT EXISTS document SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS title       ON document TYPE string;
DEFINE FIELD IF NOT EXISTS source      ON document TYPE string;
DEFINE FIELD IF NOT EXISTS source_type ON document TYPE string
    ASSERT $value IN ["file", "url", "text", "stdin"];
DEFINE FIELD IF NOT EXISTS mime        ON document TYPE option<string>;
DEFINE FIELD IF NOT EXISTS sha256      ON document TYPE string;
DEFINE FIELD IF NOT EXISTS content     ON document TYPE string;
DEFINE FIELD IF NOT EXISTS tags        ON document TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS metadata    ON document TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS created_at  ON document TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at  ON document TYPE datetime VALUE time::now();
DEFINE INDEX IF NOT EXISTS document_sha256 ON document FIELDS sha256 UNIQUE;

DEFINE TABLE IF NOT EXISTS chunk SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS document    ON chunk TYPE record<document>;
DEFINE FIELD IF NOT EXISTS idx         ON chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content     ON chunk TYPE string;
DEFINE FIELD IF NOT EXISTS token_count ON chunk TYPE int;
DEFINE FIELD IF NOT EXISTS heading     ON chunk TYPE option<string>;

-- ── Per-document chunk lookups (graph expansion, deletes) ──────────
-- (document, idx) is logically unique: replacement flows delete the old
-- chunks and insert the new ones within one transaction, so duplicates cannot
-- be created through supported flows. Kept non-UNIQUE because SurrealQL 3 has
-- no portable dedup statement for a pre-existing legacy store, and a UNIQUE
-- index would make Db::migrate fail on such data. First creation on an
-- existing large store runs synchronously during Db::migrate on startup.
DEFINE INDEX IF NOT EXISTS chunk_document_idx ON chunk FIELDS document, idx;

-- ── Full-text index (keyword / hybrid) ──────────────
-- 3.x dropped the `SEARCH INDEX` keyword; fulltext uses `FULLTEXT ANALYZER`.
-- `class` tokenizer + lowercase only (ngram broke single-word BM25).
DEFINE ANALYZER IF NOT EXISTS skb_text TOKENIZERS class FILTERS lowercase;
DEFINE INDEX IF NOT EXISTS chunk_content_fts ON chunk FIELDS content
    FULLTEXT ANALYZER skb_text BM25;

-- ── Vector DB 層 ({DIM} = embedding dimension) ──
DEFINE FIELD IF NOT EXISTS embedding ON chunk TYPE array<float>
    ASSERT array::len($value) = {DIM};
DEFINE INDEX IF NOT EXISTS chunk_embedding_hnsw ON chunk
    FIELDS embedding HNSW DIMENSION {DIM} DIST COSINE;

-- ── Graph DB 層 ─────────────────────────────────
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS kind ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS description ON entity TYPE option<string>;
DEFINE INDEX IF NOT EXISTS entity_name_kind_unique ON entity FIELDS name, kind UNIQUE;

DEFINE TABLE IF NOT EXISTS mentions SCHEMAFULL TYPE RELATION FROM chunk TO entity;
DEFINE TABLE IF NOT EXISTS related_to SCHEMAFULL TYPE RELATION FROM entity TO entity;
DEFINE FIELD IF NOT EXISTS relation ON related_to TYPE string;
DEFINE FIELD IF NOT EXISTS weight   ON related_to TYPE float DEFAULT 1.0;

-- ── メタ情報層 ──────────────────────────────────
DEFINE TABLE IF NOT EXISTS meta SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS key   ON meta TYPE string;
DEFINE FIELD IF NOT EXISTS meta_value ON meta TYPE string;
DEFINE INDEX IF NOT EXISTS meta_key_unique ON meta FIELDS key UNIQUE;
