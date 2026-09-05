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

### 1.5 本書の読み方

本書は v1 の目標仕様であり、仕様に記載された機能がすべて実装済みであることを意味しない。現在の実装状況、対応するコード・テスト、未実装項目の優先順位は `IMPLEMENTATION_PLAN.md` で管理する。実装済みと明記されていない契約は、Phase 9 の完了条件を満たすまで実装予定として扱う。

---

## 2. システムアーキテクチャ

### 2.1 全体構成図

```
┌──────────────────────┐      ┌──────────────────────┐
│   AIエージェント等     │      │   AIエージェント      │
│  (MCPクライアント)     │      │  (opencode等) + Skill │
└──────────┬───────────┘      └──────────┬───────────┘
           │ MCP (stdio)                 │ シェル実行
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
         ┌────────────┐┌─────────────┐┌─────────────┐
         │ SurrealDB  ││ tokenizers  ││ BAAI/bge-m3 │
         │ (Vector/   ││ (Rust/HF)   ││ (ONNX, ort) │
         │ Graph/Doc) │└─────────────┘└─────────────┘
        └────────────┘
```

### 2.2 設計方針

1. **単一コア（Single Core）**: `skb-mcp` と `skb` CLI は `skb-core` の公開 API を呼ぶだけのアダプタ。機能は必ずコアに実装し、アダプタ側に独自ロジックを持たせない。
2. **ローカルファースト**: v1 は SurrealDB 組込みモード（SurrealKV）のみを対象とし、外部サーバーを不要とする。リモート SurrealDB 接続は将来拡張とする。
3. **決定的な同一入出力**: コアのリクエスト/レスポンス型を serde + schemars で定義し、CLI の JSON 出力と MCP ツールの入出力スキーマを同一型から生成する。

---

## 3. 技術スタック

| レイヤ | 採用技術 | 備考 |
|---|---|---|
| 言語 | Rust（stable, 2021 edition） | workspace 構成 |
| DB | SurrealDB 3.x（Rust クレート `surrealdb`） | 組込み: SurrealKV（リモート未実装） |
| トークナイザ | `tokenizers` クレート（HuggingFace 公式、Rust 実装） | Embedding モデルの `tokenizer.json` をロードしてチャンク化に使用（モデルに追随、§5.4）。gigatoken は nightly-only のビルド制約により非採用（2026-07-29 スパイク判定） |
| Embedding | BAAI/bge-m3（ONNX）+ `ort` クレート | **デフォルト。設定ファイルで変更可能（§5.4）**。dense 1024 次元・CLS プーリング・L2 正規化 |
| モデル取得 | `hf-hub` クレート | 初回実行時に Hugging Face からキャッシュへ DL |
| MCP | `rmcp` 3.0（Rust 公式 SDK） | stdio 標準、npm の `npx` 経由 |
| CLI | `clap` 4 | JSON 出力対応 |
| 非同期 | `tokio` | |
| シリアライズ | `serde`, `schemars` | 入出力型の JSON Schema 生成 |
| テキスト抽出 | `html2text` / `pdf-extract` 等 | §12 参照 |
| npm 配布 | npm パッケージ + プラットフォーム別 optionalDependencies | §13 参照 |

### 3.1 採用技術に関する既知の注意点（検証項目）

- **トークナイザ**: gigatoken は crates.io 未公開・nightly Rust 必須（`portable_simd`, `profile-rustflags`）・pyo3 依存の重さによりビルドできず非採用（2026-07-29 検証）。`tokenizers` クレートを採用。`Tokenizer` トレイトでの抽象化により将来的な差し替えは可能。
- **ONNX Runtime**: `ort` 2.0-rc の配布設定では、対象tripleごとに生成artifactの依存関係を検査して配布可否を判定する。Linux x64/arm64では`ldd`、macOS arm64では`otool -L`、Windows x64では依存DLL検査を行い、`libonnxruntime.so`、`.dylib`、`.dll`の要否と同梱有無を確認する。TLSは全経路でrustls（ring / aws-lc-rs）を使用し、OpenSSLへの動的依存はない。Linuxの実行時依存はglibc ≥ 2.38、libz、libzstd、libgcc_s、ca-certificates、Windowsの`/MD`バイナリはVisual C++ Redistributable for Visual Studio 2015--2022 (x64)を必要とする。各対象は証明書ストアを含むクリーン環境で起動し、npm配布artifactの検査結果をリリース記録に残す。
- **SurrealDB Response::take()**: surrealdb 3.x のレスポンス取得では `meta::id()` などの明示的な投影が必要。`id` / `document` の直接選択や `value` フィールドは避ける。

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
DEFINE FIELD metadata    ON document TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD created_at  ON document TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at  ON document TYPE datetime VALUE time::now();
DEFINE INDEX document_sha256 ON document FIELDS sha256 UNIQUE;

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
-- class トークナイザ + lowercase。ngram は BM25 の精度を低下させるため不使用
DEFINE ANALYZER skb_text TOKENIZERS class FILTERS lowercase;
DEFINE INDEX chunk_content_fts ON chunk
    FIELDS content FULLTEXT ANALYZER skb_text BM25;

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
DEFINE FIELD meta_value ON meta TYPE string;
DEFINE INDEX meta_key_unique ON meta FIELDS key UNIQUE;
-- 記録キー: schema_version / embedding_model / embedding_dimension /
--          embedding_max_input_tokens / tokenizer / tokenizer_source /
--          tokenizer_algorithm / tokenizer_fingerprint_schema /
--          tokenizer_fingerprint
```

### 4.2 冪等性・重複排除

- `document.sha256`（本文テキストの SHA-256）に UNIQUE 制約。同一内容の再アップロードは **既存ドキュメントの更新（upsert）** として扱い、チャンクを再生成する（`--force` / `force: true` で明示した場合のみ）か、デフォルトでは `skipped: true` を返して何もしない。

---

## 5. 取り込みパイプライン（Ingestion）

```
入力(ファイル/URL/テキスト/stdin/base64)
  → ① 取得・形式判定          … MIME/拡張子で分岐
  → ② テキスト抽出            … プレーンテキスト化（Markdown構造は保持）
  → ③ チャンク化              … tokenizers でトークン単位分割
  → ④ 埋め込み                … bge-m3 (ONNX) で dense ベクトル化
  → ⑤ 保存                    … document + chunk をトランザクションで保存
  → ⑥ グラフ構築              … エンティティ抽出（ルールベース）→ RELATE
```

### 5.1 チャンク化（tokenizers）

- トークナイザ: **BAAI/bge-m3 と同一の tokenizer.json**（XLM-RoBERTa 系）を HuggingFace 公式の `tokenizers` クレートでロードし、埋め込みモデルの語彙と一致したトークン数で分割する。
- 既定値: `max_tokens = 512`、`overlap_tokens = 64`。いずれも **設定ファイルで変更可能**（§5.4）。`max_tokens` は使用中モデルの最大入力トークン数以下であれば任意に設定できる。
- 分割方針: 見出し・段落・文境界を優先しつつ、`max_tokens` を超えない範囲で結合。どうしても超える場合はトークン境界でハード分割。
- `token_count` は `tokenizers` のエンコード結果から実測して記録する。

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
| 見出し階層 | 章題を `entity(kind="section")` 化 | `related_to(relation="part-of")`。**不変条件**: 再取り込み後も重複エッジが発生せず、各セクション（child）の親は最大 1 件（single-parent invariant） |
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

1. `0 ≤ overlap_tokens < max_tokens ≤ max_input_tokens` を検証。違反時は `E_VALIDATION`。
2. `meta` テーブルに記録された `embedding_model` / `embedding_dimension` と設定値を比較。**不一致のまま通常操作は行わず** `E_MODEL_MISMATCH` を返し、再構築（`reindex`）を案内する。これにより、異なる次元・語彙のベクトルが同一インデックスに混在することを防ぐ。
3. `embedding.tokenizer` は明示パスと `"auto"`（`embedding.model` に対応する tokenizer.json の解決）のどちらでも、同じ tokenizer 識別子契約を適用する。tokenizer.json の存在と形式を検証し、取得元（モデル ID と revision、または明示パス）、`tokenizers` のアルゴリズム/バージョン、vocabulary、normalizer、pre-tokenizer、post-processor、decoder、その他の構成情報を canonical JSON serialization した上で SHA-256 fingerprint を生成する。fingerprint には schema version を含め、canonicalization 規則と対象フィールドを変更する場合は schema version を更新する。解決した tokenizer の取得元、アルゴリズム/バージョン、fingerprint schema version、fingerprint を `meta` に保存し、既存値と不一致の場合は `E_MODEL_MISMATCH` を返して再構築（`reindex`）を案内する。新規作成時・reindex 完了時には、明示パスと `auto` の両方を含むすべての解決経路で同じ tokenizer metadata を保存する。

#### 変更手順（reindex）

1. 設定ファイルを編集（モデル / チャンク長）。
2. `skb reindex`（MCP: `skb_reindex`）を実行:
   - モデル変更時: 新モデル・新トークナイザをロード。次元が変わる場合は `chunk.embedding` フィールドと HNSW インデックスを新次元で再定義。
   - 全ドキュメントの `document.content`（抽出済み全文）を新設定で再チャンク化 → 再埋め込み → `chunk` を置換（ドキュメント単位のトランザクション）。対象チャンクの `mentions` エッジを削除し、新チャンクから再構築する。
   - 旧チャンクとその `mentions` エッジの削除、新チャンク作成、エンティティ索引を同一トランザクションで実行する。チャンクIDの取得、削除、作成、索引のいずれかが失敗した場合は `E_DB` を返し、対象ドキュメントの変更をロールバックする。
   - `meta` テーブルを新モデル情報で更新。
3. チャンク長のみの変更も、既存ドキュメントへの反映には同じ `reindex` が必要（新規アップロード分には即時反映される）。

> **依存更新時の fingerprint 互換性（§5.4 規則 3 の運用）**: `tokenizers` クレートまたは
> serde_json 等のシリアライザを更新し、同じ `tokenizer.json` に対する canonical JSON 出力が
> 変わった場合、schema version（`TOKENIZER_FINGERPRINT_SCHEMA`）が同一でも fingerprint は
> 変化し `E_MODEL_MISMATCH` になる。fingerprint には `tokenizers` のバージョン（`tokenizer_version`）
> も含まれるため、`tokenizers` の**メジャー/マイナー更新に限らずパッチ更新も含む任意の**
> バージョン更新は fingerprint を変え得る。
> 対応手順: (a) 影響のない変更か fingerprint 差の確認、(b) canonicalization 規則や対象フィールドを
> 変えた場合のみ `TOKENIZER_FINGERPRINT_SCHEMA` を更新、(c) 利用者は `skb reindex` を実行する。
> `tokenizer_version` / `tokenizer_fingerprint_schema` は `meta` に保存されるため `skb doctor`
> で確認できる。

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
    pub async fn open(config: Config) -> Result<Self>;
    // モデル/次元/tokenizer 不一致でも開く。reindex による再構築専用（§9-5）
    pub async fn open_for_reindex(config: Config) -> Result<Self>;

    // 資料管理
    pub async fn upload(&self, req: UploadRequest) -> Result<UploadResult>;
    pub async fn list_documents(&self, q: &ListQuery) -> Result<Vec<DocumentSummary>>;
    pub async fn get_document(&self, req: &GetDocumentRequest) -> Result<DocumentDetail>;
    pub async fn delete_document(&self, req: &DeleteDocumentRequest) -> Result<DeleteResult>;

    // 検索
    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse>;

    // グラフ
    pub async fn graph_query(&self, req: &GraphQueryRequest) -> Result<GraphQueryResult>;
    pub async fn upsert_entity(&self, entity: &EntityInfo) -> Result<()>;
    pub async fn link_entities(&self, link: &LinkInfo) -> Result<()>;

    // 管理
    pub async fn stats(&self) -> Result<Stats>;
    pub async fn doctor(&self) -> Result<DoctorReport>;  // 環境診断
    pub async fn reindex(&self, req: &ReindexRequest, progress: Option<&ProgressFn>) -> Result<ReindexResult>; // モデル/チャンク設定変更の全件反映（§5.4）
}
```

- すべての Request/Response 型は `Serialize`/`Deserialize`/`JsonSchema` を derive し、**CLI の JSON 入出力と MCP ツールスキーマの双方をこの型から生成**する。
- 非同期（`tokio`）。長時間処理（reindex）は進捗コールバック（`ProgressFn`）を受け取り、MCP では progress notification、CLI ではプログレス出力へ写像する。

上記は v1 の目標API契約である。実装進捗は IMPLEMENTATION_PLAN.md を参照すること。

### 7.2 設定

読み込み優先順位: フラグ/引数 > 環境変数（`SKB_*`）> プロジェクト `./skb.toml` > ユーザ `~/.config/skb/config.toml`。

```toml
[storage]
mode = "embedded"                    # v1 は embedded（SurrealKV）のみ
path = "~/.local/share/skb/db"       # embedded: SurrealKV データディレクトリ
namespace = "skb"
database = "knowledge"

[embedding]
model = "BAAI/bge-m3"                # Embedding モデル（HF ID or ローカルパス）。変更はこのキーを編集（§5.4）
onnx_path = "auto"                   # "auto"=HFキャッシュ / 明示パス可
tokenizer = "auto"                   # "auto"=モデルに追随（tokenizers でロード）/ 明示指定も可
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

リリース用の `ort` 有効バイナリでは `onnx_path = "auto"` により bge-m3 を既定とする。`onnx_path = "mock"` はテスト・開発用の明示設定であり、`ort` featureなしの高速ビルドで使用する。

---

## 8. MCP サーバー仕様（`skb-mcp`）

### 8.1 基本仕様

| 項目 | 内容 |
|---|---|
| 実装 | Rust バイナリ `skb-mcp`（`rmcp` 使用） |
| トランスポート | stdio（唯一。HTTP は将来拡張） |
| 起動方法 | `npx surreal-knowledge-base` / `bunx surreal-knowledge-base` / バイナリ直接実行 |
| ログ | **stderr のみ**（stdio 運用時に stdout を汚染しない） |
| 終了コード | 0 正常 / 1 起動失敗 |

v1 では stdio トランスポートのみを提供する。HTTP トランスポートは将来拡張とし、v1 の機能パリティおよび配布検証の対象外とする。

### 8.2 ツール一覧

全ツールの入出力は `skb-core` の型から生成した JSON Schema に従う。

| # | ツール名 | 概要 | 主要パラメータ |
|---|---|---|---|
| 1 | `skb_upload` | 資料をアップロード | `path?`, `url?`, `content?`, `content_base64?`, `title?`, `tags?`, `metadata?`, `force?` |
| 2 | `skb_search` | 検索 | `query`, `mode?`（既定は `config.search.default_mode`。出荷時設定値は hybrid であり、設定変更時は hybrid へのフォールバックではない）, `top_k?`（既定は `config.search.top_k`、範囲1〜1000、超過は `E_VALIDATION`）, `filter?`, `graph_expand?=0`（0〜5、超過は `E_VALIDATION`） |
| 3 | `skb_list_documents` | 一覧 | `limit?=50`, `offset?=0`, `order?=created_desc`（`created_desc` / `created_asc` / `title_asc` / `title_desc`） |
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
  一覧・本文・統計の JSON シリアライズに失敗した場合は MCP の内部エラーを返す。存在しない URI は resource-not-found エラーとする。
- **Prompts**: `skb-answer` — `question` は任意。未指定または空の場合はローカル知識ベースを使って回答する既定の指示を生成する。指定時はその質問を使い、`skb_search` の結果を根拠に回答し、各引用に `document_id` と `chunk_idx` を含める。

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
skb query <surql>                    # 上級者向け。MCP では非公開
skb stats
skb reindex [--dry-run]             # モデル/チャンク設定変更の全件反映（§5.4）
skb config init | show | set <key> <value>
 npx surreal-knowledge-base              # MCP server（stdio; bunx も可）
skb doctor                          # DB・モデル・トークナイザの疎通診断
```

### 9.2 入出力規約

| 項目 | 規約 |
|---|---|
| 出力形式 | `--format json`（既定）/ `--format table`（人間向け） |
| 成功時 | stdout に結果 JSON、終了コード 0 |
| 失敗時 | `--format json` は stdout に `{"error":"E_*","message":"..."}`、終了コードはエラー種別に対応（2〜10）。非JSON形式は stderr |
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
3. **契約テスト（目標）**: 同一リクエスト JSON を MCP ハンドラ経由と CLI 経由の双方に投入し、レスポンスの JSON 一致を検証するゴールデンテストを CI で実行する。実装状況と不足するケースは `IMPLEMENTATION_PLAN.md` の Phase 9-6 に記載する（§16）。
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

対象ターゲット: `linux-x64-gnu`, `linux-arm64-gnu`, `darwin-arm64`, `win32-x64`。

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
- libstdc++ はビルド時に静的リンクし、libgcc_s は動的依存とする（Linux CI では `CXXSTDLIB=""` と `-C link-arg=-l:libstdc++.a` を使用）。
- モデル（bge-m3 ONNX ~2GB）は **npm には同梱せず** 初回起動時に HF から DL。

**Linux ランタイム要件**:

| 依存 | 理由 |
|---|---|
| glibc ≥ 2.38 | ビルドランナー（ubuntu-24.04）と ORT prebuilt の ABI フロア |
| libz | ORT prebuilt 静的ライブラリの動的依存 |
| libzstd | 同上 |
| libgcc_s | Rust の unwinding / GCC runtime（Linux ビルドで動的リンク） |
| ca-certificates | hf-hub の TLS 証明書検証（ureq 側は webpki-roots 埋め込み済み） |

macOS は OS 付属以外の動的依存なし。Windows の `/MD` バイナリは、実際のビルドで
`MSVCP140.dll`、`MSVCP140_1.dll`、`VCRUNTIME140.dll`、`VCRUNTIME140_1.dll`
を import する。これらは Microsoft Visual C++ Redistributable for Visual Studio 2015--2022
(x64) に含まれるため、npm パッケージには DLL を同梱せず、利用者が実行前に Redistributable
をインストールする責任を負う。リリース手順と実行時エラーメッセージでは、この前提と
Microsoft の公式インストーラを明示する。

### 13.5 実行方法

```bash
npx  surreal-knowledge-base          # npm 経由で MCP サーバー起動（stdio）
bunx surreal-knowledge-base          # bun 経由
```

---

## 14. エラーハンドリング・ロギング

### 14.1 エラーモデル（共通）

```jsonc
{ "error": "E_DOCUMENT_NOT_FOUND", "message": "…" }
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
| `E_TOKENIZE` | トークナイズ/チャンク化失敗 | 10 |

- MCP 側はツール結果として `{ isError: true, content: [text "[E_*] ..."] }` を返す（プロトコルエラーにはしない。起動不能時のみプロトコルエラー）。
- ログ: `tracing` + `RUST_LOG`。MCP は **stderr のみ**、CLI は stderr。機微情報（本文・パス）のログ出力は `debug` レベル以下に限定。

---

## 15. セキュリティ

- MCP 経由の `path` アップロードは `upload.allowed_dirs` 設定時にディレクトリ外参照を拒否（パストラバーサル対策）。
- 生 SurrealQL は MCP には公開しない（CLI のみ・明示コマンド）。
- URL 取得は HTTP(S) のみ許可、リダイレクト上限・サイズ上限を設定。`file://` 等のスキームは拒否（SSRF 緩和）。
- 将来リモート接続を追加する場合は、認証情報を設定ファイルのパーミッション 600 と環境変数で保護する。

---

## 16. テスト計画

以下は v1 の目標テスト計画である。現在のテスト配置・実行範囲は `IMPLEMENTATION_PLAN.md` の検証マトリクスを正とし、未実装のテストは完了扱いにしない。

| 層 | 内容 | ツール |
|---|---|---|
| 単体 | チャンク化（`tokenizers` 実測 token_count と overlap の正しさ）、RRF、スキーマ CRUD | `cargo test` |
| 統合 | 組込み SurrealDB で upload → search → delete の一連動作。bge-m3 は小型ダミー or 実モデルの量子化版で CI 実行 | `cargo test --workspace -- --test-threads=1`（目標: 実モデルE2Eを追加） |
| 契約（パリティ） | §11.2-3。同一 JSON リクエストを MCP/CLI 両経路で実行し応答を比較 | ゴールデンファイル |
| E2E | `npm pack` したパッケージを `npx` / `bunx` で起動し、initialize→tools/list→skb_upload→skb_search が通ることを検証 | シェルスクリプト + CI |
| ベンチ | `tokenizers` のトークナイズ速度、Embedding スループット、検索レイテンシ | `criterion` |

### 16.1 性能目標（初版目標・要検証）

| 指標 | 目標 | 実測（20 コア, x86_64） |
|---|---|---|
| チャンク化 | 10MB テキストを 5 秒以内 | encode 10MB: 4.73 s / chunk(512,64) 10MB: 4.69 s（tokenizers v0.23, bge-m3 トークナイザ） |
| Embedding | 512 トークン × バッチ 32、8 コア CPU で 5 chunks/s 以上 | ort_bge_m3_batch32: 15.2 s（batch 32, ~500 tokens each, CPU）— 2.1 chunks/s, 目標 5 chunks/s 未達 |
| 検索 | 10 万チャンク規模で hybrid 検索 p95 < 500ms | 1,000 チャンク: hybrid 10.6 ms, vector 1.07 ms, keyword 8.78 ms（mock 埋め込み, 10 万チャンクは未実測） |
| MCP 起動 | コールドスタート 3 秒以内 | **4.76 s**（mock 設定, tokenizer キャッシュ済み。実装が eager ロードのため目標超過） |

> 計測環境: Linux x86_64, 20 コア CPU (AVX2), 31 GB RAM, Rust 1.97.1, criterion 0.5。
> ort 実埋め込みベンチマーク: `cargo bench --features ort -p skb-core --bench skb`（rustls-only, `pkg-config`/`libssl-dev` 不要）。
> ONNX Runtime (ort 2.0.0-rc.13, tls-rustls build-dep) + bge-m3 ONNX model (HF auto download, ~2.2 GB).

---

## 17. ディレクトリ構成

```
surreal-knowledge-base/
├── Cargo.toml                    # workspace
├── crates/
│   ├── skb-core/                 # 共有コア（本仕様の実体）
│   │   └── src/{config,db,ingest,embed,tokenize,search,graph,error}.rs
│   ├── skb-cli/                  # CLI（clap）
│   │   └── tests/contract.rs     # 現在のCLI契約テスト
│   ├── skb-mcp/                  # MCP サーバー（rmcp）
│   └── skb-server/               # HTTP APIサーバー（axum、§20）
│       ├── src/{api,auth,config,error,llm}.rs
│       ├── src/dto/              # OpenAPI用サーバー所有DTO
│       ├── src/handlers/         # documents / search / graph / chat / auth / blog
│       ├── schema/002_server.surql  # サーバー所有DDL（user / blog_post、起動時冪等適用）
│       ├── examples/mock_llm.rs  # モックOpenAI互換LLMサーバー（E2E用）
│       ├── tests/                # 統合 / API E2E / TLSガード
│       └── SPIKE.md              # マルチプロセスDBロックスパイク（§20.1）
├── npm/
│   ├── package.json              # メタパッケージ
│   ├── bin/skb-mcp.js            # ラッパ
│   └── packages/                 # プラットフォーム別パッケージ
├── web/                          # フロントエンド（本プランで追加）
│   ├── apps/{vault,studio,blog}/ # 3 SPA（Vite + React）
│   └── packages/{api-client,ui}/ # OpenAPI型生成クライアント + 共通UI
├── skills/
│   └── surreal-knowledge-base/
│       └── SKILL.md
├── schema/
│   └── 001_init.surql            # §4.1 のマイグレーション（次元数は初期化時に設定値から埋め込むテンプレート）
├── .github/workflows/            # build / test / npm publish
└── SPECIFICATION.md              # 本書
```

MCP/CLIのゴールデン契約テストとnpm E2E用の専用ディレクトリは、Phase 9-6/9-7で追加する。

---

## 18. マイルストーン

| MS | 内容 | 完了条件 | 状態 |
|---|---|---|---|
| M1 | `skb-core` + CLI（upload/search/list/get/delete/stats、組込み DB） | CLI 統合テスト緑 | 部分完了 |
| M2 | `skb-mcp` + npm パッケージ化（linux-x64/darwin-arm64 先行） | `npx`/`bunx` E2E 緑 | 部分完了 |
| M3 | Skill 整備 + 契約テストによるパリティ CI 化 | マトリクス全項目 ✅ | 部分完了 |
| M4 | グラフ強化（抽出ルール拡充、グラフ拡張検索の再ランク精度評価） | 評価レポート | 未着手 |
| M5 | 性能チューニング・docx 対応（v2 スコープ判断） | 性能目標達成 | 未着手 |

---

## 19. 未決事項・リスク

| # | 項目 | 影響 | 対応方針 |
|---|---|---|---|
| 1 | `tokenizers` と bge-m3 tokenizer.json の互換性・実測性能 | チャンク化の速度と token_count の正確性 | M1 でベンチと互換性テストを実施。問題時は Tokenize トレイトの差し替え実装を検討 |
| 2 | ORT 静的リンクによる npm パッケージサイズ増 | 配布 | CPU 版最小構成で静的リンク。超過時は postinstall ダウンロード方式を検討 |
| 3 | SurrealDB FTS の日本語品質 | keyword/hybrid 精度 | `class` + lowercase を採用。ngram は BM25 の単語・複合語精度を低下させるため不使用 |
| 4 | bge-m3 初回 DL のサイズ（約 2GB 級） | 初回 UX | `skb doctor` で進捗表示付き事前 DL を案内。量子化版 ONNX の採用も検討 |
| 5 | PDF 抽出クレートの選定 | v1 スコープ | M1 で `pdf-extract` を検証し不十分なら代替選定 |

---

## 20. HTTP APIサーバー仕様（`skb-server`）

本章はフロントエンド実装仕様書に基づく HTTP API サーバー `skb-server` の実装仕様を定める。ルート定義と OpenAPI ドキュメントの生成は `crates/skb-server/src/api.rs` が正であり、`GET /api/openapi.json`（Swagger UI は `/swagger-ui`）で機械可読な契約を提供する。フロントエンド（`web/`）はこの OpenAPI JSON から型を生成して呼び出す。

### 20.1 プロセスモデルとDB所有権（スパイク結果）

SurrealDB は組込みモード（SurrealKV）で動作するため、DB ファイルの所有者は 1 プロセスに限定される。マルチプロセススパイク（`crates/skb-server/SPIKE.md`、証跡 `target/evidence/01/`）の結果:

- SurrealKv は `<db-path>/LOCK` によるクロスプロセス排他ロックを持つ。同一 `storage.path` への同時オープンは必ず 1 プロセスのみが成功し、敗者はオープン時点で即座に `E_DB`（"LOCK is already locked by another process"、終了コード 3）で失敗する。ブロックもリトライも破損も発生しない。
- このため **サーバープロセスが単一の DB 所有者** である。`skb-server` は起動時に `KnowledgeBase::open` を 1 回だけ呼び、プロセス生存中は保持し続ける。全 HTTP ハンドラはこの 1 インスタンスを共有し、サーバーはパスを再オープンしない。
- **サーバー起動中は `skb` CLI / `skb-mcp` が同一 `storage.path` を開いてはならない**。オープンは即座に `E_DB` で失敗する。安全な読み取り専用の同時アクセスモードは存在しない。サーバー停止後は CLI/MCP がスタンドアロンで開いてよい。
- リクエスト処理中に DB オープンエラーが発生した場合（外部プロセスがロックを奪った等）は `E_DB` 系 → HTTP 500 に写像される（サーバー側のストレージ障害でありクライアントエラーではない）。リトライループは持たず、オペレーターが所有権の競合を解消する。
- 既知のクセ（スパイク case 3/4、現状受け入れ）: パス要素が通常ファイルで占有されている場合は起動時に `E_DB`。親ディレクトリが存在しない場合は `create_dir_all` により **黙って自動作成される** ため、`skb.toml` の `storage.path` の打ち間違いは起動時に検出されず、誤った場所に新しいストアが生成される。

### 20.2 エンドポイント一覧

認証は `Cookie: skb_session=<JWT>`（優先）または `Authorization: Bearer <JWT>`。「公開」は認証不要を意味する。

| メソッド | パス | 認証 | 成功ステータス | 概要 |
|---|---|---|---|---|
| GET | `/api/health` | 公開 | 200 | Liveness プローブ（DB に触れない） |
| GET | `/api/openapi.json` | 公開 | 200 | OpenAPI ドキュメント（utoipa 生成） |
| GET | `/swagger-ui` | 公開 | 200 | Swagger UI |
| POST | `/api/documents` | 公開（注1） | 201 | 取り込み。`metadata.app == "blog"` の場合は author JWT 必須 |
| GET | `/api/documents` | 公開 | 200 | 一覧。`limit` / `offset` / `order` / `after` カーソル |
| GET | `/api/documents/{id}` | 公開 | 200 | 詳細。`?include_chunks=true` でチャンク付き |
| PUT | `/api/documents/{id}` | 公開（注3） | 200 | 内容差し替え（複合操作、本文参照）。レスポンス `{"document_id"}` |
| DELETE | `/api/documents/{id}` | 公開（注3） | 204 | 削除（レスポンスボディなし） |
| GET | `/api/documents/{id}/backlinks` | 公開 | 200 | 逆方向 mentions ウォークによるバックリンク（`{documents: [{id, title}]}`） |
| POST | `/api/search` | 公開 | 200 | 検索（既定 hybrid、透過パススルー）。`{hits, mode, elapsed_ms}` |
| POST | `/api/search/expand` | 公開 | 200 | 検索ヒットのグラフ拡張。`{hits, entity_origins}`（§20.6-4 の既知制限あり） |
| POST | `/api/graph/query` | 公開 | 200 | グラフ照会。`depth` 1〜5（範囲外は 400）。`{nodes, edges}` |
| POST | `/api/chat/stream` | 公開 | 200 | SSE チャット（§20.3） |
| POST | `/api/auth/register` | 公開（注4） | 201 | ユーザー登録（Argon2id）。email 重複は 409（同時登録の競合も 409） |
| POST | `/api/auth/login` | 公開 | 200 | ログイン。`Set-Cookie: skb_session=<JWT>` |
| GET | `/api/blog/posts` | 公開 | 200 | 公開済み（`published = true`）投稿のみを新着順で返す |
| POST | `/api/blog/posts/{document_id}/publish` | author | 200 | 公開フラグの設定。author ロール必須 |

注1: `metadata.app == "blog"` を含む `POST /api/documents` は author JWT を要求する（トークン無し・無効・reader ロールは 401、`SKB_SERVER_JWT_SECRET` 未設定は 503）。それ以外のアップロードは認証不要のままである。blog アップロード成功時は `blog_post` レジストリ行（`published = false`、author は JWT の email から解決）が自動作成される。

注3: `blog_post` レジストリ行を持つ document の PUT / DELETE は **その投稿の author 本人のみ** が実行できる（トークン無し・無効・reader ロールは 401、別 author は 403）。レジストリ行の存在が blog 判定の唯一の根拠であり、`metadata.app` は目安に過ぎない（後続 PUT で欠落し得る）。`blog_post` を持たない document（通常の KB 編集、Vault の autosave 含む）は公開のままである。

注4: 登録時のロールはクライアントが選べない。`SKB_SERVER_AUTHOR_EMAILS`（カンマ区切り）に含まれる email は `author`、それ以外は `reader` として登録される。未認証入力から author 権限を自己付与できる経路は存在しない。

注2: `/api/blog/*` はフロントエンド実装仕様書の §API設計表に無い **追加エンドポイント** であり、同書 §Blog の公開範囲管理（`published` フラグ + reader/author ロール）を実現するために設けた。サーバー所有スキーマは `crates/skb-server/schema/002_server.surql`（`user` / `blog_post` テーブル、起動時に冪等適用）。

**PUT の複合動作**: コアに更新 API が無いため、旧ドキュメント取得（404）→ `force` を剥離した 1 回の upload → 応答分岐として実装する。内容が変わった場合（新 id 発行）は `blog_post` 行（レジストリ存在ベースで判定、注3）を新 id へ移行した上で旧ドキュメントを削除し、新 id を返す。同一内容（sha256 一致で `skipped`）の場合は旧ドキュメントを保持し旧 id を返す（ここで削除すると唯一のコピーが失われる）。`force` は PUT では常に無視される。クライアントはレスポンスの `document_id` へ保存済み参照を張り替えること。

**複合操作の補償（compensation）**: コアにレジストリとドキュメントを跨ぐトランザクションは無いため、各複合操作は失敗時に補償して再試行可能な状態へ戻す。(1) blog upload 後のレジストリ作成失敗 → 取り込んだ document を削除してエラー。(2) PUT 中のレジストリ移行失敗 → 後継 document を削除してエラー（旧 document は無傷）。(3) DELETE でレジストリ削減後の document 削除失敗 → 既知の author / published フラグでレジストリ行を復元してエラー。唯一の不可逆ステップ（移行成功後の旧 document 削除）は、レジストリが既に新 id を指すため論理状態は正しく、警告ログとともに成功応答を返す。

**DELETE の blog_post 後片付け**: ドキュメント削除時は `blog_post` 行を無条件に先に削除する。PUT 移行後の後継ドキュメントが `metadata.app=blog` を持たない場合があり、メタデータマーカーは存在判定として信頼できないためである（インデックスバック付きの DELETE で、投稿を持たないドキュメントに対しては no-op）。公開投稿一覧が削除済みドキュメントを参照し続けることを防ぐ。削除対象が `blog_post` を持つ場合は注3 の author 認可を先に要求する。

### 20.3 SSEイベント契約（`POST /api/chat/stream`）

リクエストボディは `{"message": string}`。レスポンスは `text/event-stream` で、イベント順序は次のとおり。

| イベント | データ | 回数 |
|---|---|---|
| `citation` | `{"hits": [SearchHit]}`。検索ヒット全件（`document_id` / `title` / `score` / `matched_entities` / `highlights` を含む） | 1 |
| `token` | `{"text": "..."}` | 0 以上 |
| `done` | `{}` | 1（正常終了時） |
| `error` | `{"code": "E_...", "message": "..."}` | 0 または 1（終端） |

- パイプライン: `kb.search`（top_k 6、`graph_expand` は `SKB_CHAT_EXPAND_DEPTH`）→ citation → ヒットチャンクからプロンプト構築（文字ベースのトークン予算、§20.4）→ LLM ストリーミング転送（`choices[0].delta.content`）→ done。
- **クライアント切断で即座に中止する**: 検索・LLM 接続・フラグメント待機の全ての長時間待機は切断チャネル（`tx.closed()`）と競合し、切断時はパイプラインのタスクと上流 LLM 接続を解放する。
- 失敗時は `event: error` を送ってストリームを正常終了する。**HTTP ステータスは常に 200**（SSE エラーは in-band であり、ストリームエラーとして送らない）。エラーコードはコアの `E_*` に加え `E_LLM_CONNECTION` / `E_LLM_STATUS` / `E_LLM_PROTOCOL` / `E_LLM_CONFIG`。
- keep-alive は axum の `KeepAlive::default()`（既定 15 秒間隔のコメント行）。
- `EventSource` は GET 専用のため使用できない。フロントエンドは `fetch` + `ReadableStream` でパースする専用 hook を `packages/api-client` に置く（フロントエンド実装仕様書 §API設計と同一の方針）。

### 20.4 設定

`skb.toml` の `[server]` テーブル（コア設定ローダーは未知キーを無視するため既存セクションと共存できる）:

```toml
[server]
host = "127.0.0.1"
port = 8080
```

リッスンアドレスの優先順位: CLI `--port` / `--host` > 環境変数 > `skb.toml [server]`。

| 環境変数 | 既定 | 意味 |
|---|---|---|
| `SKB_SERVER_HOST` / `SKB_SERVER_PORT` | toml 値（既定 127.0.0.1:8080） | リッスンアドレス。`SKB_SERVER_PORT` が数値でない場合は起動失敗（`E_CONFIG`） |
| `SKB_LLM_BASE_URL` | `http://localhost:11434/v1` | OpenAI 互換 LLM のベース URL（`{base}/chat/completions` に POST）。上流からの応答には防御上限がある: エラー本文 8 KiB、SSE 1 フレーム 64 KiB（超過は `E_LLM_PROTOCOL`）、フラグメント間 60 秒の read timeout |
| `SKB_LLM_MODEL` | `llama3.1` | チャットモデル |
| `SKB_LLM_API_KEY` | 未設定 | Bearer トークン（空文字は未設定扱い）。設定時は **`https://` の `SKB_LLM_BASE_URL` のみ許可**（HTTP URL はチャット要求が `E_LLM_CONFIG` で終端する。API キーとプロンプトの平文漏えい防止） |
| `SKB_CHAT_EXPAND_DEPTH` | 2 | チャット検索の `graph_expand` 深さ。上限 5（コア `MAX_GRAPH_EXPAND`、超過は切り詰め）、パース不能値は既定 |
| `SKB_CHAT_TOKEN_BUDGET` | 4000 | プロンプト全体の文字予算（固定指示文 + 質問文 + 引用断片の合計）。超過する質問文は文字境界で切り詰められ、引用断片は残り予算を共有する。文字ベースの近似（約 4 文字/トークン）。実トークナイザは意図的に導入しない |
| `SKB_SERVER_JWT_SECRET` | 未設定 | JWT 署名鍵（HS256、有効期限 24 時間）。**未設定でも起動は継続** し warning を出力する。JWT 検証を要するパス（login、publish、`app=blog` の POST /api/documents の author 必須分岐、blog document の PUT/DELETE）は 503 `E_CONFIG` を返す。register と公開 GET は影響を受けない。**32 文字未満の弱い secret も未設定と同等に 503** する（総当たり可能な鍵で HS256 トークンを偽造できないようにするため） |
| `SKB_SERVER_AUTHOR_EMAILS` | 未設定 | 登録時に `author` を付与する email のカンマ区切りリスト（注4）。未設定なら全員 `reader` で登録される |

LLM 系環境変数と JWT secret はリクエスト毎に読まれる（テストや E2E がプロセス再起動なしで向き先を変えられる）。

`--port 0` で起動するとエフェメラルポートをバインドした後、stdout に機械可読な 1 行 `SKB_SERVER_PORT=<n>` を出力する（バインド後出力のため、この行を受信した時点でポートは受付中である）。E2E ハーネスはこの行をパースする。モック LLM サーバー（`examples/mock_llm.rs`）も同一プロトコルで `MOCK_LLM_PORT=<n>` を出力する。ログは `tracing` により **stderr のみ**（既定フィルタ `skb_server=info,warn`、`RUST_LOG` で上書き）。終了は ctrl-c / SIGTERM でグレースフルに行う。

### 20.5 エラー → HTTP マッピング

ボディは常に `{"code": "E_...", "message": "..."}` である（§14.1 の CLI JSON 出力はキーが `error` である点が異なる。コード体系自体は共通）。

| コード | 既定ステータス |
|---|---|
| `E_VALIDATION` | 400 |
| `E_DOCUMENT_NOT_FOUND` | 404 |
| `E_UNSUPPORTED_FORMAT` | 415 |
| `E_DB` / `E_IO` / `E_CONFIG` / `E_EMBEDDING` / `E_TOKENIZE` / `E_MODEL_MISMATCH` | 500 |

ハンドラによる明示的な上書き:

| ステータス | 条件 |
|---|---|
| 409 | register で email 重複（コードは `E_VALIDATION`） |
| 401 | 認証失敗全般（トークン無し/無効/期限切れ、reader ロールによる author 操作、誤認証情報。コードは `E_VALIDATION`、ユーザー列挙防止のためメッセージは汎用） |
| 403 | `blog_post` を持つ document の PUT/DELETE を別 author が試みた場合（コードは `E_VALIDATION`） |
| 415 | 非対応ソース形式（`E_UNSUPPORTED_FORMAT`、既定マッピングと同一） |
| 503 | `SKB_SERVER_JWT_SECRET` 未設定（`E_CONFIG`） |

### 20.6 仕様差異（実装上の決定事項）

フロントエンド実装仕様書に対する、実装で確定させた差異とその理由:

1. **`SearchHit.score` は RRF 融合済みの単一スコア** である。個別の BM25 / ベクトルスコアは skb-core が非公開のため HTTP API でも提供しない。§Studio 参照パネルと §Vault Cmd+K の「BM25/ベクトルスコアを表示」は、この融合スコアの表示に置き換える。
2. **Studio 再帰取得の MVP 代替**: 回答生成中の citation 検出による再帰展開の代わりに、検索時の事前一括 `graph_expand`（`POST /api/search` の `graph_expand`、チャットは `SKB_CHAT_EXPAND_DEPTH`）で関連コンテキストを取得する。
3. **`query_surql` の server 内部利用**: `KnowledgeBase::query_surql` は「CLI 専用の escape hatch」と自己文書化されているが、サーバーは固定 SQL（`schema/002_server.surql` の DDL 適用）に限ってこれを使う。ユーザー入力を含むクエリ（backlinks / auth / blog）は生の db ハンドルでパラメータバインドし、文字列補間は行わない。
4. **`/api/search/expand` の拡張レグは現在無効（inert）**: skb-core の `expand_search_hits` が WHERE 句内の順方向グラフ走査（`FROM chunk WHERE ->mentions->entity.name IN ...`）を使っており、surrealdb 3.x ではこれが黙って 0 件に一致する（既存コアバグ、修正は上流待ち）。`entity_origins` は別の有効なステートメントで埋まるため応答自体は正常だが、拡張ヒットは空で返る。エンドポイントは透過パススルーとして正しく、フロントエンドはこのエンドポイントに依存しない。
5. **`after` キーセットカーソルのサーバー側エミュレーション**: コアの行値比較 SQL（`(created_at, meta::id(id)) < (...)`）は surrealdb 3.x でパースできないため、サーバーは順序付き走査（上限 10,000 件）+ スライスでカーソルを再現する。カーソルがどのドキュメントにも一致しない場合は 400 を返す（黙って誤ったページを返さない）。コア修正後はこのシムを削除する。
6. **検索ヒットの `document_id` は `document:<key>` の完全レコード形に正規化** する（サーバー DTO 境界で変換）。コアは素のキーを返すが、ドキュメント系エンドポイントは前置き付きの形しか受け付けない（素キーは 400）。これにより検索応答の id が全エンドポイントでそのまま使える。
7. **HTTP 経由では `path` アップロードを受け付けない**: `POST`/`PUT /api/documents` の DTO は `url` / `content` / `content_base64` のみ。サーバー側ファイル読み込みを外部入力から解放するとパス走査（`/etc/passwd` 等）になるため（CLI / MCP は引き続き `path` を保持する）。
8. **POST /api/search/expand の `max_expand` は API 境界で検証** する（コア `MAX_GRAPH_EXPAND` 超過は 400 `E_VALIDATION`。トラバーサル開始前に拒否する）。

### 20.7 MVPデスコープ

フロントエンド実装仕様書に記載がありながら、MVP では実装を見送った 2 項目:

1. **SPA 静的ホスト（ServeDir / SPA-fallback）**: MVP の動作確認は dev server で行う。本番配信時の静的ホスト実装は後続とする。
2. **§Blog 冒頭のグラフ可視化（ネットワーク図）**: MVP は関連記事データの取得（`POST /api/graph/query`、vector 検索）までを実装する。グラフ UI の描画は後続とする。

### 20.8 テスト方針

| レベル | 内容 | 実行 |
|---|---|---|
| 単体 | DTO 変換、エラー写像、設定の優先順位、SSE 行パース等 | `cargo test -p skb-server` |
| 統合 | in-process ルーターによる全エンドポイント。組込み DB を伴うため **直列実行必須** | `cargo test --workspace -- --test-threads=1` |
| API E2E | 実バイナリ spawn（`--port 0` + `SKB_SERVER_PORT=` 行パース）+ 生 HTTP。モック LLM（`examples/mock_llm.rs`）を含む | 同上（スイート内） |
| UI E2E | Playwright + mock_llm（`web/`、本プラン後半で追加） | `bunx playwright test` |

TLS ガード（`cargo tree -i openssl-sys` / `-i native-tls` の非ゼロ終了 = パッケージ不在）は `tests/tls_guard.rs` としてスイート内で常時実行される。検証証跡は `target/evidence/` 配下にタスク番号別に保存する。
