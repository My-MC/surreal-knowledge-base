# Surreal Knowledge Base 仕様書

| 項目 | 内容 |
|---|---|
| プロジェクト名 | Surreal Knowledge Base（以下 **SKB**） |
| 版 | v1.0（ドラフト） |
| 作成日 | 2026-07-29 |
| 対象読者 | 実装者・レビュアー |

---

## 1. 概要

### 1.1 目的

SurrealDB を **Vector DB / Graph DB / Document DB** の 3 用途に用いたローカルファーストなナレッジベースシステムを構築する。以下の **2 つのインターフェース** を提供し、**どちらからでも同等の操作（資料アップロードを含む全機能）が行える** こととする。

1. **MCP サーバー**（Model Context Protocol）— Rust 製。npm パッケージ化し、`npm` / `bun`（`npx` / `bunx`）経由で実行可能。
2. **Skill + CLI** — Rust 製 CLI（`skb`）と、それを駆動する AI エージェント用 Skill（`SKILL.md`）。

### 1.2 背景・設計思想

- SurrealDB はマルチモデル DB であり、1 つのエンジンでドキュメント保存・ベクトル検索（HNSW）・グラフリレーション・全文検索を扱える。インフラを 1 つに集約できる。
- 両インターフェースの機能差異を防ぐため、**全機能を共有コアライブラリ（`skb-core`）に実装** し、MCP サーバーと CLI はその薄いアダプタとする（詳細は §11）。
- 埋め込みモデル・トークナイザはローカル実行とし、外部 API 依存を排除する。

### 1.3 スコープ

**含むもの**

- 資料（テキスト / Markdown / HTML / PDF 等）のアップロード・チャンク化・埋め込み・保存
- ベクトル検索 / 全文検索 / ハイブリッド検索
- エンティティ・リレーションによるグラフ構築とグラフ検索
- ドキュメント管理（一覧・取得・削除・統計）
- MCP サーバー（Rust）の npm パッケージ化・配布
- CLI（Rust）および Skill の提供
- 両アプローチ間の機能パリティ担保の仕組み

**含まないもの（将来拡張）**

- bge-m3 の sparse（lexical weights）/ multi-vector（ColBERT）出力を用いたスコアリング（v1 では dense のみ。ハイブリッドは SurrealDB 全文検索との RRF で実現）
- LLM による自動エンティティ抽出（インターフェースのみ定義）
- マルチユーザー・認証・権限管理
- Web UI

### 1.4 用語

| 用語 | 定義 |
|---|---|
| ドキュメント | アップロードされた資料 1 件（`document` テーブルの 1 レコード） |
| チャンク | ドキュメントをトークン単位で分割した断片。埋め込みの単位（`chunk` テーブル） |
| エンティティ | グラフのノード（概念・固有名詞など、`entity` テーブル） |
| MCP | Model Context Protocol。AI エージェントとツールを接続する標準プロトコル |
| Skill | AI エージェントに CLI の使い方を教える指示書（`SKILL.md`） |
| RRF | Reciprocal Rank Fusion。複数ランキングの統合手法 |

---

## 2. システムアーキテクチャ

### 2.1 全体構成図

```
┌──────────────────────┐      ┌──────────────────────┐
│   AIエージェント等     │      │   AIエージェント      │
│  (MCPクライアント)     │      │  (opencode等) + Skill │
└──────────┬───────────┘      └──────────┬───────────┘
           │ MCP (stdio/HTTP)            │ シェル実行
           ▼                             ▼
┌──────────────────────┐      ┌──────────────────────┐
│  skb-mcp (Rust)      │      │  skb CLI (Rust)      │
│  ※npm/bun経由で起動   │      │                      │
└──────────┬───────────┘      └──────────┬───────────┘
           │                             │
           └──────────────┬──────────────┘
                          ▼
              ┌────────────────────────┐
              │   skb-core (Rust lib)  │   ← 全機能の実体（共有コア）
              │  ingest/search/graph/… │
              └───┬────────┬────────┬──┘
                  ▼        ▼        ▼
        ┌────────────┐┌─────────┐┌─────────────┐
        │ SurrealDB  ││gigatoken││ BAAI/bge-m3 │
        │ (Vector/   ││(Rust)   ││ (ONNX, ort) │
        │ Graph/Doc) │└─────────┘└─────────────┘
        └────────────┘
```

### 2.2 設計方針

1. **単一コア（Single Core）**: `skb-mcp` と `skb` CLI は `skb-core` の公開 API を呼ぶだけのアダプタ。機能は必ずコアに実装し、アダプタ側に独自ロジックを持たせない。
2. **ローカルファースト**: デフォルトは SurrealDB 組込みモード（SurrealKV）で、外部サーバー不要。リモート SurrealDB への接続も設定で切替可能。
3. **決定的な同一入出力**: コアのリクエスト/レスポンス型を serde + schemars で定義し、CLI の JSON 出力と MCP ツールの入出力スキーマを同一型から生成する。

---

## 3. 技術スタック

| レイヤ | 採用技術 | 備考 |
|---|---|---|
| 言語 | Rust（stable, 2021 edition） | workspace 構成 |
| DB | SurrealDB 2.x（Rust クレート `surrealdb`） | 組込み: SurrealKV / リモート: WebSocket |
| トークナイザ | `tokenizers` クレート（HuggingFace 公式、Rust 実装） | Embedding モデルの `tokenizer.json` をロードしてチャンク化に使用（モデルに追随、§5.4）。gigatoken は nightly-only のビルド制約により非採用（2026-07-29 スパイク判定） |
| Embedding | BAAI/bge-m3（ONNX）+ `ort` クレート | **デフォルト。設定ファイルで変更可能（§5.4）**。dense 1024 次元・CLS プーリング・L2 正規化 |
| モデル取得 | `hf-hub` クレート | 初回実行時に Hugging Face からキャッシュへ DL |
| MCP | `rmcp`（Rust 公式 SDK） | stdio 標準、Streamable HTTP 任意 |
| CLI | `clap` 4 | JSON 出力対応 |
| 非同期 | `tokio` | |
| シリアライズ | `serde`, `schemars` | 入出力型の JSON Schema 生成 |
| テキスト抽出 | `html2text` / `pdf-extract` 等 | §12 参照 |
| npm 配布 | npm パッケージ + プラットフォーム別 optionalDependencies | §13 参照 |

### 3.1 採用技術に関する既知の注意点（検証項目）

- **トークナイザ**: gigatoken は crates.io 未公開・nightly Rust 必須（`portable_simd`, `profile-rustflags`）・pyo3 依存の重さによりビルドできず非採用（2026-07-29 検証）。`tokenizers` クレートを採用。`Tokenizer` トレイトでの抽象化により将来的な差し替えは可能。
- **ONNX Runtime**: `ort` 2.0-rc の `download-binaries` 戦略により ONNX Runtime は**静的リンク**される（pyke.io の静的ライブラリ）。バイナリ単体で自己完結し、実行時の `libonnxruntime.so` は不要。TLS も全経路で rustls（ring / aws-lc-rs）を使用し、OpenSSL への動的依存はない。ランタイムの外部依存は libc (glibc ≥ 2.35), libz, libzstd のみ（ORT prebuilt 由来）。
- **SurrealDB Response::take()**: surrealdb 2.x の `Response::take()` は内部 enum 型と serde_json の非互換により実用上制限がある。本番実装では `db.create()` / `db.select()` の型付き API を使用する。

---

## 4. データモデル（SurrealDB スキーマ設計）

SurrealDB の 3 側面とテーブルの対応:

| 側面 | テーブル/機能 | 用途 |
|---|---|---|
| Document DB | `document`, `chunk` | 資料本文・メタデータの保存 |
| Vector DB | `chunk.embedding` + HNSW インデックス | 意味検索 |
| Graph DB | `entity`, `mentions`（RELATION）, `related_to`（RELATION） | 知識グラフ |
| （補助）全文検索 | `chunk_content_fts` インデックス | キーワード検索・ハイブリッド検索 |

### 4.1 SurrealQL スキーマ定義（初期マイグレーション）

```sql
DEFINE NAMESPACE IF NOT EXISTS skb;
USE NS skb;
DEFINE DATABASE IF NOT EXISTS knowledge;
USE DB knowledge;

-- ── Document DB 層 ──────────────────────────────
DEFINE TABLE document SCHEMAFULL;
DEFINE FIELD title       ON document TYPE string;
DEFINE FIELD source      ON document TYPE string;          -- ファイルパス / URL / "text" 等
DEFINE FIELD source_type ON document TYPE string
    ASSERT $value IN ["file", "url", "text", "stdin"];
DEFINE FIELD mime        ON document TYPE option<string>;
DEFINE FIELD sha256      ON document TYPE string;
DEFINE FIELD content     ON document TYPE string;          -- 抽出済み全文（再チャンク化・再埋め込みに使用、§5.4）
DEFINE FIELD tags        ON document TYPE array<string> DEFAULT [];
DEFINE FIELD metadata    ON document TYPE object DEFAULT {};
DEFINE FIELD created_at  ON document TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at  ON document TYPE datetime VALUE time::now();
DEFINE INDEX document_sha256_unique ON document FIELDS sha256 UNIQUE;

DEFINE TABLE chunk SCHEMAFULL;
DEFINE FIELD document    ON chunk TYPE record<document>;
DEFINE FIELD idx         ON chunk TYPE int;                -- ドキュメント内順序
DEFINE FIELD content     ON chunk TYPE string;
DEFINE FIELD token_count ON chunk TYPE int;
DEFINE FIELD heading     ON chunk TYPE option<string>;     -- 所属見出し（あれば）

-- ── Vector DB 層 ────────────────────────────────
-- 次元数 {DIM} は初期化時に設定（embedding.dimension / 自動検出値）から埋め込む
-- テンプレート変数。デフォルト（BAAI/bge-m3）では 1024。
DEFINE FIELD embedding ON chunk TYPE array<float>
    ASSERT array::len($value) = {DIM};
DEFINE INDEX chunk_embedding_hnsw ON chunk
    FIELDS embedding HNSW DIMENSION {DIM} DIST COSINE;

-- ── 全文検索層 ──────────────────────────────────
-- 日本語・多言語対応のため class トークナイザ + ngram フィルタ
DEFINE ANALYZER skb_text TOKENIZERS class FILTERS lowercase, ngram(2,3);
DEFINE INDEX chunk_content_fts ON chunk
    FIELDS content SEARCH ANALYZER skb_text BM25;

-- ── Graph DB 層 ─────────────────────────────────
DEFINE TABLE entity SCHEMAFULL;
DEFINE FIELD name ON entity TYPE string;
DEFINE FIELD kind ON entity TYPE string;                   -- person/org/concept/tech/…
DEFINE FIELD description ON entity TYPE option<string>;
DEFINE INDEX entity_name_kind_unique ON entity FIELDS name, kind UNIQUE;

-- チャンク → エンティティ（言及）
DEFINE TABLE mentions SCHEMAFULL TYPE RELATION FROM chunk TO entity;

-- エンティティ → エンティティ（関連）
DEFINE TABLE related_to SCHEMAFULL TYPE RELATION FROM entity TO entity;
DEFINE FIELD relation ON related_to TYPE string;           -- "references"/"part-of"/自由記述
DEFINE FIELD weight   ON related_to TYPE float DEFAULT 1.0;

-- ── メタ情報（稼働中モデルの記録・設定不整合の検出用、§5.4） ──
DEFINE TABLE meta SCHEMAFULL;
DEFINE FIELD key   ON meta TYPE string;
DEFINE FIELD value ON meta TYPE any;
DEFINE INDEX meta_key_unique ON meta FIELDS key UNIQUE;
-- 記録キー: schema_version / embedding_model / embedding_dimension /
--          embedding_max_input_tokens / tokenizer
```

### 4.2 冪等性・重複排除

- `document.sha256`（本文テキストの SHA-256）に UNIQUE 制約。同一内容の再アップロードは **既存ドキュメントの更新（upsert）** として扱い、チャンクを再生成する（`--force` / `force: true` で明示した場合のみ）か、デフォルトでは `skipped: true` を返して何もしない。

---

## 5. 取り込みパイプライン（Ingestion）

```
入力(ファイル/URL/テキスト/stdin/base64)
  → ① 取得・形式判定          … MIME/拡張子で分岐
  → ② テキスト抽出            … プレーンテキスト化（Markdown構造は保持）
  → ③ チャンク化              … gigatoken でトークン単位分割
  → ④ 埋め込み                … bge-m3 (ONNX) で dense ベクトル化
  → ⑤ 保存                    … document + chunk をトランザクションで保存
  → ⑥ グラフ構築              … エンティティ抽出（ルールベース）→ RELATE
```

### 5.1 チャンク化（gigatoken）

- トークナイザ: **BAAI/bge-m3 と同一のトークナイザ**（XLM-RoBERTa 系）を gigatoken でロードし、埋め込みモデルの語彙と完全に一致したトークン数で分割する。
- 既定値: `max_tokens = 512`、`overlap_tokens = 64`。いずれも **設定ファイルで変更可能**（§5.4）。`max_tokens` は使用中モデルの最大入力トークン数以下であれば任意に設定できる。
- 分割方針: 見出し・段落・文境界を優先しつつ、`max_tokens` を超えない範囲で結合。どうしても超える場合はトークン境界でハード分割。
- `token_count` は gigatoken のエンコード結果から実測して記録。

### 5.2 埋め込み（BAAI/bge-m3）

| 項目 | 値 |
|---|---|
| モデル | `BAAI/bge-m3`（公式 ONNX エクスポート）**← デフォルト。設定ファイルで変更可能（§5.4）** |
| 出力 | dense ベクトル 1024 次元 |
| プーリング | CLS + L2 正規化（コサイン類似度運用のため） |
| 最大系列長 | 8192 トークン（bge-m3 の上限。実際のチャンク長は `chunking.max_tokens` で制御） |
| バッチ | `batch_size = 32`（設定可能） |
| 実行 | CPU 既定（`device = "cuda"` / `"coreml"` は将来拡張） |
| 配布 | 初回起動時に HF Hub から `~/.cache/skb/models/` へ DL（`hf-hub`）。オフライン時は `embedding.onnx_path` でローカル指定可 |

- クエリ側は bge-m3 の仕様上 **instruction プレフィックス不要**（そのまま埋め込む）。

### 5.3 グラフ構築（ルールベース抽出）

v1 では LLM 非依存のルールベースで行う:

| 抽出源 | エンティティ | リレーション |
|---|---|---|
| Markdown の `[[WikiLink]]` / `[text](link)` | リンク先を `entity` 化 | `chunk ->mentions-> entity`、リンク先が他ドキュメントならエンティティ経由で関連 |
| YAML frontmatter の `tags`, `aliases` | タグを `entity(kind="tag")` 化 | `mentions` |
| 見出し階層 | 章題を `entity(kind="section")` 化 | `related_to(relation="part-of")` |
| 手動操作 | `skb graph link` / MCP `skb_graph_link` | `related_to` |

LLM 抽出は `EntityExtractor` トレイトの差し替え実装として将来追加する。

### 5.4 Embedding モデル・チャンク長の変更（設定可能化）

デフォルト（Embedding = **BAAI/bge-m3**、チャンク長 = **512 トークン**）は維持したまま、**設定ファイルの編集のみで後から変更できる** ものとする。

#### 変更可能な設定キー

- `embedding.model`: HF モデル ID またはローカル ONNX のパス。トークナイザは既定で同一モデルからロードするため、**モデルとトークナイザの語彙不一致は起きない**。
- `embedding.tokenizer`: 明示上書き用（既定 `"auto"` = モデルに追随）。
- `embedding.dimension`: 埋め込み次元数（既定 0 = モデルに試行推論して自動検出）。
- `embedding.max_input_tokens`: モデルのコンテキスト長上限（既定 0 = モデルの設定ファイルから自動検出。bge-m3 では 8192）。
- `chunking.max_tokens` / `chunking.overlap_tokens`: チャンク長・オーバーラップ。

#### 整合性ルール（`KnowledgeBase::open` 時に検証）

1. `0 < overlap_tokens < max_tokens ≤ max_input_tokens` を検証。違反時は `E_VALIDATION`。
2. `meta` テーブルに記録された `embedding_model` / `embedding_dimension` と設定値を比較。**不一致のまま通常操作は行わず** `E_MODEL_MISMATCH` を返し、再構築（`reindex`）を案内する。これにより、異なる次元・語彙のベクトルが同一インデックスに混在することを防ぐ。

#### 変更手順（reindex）

1. 設定ファイルを編集（モデル / チャンク長）。
2. `skb reindex`（MCP: `skb_reindex`）を実行:
   - モデル変更時: 新モデル・新トークナイザをロード。次元が変わる場合は `chunk.embedding` フィールドと HNSW インデックスを新次元で再定義。
   - 全ドキュメントの `document.content`（抽出済み全文）を新設定で再チャンク化 → 再埋め込み → `chunk` を置換（ドキュメント単位のトランザクション）。グラフの `mentions` も再構築。
   - `meta` テーブルを新モデル情報で更新。
3. チャンク長のみの変更も、既存ドキュメントへの反映には同じ `reindex` が必要（新規アップロード分には即時反映される）。

※ reindex は全件再処理のため、大規模データでは長時間化する。進捗通知（§7.1）に対応する。

---

## 6. 検索パイプライン

| モード | 実装 | 用途 |
|---|---|---|
| `vector` | `embedding <|k, EF|> $q`（HNSW KNN）+ コサイン類似度 | 意味検索 |
| `keyword` | `content @query@`（BM25 全文検索） | 語句検索 |
| `hybrid`（既定） | vector と keyword をそれぞれ top_k×3 取得し **RRF**（`k=60`）で統合 | 汎用 |
| グラフ拡張（任意） | ヒットしたチャンクの `->mentions->entity` から N ホップ先のエンティティを言及するチャンクを加えて再ランク | 関連資料の発見 |

レスポンスには `document_id`, `title`, `chunk_idx`, `content`（スニペット）, `score`, `source`, `highlights`（keyword 時）, `matched_entities`（グラフ拡張時）を含める。

---

## 7. コアライブラリ仕様（`skb-core`）

### 7.1 公開 API（抜粋・Rust）

```rust
pub struct KnowledgeBase { /* db, embedder, tokenizer, config */ }

impl KnowledgeBase {
    pub async fn open(config: &Config) -> Result<Self>;

    // 資料管理
    pub async fn upload(&self, req: UploadRequest) -> Result<UploadResult>;
    pub async fn list_documents(&self, q: ListQuery) -> Result<Page<DocumentSummary>>;
    pub async fn get_document(&self, id: &str, opts: GetOptions) -> Result<DocumentDetail>;
    pub async fn delete_document(&self, id: &str) -> Result<DeleteResult>;

    // 検索
    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse>;

    // グラフ
    pub async fn graph_query(&self, req: GraphQueryRequest) -> Result<GraphQueryResponse>;
    pub async fn upsert_entity(&self, req: EntityRequest) -> Result<Entity>;
    pub async fn link(&self, req: LinkRequest) -> Result<LinkResult>;

    // 管理
    pub async fn stats(&self) -> Result<Stats>;
    pub async fn doctor(&self) -> Result<DoctorReport>;  // 環境診断
    pub async fn reindex(&self, req: ReindexRequest) -> Result<ReindexResult>; // モデル/チャンク設定変更の全件反映（§5.4）
}
```

- すべての Request/Response 型は `Serialize`/`Deserialize`/`JsonSchema` を derive し、**CLI の JSON 入出力と MCP ツールスキーマの双方をこの型から生成**する。
- 非同期（`tokio`）。長時間処理（upload）は内部で進捗コールバックを受け取れる設計とし、MCP では progress notification、CLI ではプログレスバーへ写像する。

### 7.2 設定

読み込み優先順位: フラグ/引数 > 環境変数（`SKB_*`）> プロジェクト `./skb.toml` > ユーザ `~/.config/skb/config.toml`。

```toml
[storage]
mode = "embedded"                    # embedded | remote
path = "~/.local/share/skb/db"       # embedded: SurrealKV データディレクトリ
# url = "ws://127.0.0.1:8000"        # remote 時
# username = "root"
# password = "root"
namespace = "skb"
database = "knowledge"

[embedding]
model = "BAAI/bge-m3"                # Embedding モデル（HF ID or ローカルパス）。変更はこのキーを編集（§5.4）
onnx_path = "auto"                   # "auto"=HFキャッシュ / 明示パス可
tokenizer = "auto"                   # "auto"=モデルに追随（gigatoken でロード）/ 明示指定も可
dimension = 0                        # 埋め込み次元数。0=モデルから自動検出（bge-m3 は 1024）
max_input_tokens = 0                 # モデルのコンテキスト長上限。0=モデル設定から自動検出（bge-m3 は 8192）
device = "cpu"
batch_size = 32

[chunking]
max_tokens = 512                     # チャンク長のデフォルト。max_input_tokens 以下で変更可（既存データ反映は reindex）
overlap_tokens = 64                  # max_tokens 未満であること

[search]
default_mode = "hybrid"
top_k = 10
rrf_k = 60

[upload]
max_file_mb = 100
allowed_dirs = []                    # MCP経由の path アップロード許可ディレクトリ（空=無制限）
```

---

## 8. MCP サーバー仕様（`skb-mcp`）

### 8.1 基本仕様

| 項目 | 内容 |
|---|---|
| 実装 | Rust バイナリ `skb-mcp`（`rmcp` 使用） |
| トランスポート | stdio（既定）/ Streamable HTTP（`--http --port 8787`） |
| 起動方法 | `npx surreal-knowledge-base` / `bunx surreal-knowledge-base` / バイナリ直接実行 |
| ログ | **stderr のみ**（stdio 運用時に stdout を汚染しない） |
| 終了コード | 0 正常 / 1 起動失敗 / 2 設定不正 |

### 8.2 ツール一覧

全ツールの入出力は `skb-core` の型から生成した JSON Schema に従う。

| # | ツール名 | 概要 | 主要パラメータ |
|---|---|---|---|
| 1 | `skb_upload` | 資料をアップロード | `path?`, `url?`, `content?`, `content_base64?`, `title?`, `tags?`, `metadata?`, `force?` |
| 2 | `skb_search` | 検索 | `query`, `mode?=hybrid`, `top_k?=10`, `filter?`, `graph_expand?=0` |
| 3 | `skb_list_documents` | 一覧 | `limit?=50`, `offset?=0`, `order?=updated_desc` |
| 4 | `skb_get_document` | 取得 | `id`, `include_chunks?=false` |
| 5 | `skb_delete_document` | 削除 | `id` |
| 6 | `skb_graph_query` | グラフ探索 | `from`（entity名 or document id）, `relation?`, `depth?=1`, `limit?=50` |
| 7 | `skb_graph_upsert_entity` | エンティティ作成/更新 | `name`, `kind`, `description?` |
| 8 | `skb_graph_link` | エンティティ間リンク | `from`, `to`, `relation`, `weight?=1.0` |
| 9 | `skb_stats` | 統計 | なし |
| 10 | `skb_reindex` | モデル/チャンク設定変更の全件反映（再チャンク化・再埋め込み、§5.4） | `dry_run?=false` |

#### `skb_upload` の入出力例

```jsonc
// 入力（path / url / content / content_base64 のいずれか 1 つ必須）
{ "path": "/home/user/docs/design.md", "tags": ["design"], "metadata": {"project": "skb"} }
// 出力
{
  "document_id": "document:01J…",
  "title": "design.md",
  "status": "created",            // created | updated | skipped
  "chunks": 42,
  "tokens": 18934,
  "entities": ["SurrealDB", "bge-m3"]
}
```

### 8.3 リソース・プロンプト

- **Resources**: `skb://documents`（一覧）, `skb://documents/{id}`（本文）, `skb://stats` — 読み取り専用。
- **Prompts**: `skb-answer`（`question` を受け取り `skb_search` の結果を根拠に回答する RAG 用テンプレート）。

### 8.4 MCP クライアント設定例

```jsonc
// opencode.json（bun 使用例。npm なら "npx" に置き換え）
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "surreal-knowledge-base": {
      "type": "local",
      "command": ["bunx", "surreal-knowledge-base"],
      "enabled": true
    }
  }
}
```

---

## 9. CLI 仕様（`skb`）

### 9.1 コマンド体系

```
skb upload <paths...> [--url U] [--stdin] [--recursive] [--tags a,b]
                      [--metadata JSON] [--force]
skb search <query> [--mode hybrid|vector|keyword] [--top-k N]
                   [--graph-expand N] [--filter KEY=VAL]...
skb list [--limit N] [--offset N] [--order ...]
skb get <id> [--chunks]
skb delete <id> [--yes]
skb graph query --from <entity-or-doc> [--relation R] [--depth N]
skb graph entity add <name> --kind K [--description S]
skb graph link <from> <to> [--relation R] [--weight F]
skb stats
skb reindex [--dry-run]             # モデル/チャンク設定変更の全件反映（§5.4）
skb config init | show | set <key> <value>
skb mcp serve [--http --port N]     # skb-mcp と同一エントリポイント
skb doctor                          # DB・モデル・トークナイザの疎通診断
```

### 9.2 入出力規約

| 項目 | 規約 |
|---|---|
| 出力形式 | `--format json`（既定）/ `--format table`（人間向け） |
| 成功時 | stdout に結果 JSON、終了コード 0 |
| 失敗時 | stderr に `{"error":{"code","message","details"}}`、終了コード 1 以上 |
| stdin | `skb upload --stdin` で本文を標準入力から読む（パイプ連携用） |
| バイナリ受領 | `skb upload` はファイルパス指定のみ（base64 標準入力は `--stdin --base64`） |

---

## 10. Skill 仕様

### 10.1 概要

- 名称: `surreal-knowledge-base`
- 配置: リポジトリ内 `skills/surreal-knowledge-base/SKILL.md`。利用時は `~/.config/opencode/skills/surreal-knowledge-base/`（ユーザ全体）またはプロジェクトの `.opencode/skills/` に配置する。
- 役割: AI エージェントが `skb` CLI を適切に呼び出すための指示書。**Skill 自身は機能を実装せず、必ず CLI を経由する**（これにより MCP 経由と同一のコア動作が保証される）。

### 10.2 `SKILL.md` に含める内容

1. フロントマター（`name`, `description`）— 「ナレッジベースへの資料登録・検索が必要なときに使う」旨を記述。
2. 前提確認: `skb doctor` でセットアップ済みか確認し、未設定なら `skb config init` を案内。
3. 操作レシピ（全て `--format json` で呼び出す）:
   - 資料の追加: `skb upload <path>` / `cat file | skb upload --stdin --title ...`
   - 検索して根拠付きで回答: `skb search "<query>" --graph-expand 1`
   - 一覧・取得・削除・グラフ操作
4. 出力の解釈ルール: 検索結果の `score`・出典（`source`）を回答に必ず付記する。
5. エラー時の対処: エラーコード表（§14）に基づくリトライ/案内。

---

## 11. 機能パリティ保証

### 11.1 パリティマトリクス

| 機能 | MCP | CLI | Skill |
|---|---|---|---|
| 資料アップロード（ファイル） | ✅ `skb_upload(path)` | ✅ `skb upload <path>` | ✅ CLI 経由 |
| 資料アップロード（URL） | ✅ `skb_upload(url)` | ✅ `skb upload --url` | ✅ CLI 経由 |
| 資料アップロード（インラインテキスト） | ✅ `skb_upload(content)` | ✅ `skb upload --stdin` | ✅ CLI 経由 |
| 資料アップロード（バイナリ） | ✅ `skb_upload(content_base64)` | ✅ `skb upload <path>` / `--stdin --base64` | ✅ CLI 経由 |
| 検索（vector/keyword/hybrid/グラフ拡張） | ✅ `skb_search` | ✅ `skb search` | ✅ CLI 経由 |
| 一覧 / 取得 / 削除 / 統計 | ✅ 各ツール | ✅ 各コマンド | ✅ CLI 経由 |
| グラフ探索・エンティティ作成・リンク | ✅ 各ツール | ✅ 各コマンド | ✅ CLI 経由 |
| 環境診断 | ✅ `skb_stats`+起動時セルフチェック | ✅ `skb doctor` | ✅ CLI 経由 |
| 再構築（reindex、モデル/チャンク長変更の反映） | ✅ `skb_reindex` | ✅ `skb reindex` | ✅ CLI 経由 |
| 生 SurrealQL 実行 | ❌（非公開・セキュリティ上除外） | ✅ `skb query`（上級者向け、パリティ対象外） | — |

### 11.2 パリティの担保方法

1. **構造的保証**: 両アダプタが `skb-core` の同一メソッドを呼ぶ。アダプタへのロジック混入はレビューで禁止。
2. **型の一元化**: 入出力型をコアで定義し、MCP スキーマ・CLI JSON を自動生成。
3. **契約テスト**: 同一リクエスト JSON を MCP ハンドラ経由と CLI 経由の双方に投入し、レスポンスの JSON 一致を検証するゴールデンテストを CI で実行（§16）。
4. **リリースゲート**: 新機能追加時は本マトリクスの更新を必須とし、両アプローチの実装が揃うまでリリースしない。

---

## 12. アップロード仕様

### 12.1 対応フォーマット

| 形式 | 抽出方法 | フェーズ |
|---|---|---|
| プレーンテキスト (.txt) | そのまま | v1 |
| Markdown (.md) | 構造保持（見出し・リンク抽出に利用） | v1 |
| HTML (.html) | `html2text` 系でテキスト化 | v1 |
| PDF (.pdf) | `pdf-extract` 等でテキスト抽出 | v1 |
| Word (.docx) | テキスト抽出クレート | v2 |
| その他バイナリ | 拒否（`E_UNSUPPORTED_FORMAT`） | — |

### 12.2 入力経路（MCP / CLI 双方で同等に提供）

| 経路 | MCP `skb_upload` | CLI |
|---|---|---|
| ローカルファイル | `path`（`upload.allowed_dirs` で制限可） | `<paths...>`（複数・glob・`--recursive` 可） |
| URL 取得 | `url`（HTTP(S) GET、サイズ上限あり） | `--url` |
| インラインテキスト | `content` | `--stdin`（パイプ） |
| バイナリ | `content_base64` | `--stdin --base64` |

### 12.3 振る舞い

- サイズ上限: 既定 100MB（`upload.max_file_mb`）。
- 重複: SHA-256 一致時は `skipped`（`force` で再取り込み＝既存チャンク置換）。
- 部分失敗: 複数ファイル指定時、失敗分は `errors[]` に集約して返し、成功分はコミットする。
- トランザクション: 1 ドキュメントの `document` + `chunk` + `mentions` は 1 トランザクションで保存。

---

## 13. npm パッケージ化（`skb-mcp`）

### 13.1 配布形態

esbuild / Biome と同様の **プラットフォーム別バイナリ + optionalDependencies** 方式。

| パッケージ | 内容 |
|---|---|
| `surreal-knowledge-base` | メタパッケージ。`bin/skb-mcp.js`（起動ラッパ）のみ含む |
| `@surreal-knowledge-base/darwin-arm64` 等 | 各 OS/アーチ向け `skb-mcp` 自己完結バイナリ |

対象ターゲット: `linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64`, `win32-x64`。

### 13.2 メタパッケージ `package.json`（骨子）

```jsonc
{
  "name": "surreal-knowledge-base",
  "version": "0.1.0",
  "bin": { "skb-mcp": "./bin/skb-mcp.js" },
  "engines": { "node": ">=18" },
  "optionalDependencies": {
    "@surreal-knowledge-base/linux-x64": "0.1.0",
    "@surreal-knowledge-base/linux-arm64": "0.1.0",
    "@surreal-knowledge-base/darwin-x64": "0.1.0",
    "@surreal-knowledge-base/darwin-arm64": "0.1.0",
    "@surreal-knowledge-base/win32-x64": "0.1.0"
  }
}
```

### 13.3 起動ラッパ（`bin/skb-mcp.js`）要件

- Node.js 依存 API のみで実装（`node:child_process`, `node:process`）。**bun でもそのまま動作** する（bun は Node API 互換のため）。追加依存ゼロ。
- 動作: `process.platform` / `process.arch` から対応 optional パッケージを `require.resolve` → 同梱バイナリを `spawn`（`stdio: "inherit"`）→ 子プロセスの終了コードを伝播。
- 見つからない場合: 対応プラットフォームと手動インストール手順を stderr に出して終了コード 1。

### 13.4 バイナリ・ランタイム要件

```
@surreal-knowledge-base/linux-x64/
├── package.json        # "os": ["linux"], "cpu": ["x64"]
├── bin/skb-mcp         # Rust バイナリ（ORT 静的リンク・自己完結）
└── THIRD_PARTY_LICENSES.md  # ONNX Runtime MIT ライセンス表示
```

- `ort` 2.0-rc の `download-binaries` 戦略により ONNX Runtime は静的リンクされる。共有ライブラリの同梱は不要。
- TLS は全経路で rustls（ring / aws-lc-rs）を使用。OpenSSL は実行時に一切不要。
- libstdc++ / libgcc_s もビルド時に静的リンク（`-static-libstdc++ -static-libgcc`）。
- モデル（bge-m3 ONNX ~2GB）は **npm には同梱せず** 初回起動時に HF から DL。

**Linux ランタイム要件**:

| 依存 | 理由 |
|---|---|
| glibc ≥ 2.35 | ビルドランナー（ubuntu-22.04）のABIフロア |
| libz | ORT prebuilt 静的ライブラリの動的依存 |
| libzstd | 同上 |
| ca-certificates | hf-hub の TLS 証明書検証（ureq 側は webpki-roots 埋め込み済み） |

macOS / Windows は OS 付属以外の動的依存なし（Windows は CRT 静的化で standalone .exe）。

### 13.5 実行方法

```bash
npx  surreal-knowledge-base          # npm 経由で MCP サーバー起動（stdio）
bunx surreal-knowledge-base          # bun 経由
bunx surreal-knowledge-base --http --port 8787   # HTTP モード
```

---

## 14. エラーハンドリング・ロギング

### 14.1 エラーモデル（共通）

```jsonc
{ "error": { "code": "E_DOCUMENT_NOT_FOUND", "message": "…", "details": { /* 任意 */ } } }
```

| コード | 意味 | CLI 終了コード |
|---|---|---|
| `E_CONFIG` | 設定不正・未初期化 | 2 |
| `E_DB` | SurrealDB 接続/クエリ失敗 | 3 |
| `E_UNSUPPORTED_FORMAT` | 非対応形式 | 4 |
| `E_IO` | ファイル/ネットワーク取得失敗 | 5 |
| `E_DOCUMENT_NOT_FOUND` | 対象なし | 6 |
| `E_EMBEDDING` | モデルロード/推論失敗 | 7 |
| `E_VALIDATION` | 入力検証失敗 | 8 |
| `E_MODEL_MISMATCH` | 稼働中モデルと設定の不一致（reindex 必要、§5.4） | 9 |

- MCP 側はツール結果として `{ isError: true, content: [上記 JSON の text] }` を返す（プロトコルエラーにはしない。起動不能時のみプロトコルエラー）。
- ログ: `tracing` + `RUST_LOG`。MCP は **stderr のみ**、CLI は stderr。機微情報（本文・パス）のログ出力は `debug` レベル以下に限定。

---

## 15. セキュリティ

- MCP 経由の `path` アップロードは `upload.allowed_dirs` 設定時にディレクトリ外参照を拒否（パストラバーサル対策）。
- 生 SurrealQL は MCP には公開しない（CLI のみ・明示コマンド）。
- URL 取得は HTTP(S) のみ許可、リダイレクト上限・サイズ上限を設定。`file://` 等のスキームは拒否（SSRF 緩和）。
- 認証情報（リモート SurrealDB のパスワード等）は設定ファイルのパーミッション 600 を推奨し、環境変数での上書きを可能にする。

---

## 16. テスト計画

| 層 | 内容 | ツール |
|---|---|---|
| 単体 | チャンク化（トークン数が gigatoken 実測と一致、overlap 正しい）、RRF、スキーマ CRUD | `cargo test` |
| 統合 | 組込み SurrealDB で upload → search → delete の一連動作。bge-m3 は小型ダミー or 実モデルの量子化版で CI 実行 | `cargo test --features it` |
| 契約（パリティ） | §11.2-3。同一 JSON リクエストを MCP/CLI 両経路で実行し応答を比較 | ゴールデンファイル |
| E2E | `npm pack` したパッケージを `npx` / `bunx` で起動し、initialize→tools/list→skb_upload→skb_search が通ることを検証 | シェルスクリプト + CI |
| ベンチ | トークナイズ速度（gigatoken、SentencePiece 系の実測）、Embedding スループット、検索レイテンシ | `criterion` |

### 16.1 性能目標（初版目標・要検証）

| 指標 | 目標 | 実測（20 コア, x86_64） |
|---|---|---|
| チャンク化 | 10MB テキストを 5 秒以内 | encode 10MB: 5.35 s / chunk(512,64) 10MB: 5.27 s（tokenizers v0.23, bge-m3 トークナイザ） |
| Embedding | 512 トークン × バッチ 32、8 コア CPU で 5 chunks/s 以上 | —（ort feature 未実測; 要 `pkg-config` + `libssl-dev`） |
| 検索 | 10 万チャンク規模で hybrid 検索 p95 < 500ms | 1,000 チャンク: hybrid 11.3 ms, vector 1.5 ms, keyword 9.2 ms（mock 埋め込み, 10 万チャンクは未実測） |
| MCP 起動 | コールドスタート 3 秒以内 | **4.54 s**（mock 設定, tokenizer キャッシュ済み。実装が eager ロードのため目標超過） |

> 計測環境: Linux x86_64, 20 コア CPU, 31 GB RAM, Rust 1.97.1, criterion 0.5。
> ort 実埋め込みベンチマークは `pkg-config` + `libssl-dev` のインストール後に `cargo bench --features ort` で実行可能。

---

## 17. ディレクトリ構成

```
surreal-knowledge-base/
├── Cargo.toml                    # workspace
├── crates/
│   ├── skb-core/                 # 共有コア（本仕様の実体）
│   │   └── src/{config,db,ingest,embed,tokenize,search,graph,error}.rs
│   ├── skb-cli/                  # CLI（clap）
│   └── skb-mcp/                  # MCP サーバー（rmcp）
├── npm/
│   ├── package.json              # メタパッケージ
│   ├── bin/skb-mcp.js            # ラッパ
│   └── packages/                 # プラットフォーム別パッケージ
├── skills/
│   └── surreal-knowledge-base/
│       └── SKILL.md
├── schema/
│   └── 001_init.surql            # §4.1 のマイグレーション（次元数は初期化時に設定値から埋め込むテンプレート）
├── tests/
│   ├── contract/                 # パリティ用ゴールデンテスト
│   └── e2e/
├── .github/workflows/            # build / test / npm publish
└── SPECIFICATION.md              # 本書
```

---

## 18. マイルストーン

| MS | 内容 | 完了条件 |
|---|---|---|
| M1 | `skb-core` + CLI（upload/search/list/get/delete/stats、組込み DB） | CLI 統合テスト緑 |
| M2 | `skb-mcp` + npm パッケージ化（linux-x64/darwin-arm64 先行） | `npx`/`bunx` E2E 緑 |
| M3 | Skill 整備 + 契約テストによるパリティ CI 化 | マトリクス全項目 ✅ |
| M4 | グラフ強化（抽出ルール拡充、グラフ拡張検索の再ランク精度評価） | 評価レポート |
| M5 | 性能チューニング・win32 対応・docx 対応（v2 スコープ判断） | 性能目標達成 |

---

## 19. 未決事項・リスク

| # | 項目 | 影響 | 対応方針 |
|---|---|---|---|
| 1 | gigatoken の SentencePiece（XLM-R）系の実測性能・互換性 | チャンク化の速度 | M1 早期にベンチ。問題時は `tokenizers` クレートへ feature flag 切替（トレイト抽象化済み） |
| 2 | onnxruntime 同梱による npm パッケージサイズ増 | 配布 | CPU 版最小構成で同梱。超過時は postinstall ダウンロード方式へ変更 |
| 3 | SurrealDB FTS の日本語品質 | keyword/hybrid 精度 | ngram 設定のチューニングを M4 で評価。必要なら形態素解析アナライザ追加 |
| 4 | bge-m3 初回 DL のサイズ（約 2GB 級） | 初回 UX | `skb doctor` で進捗表示付き事前 DL を案内。量子化版 ONNX の採用も検討 |
| 5 | PDF 抽出クレートの選定 | v1 スコープ | M1 で `pdf-extract` を検証し不十分なら代替選定 |
