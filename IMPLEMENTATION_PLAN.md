# Surreal Knowledge Base 実装プラン

| 項目 | 内容 |
|---|---|
| プロジェクト名 | Surreal Knowledge Base（SKB） |
| ベース仕様書 | `SPECIFICATION.md` |
| 作成日 | 2026-07-29 |

---

## 事前調査で判明した重要事項

| 項目 | 調査結果 | 影響 |
|---|---|---|
| **gigatoken** | crates.io 未公開（404）。git 依存が必須。lib.rs に `#![feature(portable_simd)]` があり **Rust nightly 必須**。`pyo3`（abi3-py310）・`numpy`・`parquet`・`icu(datagen)` の重い依存あり、ビルド時に Python 3.10+ が必要 | 仕様書 §3.1 のリスクが現実化。Phase 0 スパイクで検証し、ダメなら `tokenizers` クレートへ切替（`Tokenizer` トレイトで抽象化済み） |
| **rmcp** | crates.io で公開済み（公式 Rust SDK） | 問題なし |
| **bge-m3 ONNX** | HF リポジトリに公式提供 | `hf-hub` + `ort` で実現可能 |

---

## 全体方針

- **モノレポ + Cargo workspace**、TDD で進行
- **トレイト抽象化を先行**: `Tokenizer` トレイト・`Embedder` トレイトを最初に定義し実装差し替えを可能にする
- **契約テスト基盤は CLI/MCP 実装と同時** に構築しパリティを保証
- 各 Phase に Exit Criteria を設け、満たさない限り次へ進まない

---

## Phase 0: 技術検証スパイク（目安: 1〜2 人日）

本実装前に 3 つの技術リスクを潰す。成果物は検証コードと判断レポート。

| # | タスク | 検証内容 | 判断基準 |
|---|---|---|---|
| 0-1 | **gigatoken Rust 依存スパイク** | git 依存でビルド可能か（nightly、libpython リンク）、`load_tokenizer` で bge-m3 の `tokenizer.json` をロードできるか、`tokenizers` クレートとエンコード結果が一致するか | 一致＆実用的なビルド時間 → 採用。困難 → `tokenizers` クレート採用 |
| 0-2 | **SurrealDB 組込みスパイク** | `kv-surrealkv` 組込みモードで SPEC §4.1 スキーマ（HNSW・FTS analyzer・RELATION）が全て定義・動作するか | 全機能動作 → 継続 |
| 0-3 | **ort + bge-m3 スパイク** | `hf-hub` で ONNX DL、`ort` で推論、CLS+L2 正規化で次元検出、妥当な類似度スコアが出るか | 妥当なスコア → 採用 |

**決定事項**: gigatoken の採否はこのスパイク結果で判断する。

---

## Phase 1: `skb-core` 基盤（目安: 6〜8 人日）

仕様書 §7 のコアライブラリをグラフ以外から実装。

| # | タスク | 内容 |
|---|---|---|
| 1-1 | workspace 構築 | `crates/{skb-core,skb-cli,skb-mcp}`、ツールチェイン（スパイク結果に応じて stable/nightly）、CI 骨格（fmt/clippy/test） |
| 1-2 | `config` モジュール | TOML 読込、優先順位（引数 > 環境変数 > `./skb.toml` > `~/.config/skb/config.toml`）、全キー・バリデーション |
| 1-3 | `error` モジュール | エラーコード体系、`thiserror` |
| 1-4 | `db` モジュール | 組込み/リモート接続、`001_init.surql` テンプレート（`{DIM}` 埋め込み）、`meta` テーブル・モデル不整合検出（`E_MODEL_MISMATCH`） |
| 1-5 | `tokenize` モジュール | `Tokenizer` トレイト定義 + 実装、チャンカー（見出し/段落/文境界優先） |
| 1-6 | `embed` モジュール | `Embedder` トレイト定義 + ort 実装、HF DL、遅延ロード、バッチ推論、次元自動検出、テスト用モック |
| 1-7 | `ingest` モジュール | 入力統一（path/url/content/base64/stdin）、テキスト抽出（txt/md/html）、SHA-256 重複排除、トランザクション保存、進捗コールバック |
| 1-8 | `search` モジュール | vector（HNSW KNN）/ keyword（BM25）/ hybrid（RRF） |
| 1-9 | ドキュメント CRUD・`stats`・`doctor` | list/get/delete、診断レポート |

**Exit Criteria**: モック Embedder で upload→search→delete の統合テストが緑。実モデルでの E2E 手動確認。

---

## Phase 2: グラフ + reindex（目安: 3 人日）

| # | タスク | 内容 |
|---|---|---|
| 2-1 | `graph` モジュール | entity upsert、`mentions`/`related_to` の RELATE、N ホップ探索 |
| 2-2 | ルールベース抽出 | WikiLink/Markdown リンク・frontmatter tags・見出し階層。`EntityExtractor` トレイト化 |
| 2-3 | グラフ拡張検索 | ヒット → entity → 関連チャンクの再ランク |
| 2-4 | `reindex` | 全件再チャンク化・再埋め込み・次元変更時インデックス再定義・`meta` 更新、`dry_run` |

**Exit Criteria**: アップロードで `mentions` 自動生成、`graph_expand=1` で関連チャンクが返る。モデル変更→`E_MODEL_MISMATCH`→reindex→復旧の一連テストが緑。

---

## Phase 3: CLI `skb`（目安: 2〜3 人日）

- clap による全コマンド（`upload/search/list/get/delete/graph/stats/reindex/config/doctor`）
- `--format json|table`、エラーは stderr + 終了コード、`--stdin` パイプ対応、進捗表示
- **Exit Criteria**: 全コマンドの結合テスト（assert_cmd）緑。

---

## Phase 4: MCP サーバー `skb-mcp`（目安: 3 人日）

- `rmcp` で stdio サーバー。10 ツール（スキーマは schemars で自動生成）
- Resources（`skb://documents` 等）・Prompts（`skb-answer`）
- ログ stderr のみ、ツールエラーは `isError: true`
- CLI とは分離し、npm パッケージから `npx surreal-knowledge-base` で起動
- **Exit Criteria**: MCP Inspector / opencode から initialize → tools/list → upload → search が通る。

---

## Phase 5: 契約テスト + npm パッケージ化（目安: 5〜6 人日）

| # | タスク | 内容 |
|---|---|---|
| 5-1 | **契約テスト** | MCP ハンドラ経由・CLI 経由の同一 JSON リクエストでレスポンス一致を検証するゴールデンテスト。CI 必須化 |
| 5-2 | クロスビルド | GitHub Actions で 4 ターゲット（linux-x64/arm64, darwin-arm64, win32-x64） |
| 5-3 | npm パッケージ | メタパッケージ + プラットフォーム別 optionalDependencies、`bin/skb-mcp.js` ラッパ（依存ゼロ・spawn）、`libonnxruntime` 同梱 |
| 5-4 | E2E（全 4 ターゲット） | `npm pack` → 各実機ランナーで `npx`/`bunx` スモークテスト |

**Exit Criteria**: 契約テスト全緑。全 4 ターゲットの CI 実機 E2E が通ること。

---

## Phase 6: Skill（目安: 1〜2 人日）

- `skills/surreal-knowledge-base/SKILL.md` 作成（前提確認・操作レシピ・出力解釈・エラー対処）
- opencode で Skill を読み込ませ、「資料登録→検索」の実機確認

---

## Phase 7: 仕上げ（目安: 3 人日〜）

- ベンチマーク（トークナイズ/Embedding/検索レイテンシ/MCP 起動）
- 日本語 FTS の精度評価・チューニング
- win32/docx（v2 判断）、README・ドキュメント整備、npm publish

---

## 依存関係

```
Phase 0 (スパイク)
   │
   ▼
Phase 1 (skb-core) ──► Phase 2 (graph/reindex)
   │                        │
   ▼                        ▼
Phase 3 (CLI) ──► Phase 4 (MCP) ──► Phase 5 (契約テスト+npm) ──► Phase 6 (Skill) ──► Phase 7 ──► Phase 8
```

---

## Phase 8: npm 配布用 ort 有効バイナリ + 依存最小化

### 目標
- ort feature を有効にした self-contained バイナリを全 4 プラットフォームでビルドし npm 配布可能にする
- 外部動的依存（OpenSSL, libstdc++）を排除し、Linux ランタイム要件を glibc + libz + libzstd のみに

### 実施内容

1. **Cargo 依存整理**
   - surrealdb: `default-features = false` で protocol-ws/tokio-tungstenite を除去（埋め込み専用）
   - skb-mcp / skb-cli: `[features] ort = ["skb-core/ort"]` を追加（default = []、dev ビルド高速化のため）
   - workspace: `[profile.release] strip = "symbols"` で配布バイナリサイズ削減

2. **TLS rustls 確認・固定化**
   - hf-hub 1.0 → reqwest 0.13: default-tls = rustls（aws-lc-rs + platform-verifier）。変更不要
   - ureq 3: default = rustls（ring + webpki-roots 埋め込み CA）。変更不要
   - CI ガード: `cargo tree -i openssl-sys | grep "did not match"` で回帰防止

3. **ライセンス同梱**
   - `npm/THIRD_PARTY_LICENSES.md`: ONNX Runtime MIT ライセンス全文（静的リンクで法的要件）
   - `LICENSE`: プロジェクト MIT

4. **プラットフォーム別 package.json テンプレート**（4 種すべてコミット済み）

5. **CI 更新**
   - ランナー: ubuntu-22.04 (x64), ubuntu-22.04-arm (arm64), macos-latest (arm64), windows-2022 (x64)
   - RUSTFLAGS: linux `-static-libstdc++ -static-libgcc`, windows `-C target-feature=+crt-static`
   - ビルド: `cargo build --release -p skb-mcp --features ort --target ...`
   - ~/.cache/ort.pyke.io の actions/cache キャッシュ
   - スモークテスト（linux-x64 ネイティブ）: ldd/objdump 検証 + npm pack/install + mock config で MCP initialize ハンドシェイク

6. **ドキュメント更新**
   - SPECIFICATION.md §3.1/§13.1/§13.4: ort 静的リンクの実態に合わせて書き換え + ランタイム要件表
   - CONTRIBUTING.md: OpenSSL 不要の明記、ort ビルド手順、ランタイム要件
   - IMPLEMENTATION_PLAN.md: 本フェーズ追記

### 終了条件
- `cargo test --workspace` 全 9 テスト通過（surrealdb スリム化後も影響なし）
- `cargo build --release -p skb-mcp --features ort` 成功
- `ldd target/release/skb-mcp`: libonnxruntime / libssl / libstdc++ 非含有
- CI 全 4 ターゲットビルド + smoke test 通過
- `cargo tree -i openssl-sys | grep "did not match"` 成功
