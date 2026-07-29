---
name: surreal-knowledge-base
description: >
  Use when you need to register documents in a knowledge base, search through stored
  knowledge, manage documents, or query a knowledge graph. This skill provides access
  to the `skb` CLI which wraps a SurrealDB-based multi-model knowledge base
  (Vector/Graph/Document DB) with BAAI/bge-m3 embeddings.
---

# Surreal Knowledge Base

The `skb` CLI provides a local-first knowledge base using SurrealDB for vector
search, graph relations, and document storage, with BAAI/bge-m3 embeddings.

## Prerequisites

Before using, verify the setup is working:

```bash
skb doctor
```

If not initialized, create a default config:

```bash
skb config init
```

Edit `./skb.toml` to configure embedding model, storage path, chunking, etc.

## Operations

Always use `--format json` for structured output. All commands may trigger
first-time downloads of the tokenizer.json (17MB) from HuggingFace Hub.

### Upload Document

```bash
# From file
skb upload --path /path/to/doc.md --title "My Document" --tags ml,vector

# From URL
skb upload --url https://example.com/doc.html --title "Web Doc"

# From pipe/stdin
cat notes.txt | skb upload --stdin --title "Meeting Notes"

# Re-upload and overwrite existing
skb upload --path doc.md --force
```

### Search

```bash
# Hybrid search (default, vector + keyword + RRF)
skb search "multi-model database"

# Vector-only search
skb search "nearest neighbor" --mode vector --top-k 5

# Keyword search
skb search "HNSW" --mode keyword

# With graph expansion (find related documents)
skb search "SurrealDB" --graph-expand 3
```

### List / Get / Delete

```bash
skb list --limit 20
skb get document:abc123 --chunks
skb delete document:abc123 --yes
```

### Stats

```bash
skb stats
```

### Graph Operations

```bash
# Query graph from a node
skb graph query --from "SurrealDB" --depth 2

# Add entity
skb graph entity "HNSW" --kind algorithm

# Link entities
skb graph link "SurrealDB" "HNSW" --relation uses
```

### Reindex

```bash
# After changing chunking/embedding config in skb.toml
skb reindex
```

## Interpreting Results

- Search results include `document_id`, `chunk_idx`, `content`, and `score`.
- Always cite the document source when presenting search results.
- If `status` is "skipped", the document already exists (SHA-256 match).
- Use `--force` to re-upload and replace existing documents with the same content.

## Error Handling

- `E_EMBEDDING`: Model loading or inference failure. Check `skb.toml` and network.
- `E_DB`: Database connection error. Check storage path permissions.
- `E_MODEL_MISMATCH`: Config model differs from stored model. Run `skb reindex`.
- `E_DOCUMENT_NOT_FOUND`: The requested document does not exist.
- `E_VALIDATION`: Invalid parameters. Verify input format.
