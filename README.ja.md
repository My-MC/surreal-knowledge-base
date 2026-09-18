# Surreal Knowledge Base

[![CI](https://github.com/My-MC/surreal-knowledge-base/actions/workflows/ci.yml/badge.svg)](https://github.com/My-MC/surreal-knowledge-base/actions/workflows/ci.yml)

埋め込み [SurrealDB](https://surrealdb.com/)（SurrealKV）と
[BAAI/bge-m3](https://huggingface.co/BAAI/bge-m3) によるハイブリッド検索（ベクトル +
BM25）と知識グラフを備えたローカルファースト知識ベース。MCP サーバー、CLI、HTTP API、3つの Web アプリケーションとして提供します。

> English version: [README.md](README.md)

## 特徴

- **ハイブリッド検索** — ベクトル検索（HNSW）+ キーワード検索（BM25）を RRF（Reciprocal Rank Fusion）で統合
- **知識グラフ** — 文書からのルールベースエンティティ抽出とリンク検索
- **マルチフォーマット** — Markdown、プレーンテキスト、PDF（テキスト抽出）
- **冪等アップロード** — SHA-256 ハッシュによる重複排除
- **チャンク設定可変** — 最大トークン数・オーバーラップ量を設定可能
- **埋め込み専用** — SurrealKV ストレージエンジン、外部サービス不要
- **再インデックス** — 埋め込みモデルやチャンク設定の変更を既存データに適用
- **MCP サーバー** — 10 ツール（Claude Desktop / opencode / 任意の MCP クライアント対応）
- **CLI** — `skb` コマンドで全機能を提供（MCP と同等の機能パリティ）
- **HTTP API** — OpenAPI を備えた文書・検索・グラフ・SSE チャット・認証・ブログ API
- **Web アプリケーション** — 編集用 Vault、引用付き RAG チャット用 Studio、知識公開用 Blog
- **実埋め込み** — BAAI/bge-m3 ONNX Runtime 推論（オプション、`ort` feature）

## 構成

```
crates/
├── skb-core/    コアライブラリ（DB、埋め込み、トークン化、検索、グラフ、取り込み）
├── skb-cli/     CLI バイナリ（skb）
├── skb-mcp/     MCP サーバーバイナリ（skb-mcp）
└── skb-server/  HTTP API サーバー（skb-server）
npm/             npm メタパッケージ + プラットフォーム別パッケージ
web/             Vault、Studio、Blog、共有 API クライアント、UI を含む Bun workspace
schema/          SurrealDB マイグレーション（001_init.surql）
skills/          opencode エージェント Skill
```

## 必要条件

- [Rust](https://rustup.rs/) 1.88+
- 実埋め込み（`--features ort`）の場合：初回ビルド時に ONNX Runtime を自動ダウンロード（約 15 分、`~/.cache/ort.pyke.io` にキャッシュ）
- CLI/MCP/HTTP サーバーの初回実行時（mock 埋め込みでも）：tokenizer.json を Hugging Face から自動ダウンロード（約 17 MB、`~/.cache/huggingface` にキャッシュ）
- Web アプリケーション: [Bun](https://bun.sh/) 1.4+

## クイックスタート

### ビルド

```bash
# モック埋め込みによる高速ビルド（テスト・開発用）
cargo build

# 実 BAAI/bge-m3 埋め込み（本番用）
cargo build --release -p skb-mcp --features ort

# 実 BAAI/bge-m3 埋め込みを使う HTTP API（本番用）
cargo build --release -p skb-server --features ort
```

### 設定

`skb.toml` を作成（カレントディレクトリ → `~/.config/skb/config.toml` の順に探索）：

```toml
# モック埋め込み（高速、GPU 不要、決定論的出力）
[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "./skb-data"

[server]
host = "127.0.0.1"
port = 8080

# 実埋め込み（BAAI/bge-m3）
# [embedding]
# model = "BAAI/bge-m3"
# onnx_path = "auto"
# [storage]
# path = "~/.local/share/skb/db"
```

### CLI

```bash
# 文書をアップロード
skb upload README.md --title "README"

# URL からアップロード
skb upload --url https://example.com/doc.md --tags "docs,example"

# 標準入力からアップロード
cat notes.txt | skb upload --stdin --title "ミーティングメモ"

# 検索（hybrid = ベクトル + キーワード）
skb search "ベクトルデータベース" --mode hybrid --top-k 10

# 文書一覧
skb list --limit 20

# 文書詳細
skb get <doc-id>

# 文書削除
skb delete <doc-id> --yes

# 統計情報
skb stats

# 診断
skb doctor
```

### MCP サーバー

開発時はリポジトリルートから stdio サーバーを起動します：

```bash
cargo run -p skb-mcp --bin skb-mcp
```

#### OpenCode の設定

`/absolute/path/to/surreal-knowledge-base` はリポジトリの絶対パスに置き換えてください。

```jsonc
{
  "mcp": {
    "surreal-knowledge-base": {
      "type": "local",
      "command": [
        "cargo", "run",
        "--manifest-path", "/absolute/path/to/surreal-knowledge-base/Cargo.toml",
        "-p", "skb-mcp", "--bin", "skb-mcp"
      ],
      "enabled": true
    }
  }
}
```

#### Claude Desktop の設定

```jsonc
{
  "mcpServers": {
    "surreal-knowledge-base": {
      "command": "/absolute/path/to/cargo",
      "args": [
        "run",
        "--manifest-path", "/absolute/path/to/surreal-knowledge-base/Cargo.toml",
        "-p", "skb-mcp", "--bin", "skb-mcp"
      ]
    }
  }
}
```

### HTTP API サーバー

`skb-server` は実行中に組み込み DB を所有します。稼働中は同じ `[storage].path`
に対して CLI や MCP サーバーを起動しないでください。

```bash
# 上記の mock 設定なら ort feature なしで起動できます。
cargo run -p skb-server --bin skb-server -- --port 8080

# または実埋め込みを使う release ビルドを起動します。
./target/release/skb-server --port 8080
```

API は `http://127.0.0.1:8080` で利用できます。生成された契約は
[`/api/openapi.json`](http://127.0.0.1:8080/api/openapi.json)、操作画面は
[`/swagger-ui`](http://127.0.0.1:8080/swagger-ui) で確認できます。チャット API は
OpenAI 互換 LLM から回答をストリーミングします。サーバーと LLM の設定は
[SPECIFICATION.md](SPECIFICATION.md) を参照してください。

### Web アプリケーション

最初に `skb-server` を起動し、続いて Bun workspace の依存をインストールします。

```bash
cd web
bun install
```

`web/` からアプリケーションを起動します。各 Vite 開発サーバーは `/api` を
`SKB_SERVER_PORT`（既定は `8080`）へプロキシします。

```bash
bun --filter @skb/vault dev   # Markdown ナレッジベースエディタ
bun --filter @skb/studio dev  # 引用付き RAG チャット
bun --filter @skb/blog dev    # 公開知識ブログと著者向け投稿フロー
```

| アプリケーション | 用途 |
|---|---|
| Vault | Markdown 文書の閲覧、編集、自動保存、検索、グラフ連携。 |
| Studio | ナレッジベースへの質問と、引用を伴うストリーミング回答の確認。 |
| Blog | 公開済み文書の閲覧。招待された著者はサインイン、投稿、公開が可能。 |

### Docker Compose デプロイ

Docker Compose では、ホスト側に Rust や Bun をセットアップせず HTTP API と各静的
Web アプリケーションを実行できます。Vault、Studio、Blog には個別のサービスと
Dockerfile があり、それぞれを独立して起動できます。API は引き続き組み込み
SurrealKV を所有し、`skb-data` named volume によりコンテナの再作成後もデータを保持します。

```bash
docker compose up --build
```

Vault は `http://127.0.0.1:3000`、Studio は `http://127.0.0.1:3001`、Blog は
`http://127.0.0.1:3002`、API は `http://127.0.0.1:8080` で利用できます。各
フロントエンドは同一オリジンの `/api` リクエストを API コンテナへリバースプロキシします。

```bash
docker compose up vault
docker compose up studio
docker compose up blog
```

既定の Compose イメージは mock embedder
(`SKB_EMBEDDING_ONNX_PATH=mock`) を使います。実際の BAAI/bge-m3 埋め込みを
使う場合は、`ort` を有効にした API イメージをビルドし、対応するモデル設定へ変更
してください。コンテナだけでなく永続 DB データも削除するには
`docker compose down --volumes` を実行します。

## CLI コマンド一覧

| コマンド | 説明 |
|---|---|
| `skb upload <FILE>` | ファイルをアップロード（`--recursive`、`--metadata JSON`、`--force`） |
| `skb upload --url <URL>` | URL からアップロード |
| `skb upload --stdin` | 標準入力からアップロード |
| `skb search <QUERY>` | 検索（`--mode hybrid\|vector\|keyword --top-k N --filter KEY=VALUE`） |
| `skb list` | 文書一覧（`--limit N --offset N --order ...`） |
| `skb get <ID>` | 文書詳細（`--chunks`） |
| `skb delete <ID>` | 文書削除（`--yes`） |
| `skb stats` | 統計情報 |
| `skb graph query <ENTITY>` | 知識グラフ検索 |
| `skb graph entity <NAME> --kind <KIND>` | エンティティ追加・更新 |
| `skb graph link <FROM> <TO>` | エンティティ間リンク |
| `skb reindex` | 全文書を再インデックス（`--dry-run` 対応） |
| `skb config init\|show\|set` | 設定管理 |
| `cargo run -p skb-mcp --bin skb-mcp` | 開発用 MCP サーバー起動 |
| `skb doctor` | 診断実行 |

全コマンドで `--format json` を指定すると構造化出力になります。

## MCP ツール一覧

| ツール | 説明 |
|---|---|
| `skb_upload` | 文書アップロード（path, url, content, content_base64） |
| `skb_search` | 文書検索（hybrid, vector, keyword） |
| `skb_list_documents` | 全文書一覧 |
| `skb_get_document` | 文書詳細取得 |
| `skb_delete_document` | 文書削除 |
| `skb_stats` | 統計情報 |
| `skb_graph_query` | 知識グラフ検索 |
| `skb_graph_upsert_entity` | エンティティ作成・更新 |
| `skb_graph_link` | エンティティ間リンク |
| `skb_reindex` | 全文書再インデックス |

## 設定リファレンス

### `[storage]`

| キー | デフォルト | 説明 |
|---|---|---|
| `path` | `~/.local/share/skb/db` | データベースディレクトリ |
| `namespace` | `"skb"` | SurrealDB 名前空間 |
| `database` | `"knowledge"` | SurrealDB データベース名 |

> 注意: 設定キーは `[storage]` です（`[database]` ではありません）。誤ったキーを使用するとデフォルト値にフォールバックします。

### `[embedding]`

| キー | デフォルト | 説明 |
|---|---|---|
| `model` | `"BAAI/bge-m3"` | HuggingFace モデル ID |
| `onnx_path` | `"auto"` | ONNX モデルパス（`"mock"` で高速モック埋め込み） |
| `dimension` | `0`（自動検出） | 埋め込み次元数（モック時は `8` 等を指定） |
| `batch_size` | `32` | 推論バッチサイズ |
| `max_input_tokens` | `0`（自動 = 8192） | 入力の最大トークン数 |

### `[chunking]`

| キー | デフォルト | 説明 |
|---|---|---|
| `max_tokens` | `512` | チャンクあたりの最大トークン数 |
| `overlap_tokens` | `64` | 隣接チャンク間のオーバーラップ |

### `[search]`

| キー | デフォルト | 説明 |
|---|---|---|
| `default_mode` | `"hybrid"` | デフォルト検索モード（`hybrid\|vector\|keyword`） |
| `top_k` | `10` | デフォルト結果数 |
| `rrf_k` | `60` | RRF ランク定数 |

### `[upload]`

| キー | デフォルト | 説明 |
|---|---|---|
| `max_file_mb` | `100` | アップロード最大ファイルサイズ |

## 開発

```bash
# 高速コンパイルチェック
cargo check --workspace

# リント（警告ゼロ必須）
cargo clippy --workspace

# フォーマット
cargo fmt --all

# テスト（組み込み SurrealKV のため逐次実行必須）
cargo test --workspace -- --test-threads=1

# ベンチマーク（モック埋め込み）
cargo bench

# ベンチマーク（実 BAAI/bge-m3、ort feature 必要）
cargo bench --features ort
```

### Web 開発

```bash
cd web

# workspace のフォーマットとリント
bun run biome

# 全アプリケーションと共有パッケージの型検査
bun --filter '*' typecheck

# SSE スモークテスト（モック LLM と HTTP サーバーを自身で起動）
bun run sse-smoke

# ブラウザ E2E テスト（先に skb-server と mock_llm をビルド）
cargo build --manifest-path ../Cargo.toml -p skb-server --bin skb-server --examples
bunx playwright install chromium
bunx playwright test
```

## ドキュメント

- [SPECIFICATION.md](SPECIFICATION.md) — 正式仕様書
- [CONTRIBUTING.md](CONTRIBUTING.md) — 開発規約
- [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) — フェーズ別実装計画
- [AGENTS.md](AGENTS.md) — エージェント向け指示

## ライセンス

MIT。ONNX Runtime は [MIT ライセンス](npm/THIRD_PARTY_LICENSES.md)の下で静的リンクされています。
