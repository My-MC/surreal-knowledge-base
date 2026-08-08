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
| **gigatoken** | crates.io 未公開（404）。git 依存が必須。lib.rs に `#![feature(portable_simd)]` があり **Rust nightly 必須**。`pyo3`（abi3-py310）等の重い依存も必要 | Phase 0 の検証で不採用と判断。HuggingFace 公式の `tokenizers` クレートを採用済み |
| **rmcp** | crates.io で公開済み（公式 Rust SDK） | 問題なし |
| **bge-m3 ONNX** | HF リポジトリに公式提供 | `hf-hub` + `ort` で実現可能 |

---

## 全体方針

- **モノレポ + Cargo workspace**、TDD で進行
- **トレイト抽象化を先行**: `Tokenizer` トレイト・`Embedder` トレイトを最初に定義し実装差し替えを可能にする
- **契約テスト基盤は CLI/MCP 実装と同時** に構築しパリティを保証
- 各 Phase に Exit Criteria を設け、満たさない限り次へ進まない

## 実装状況の基準

この文書の状態は、`main` ブランチのコード、テスト、CI、生成物を確認した結果で更新する。`SPECIFICATION.md` は目標仕様を定義し、本書は実装済み範囲と未実装要件を区別する。

| 状態 | 意味 |
|---|---|
| 完了 | Exit Criteria、対応テスト、必要な配布/CI検証をすべて満たす |
| 部分完了 | 基本機能はあるが、仕様の一部または検証が不足している |
| 進行中 | 実装または検証を開始しているが、利用可能な完了条件を満たしていない |
| 未着手 | 実装・検証の成果物がない |
| 保留 | 技術判断または外部要因により着手を保留している |

### Phase 状態一覧

| Phase | 内容 | 状態 | 主な不足 |
|---|---|---|---|
| 0 | 技術検証スパイク | 部分完了 | 0-2/0-3の検証証跡を成果物として整理する |
| 1 | `skb-core` 基盤 | 部分完了 | 設定検証、入力安全性、DTO、CRUD件数、検索応答の不足 |
| 2 | グラフ + reindex | 部分完了 | N-hop、再ランク、dimension/HNSW/meta整合性の不足 |
| 3 | CLI | 部分完了 | 仕様上の入力形式、glob、JSON doctor、query、progressの不足 |
| 4 | MCP | 部分完了 | progressの不足 |
| 5 | 契約テスト + npm | 部分完了 | MCP/CLI比較、全ターゲットE2E、upload/search smokeの不足 |
| 6 | Skill | 部分完了 | 実装済みレスポンスとSkillの引用・エラー説明の同期が必要 |
| 7 | 仕上げ | 進行中 | ベンチ結果の判定、日本語FTS評価、公開手順の整理が必要 |
| 8 | ort有効バイナリ + 依存最小化 | 部分完了 | CI証跡と生成artifactの扱い、Windows runtime案内のE2Eが必要 |
| 9 | 仕様適合化と未実施機能 | 部分完了 | 9-1/9-2は完了。9-3〜9-7を順に実装する。既存の部分実装は完了条件を満たすまで未完了とする |

### 現状検証マトリクス

| 領域 | 現在確認できる実装 | 不足する検証/実装 | 次のPhase |
|---|---|---|---|
| 設定・モデル | `./skb.toml`/ユーザー設定探索、`SKB_*`環境変数オーバーライド、model名のmeta照合、dimension/max_inputのモデル解決と`E_VALIDATION`、tokenizer fingerprintの生成・meta保存・`E_MODEL_MISMATCH`、再起動検証（9-1完了） | CLI引数による設定オーバーライド（CLI > SKB_* > file）は引数が存在しないため未実装（envが最上位）。bge-m3 の config.json からの dimension / max_input_tokens 自動検出も未実装 | — |
| Upload | path/url/content/base64の基本処理、PDF抽出、allowed_dirs | 全経路の上限、SSRF、任意バイナリ、原子性、部分失敗 | 9-3 |
| Chunk/Graph/Search | token分割、基本抽出、vector/keyword/hybrid、単純graph expansion | heading、frontmatter/WikiLink、N-hop/re-rank、検索応答拡張 | 9-4 |
| Reindex | ドキュメント単位のchunk置換transaction | mismatch時の起動、dimension/HNSW/meta、全体rollback、progress | 9-5 |
| CLI/MCP | stdio MCP、主要CLI/MCP操作、resource-not-found、共通DTO/JSON Schema（9-2完了） | CLI parity、件数、query、JSON、progress、golden test | 9-6 |
| 配布/CI | 4ターゲットbuild matrix、linux smoke initialize | upload/search E2E、bunx、runtime依存、リリースゲート | 9-7 |

---

## Phase 0: 技術検証スパイク（目安: 1〜2 人日）

本実装前に 3 つの技術リスクを潰す。成果物は検証コードと判断レポート。

| # | タスク | 検証内容 | 判断基準 |
|---|---|---|---|
| 0-1 | **gigatoken Rust 依存スパイク（完了）** | git 依存のビルド条件と bge-m3 tokenizer の互換性を確認 | nightly・重い Python 依存・未公開依存のため不採用。`tokenizers` クレートへ切替済み |
| 0-2 | **SurrealDB 組込みスパイク** | `kv-surrealkv` 組込みモードで SPEC §4.1 スキーマ（HNSW・FTS analyzer・RELATION）が全て定義・動作するか | 全機能動作 → 継続 |
| 0-3 | **ort + bge-m3 スパイク** | `hf-hub` で ONNX DL、`ort` で推論、CLS+L2 正規化で次元検出、妥当な類似度スコアが出るか | 妥当なスコア → 採用 |

**決定事項（完了）**: gigatoken は不採用とし、HuggingFace 公式の `tokenizers` クレートを使用する。v1 の DB は SurrealKV 組込みモードのみとし、リモート接続は将来拡張とする。

---

## Phase 1: `skb-core` 基盤（目安: 6〜8 人日）

仕様書 §7 のコアライブラリをグラフ以外から実装。

| # | タスク | 内容 |
|---|---|---|
| 1-1 | workspace 構築 | `crates/{skb-core,skb-cli,skb-mcp}`、ツールチェイン（スパイク結果に応じて stable/nightly）、CI 骨格（fmt/clippy/test） |
| 1-2 | `config` モジュール | TOML 読込、優先順位（引数 > 環境変数 > `./skb.toml` > `~/.config/skb/config.toml`）、全キー・バリデーション |
| 1-3 | `error` モジュール | エラーコード体系、`thiserror` |
| 1-4 | `db` モジュール | SurrealKV 組込み接続、`001_init.surql` テンプレート（`{DIM}` 埋め込み）、`meta` テーブル・モデル不整合検出（`E_MODEL_MISMATCH`）。リモート接続は v1 対象外 |
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
| 5-3 | npm パッケージ | メタパッケージ + プラットフォーム別 optionalDependencies、`bin/skb-mcp.js` ラッパ（依存ゼロ・spawn）。ONNX Runtime は静的リンクし、共有ライブラリは同梱しない |
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
                                                                                                  │
                                                                                                  ▼
                                                                                         Phase 9 (仕様適合化)
```

---

## Phase 8: npm 配布用 ort 有効バイナリ + 依存最小化

### 目標
- ort feature を有効にした self-contained バイナリを全 4 プラットフォームでビルドし npm 配布可能にする
- 外部動的依存（OpenSSL, libstdc++）を排除し、Linux ランタイム要件を glibc >= 2.38 + libz + libzstd + libgcc_s + ca-certificates とする。Windows は `/MD` のVisual C++ Redistributableを前提とする。

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
   - ランナー: ubuntu-24.04 (x64), ubuntu-24.04-arm (arm64), macos-latest (arm64), windows-2022 (x64)
   - Linux: `CXXSTDLIB=""` と `-C link-arg=-l:libstdc++.a` で libstdc++ を静的リンク。libgcc_s は動的依存とする。Windows は ORT の要件に合わせて CRT を動的リンクする。
   - Windows の `/MD` バイナリは `MSVCP140.dll`、`MSVCP140_1.dll`、`VCRUNTIME140.dll`、`VCRUNTIME140_1.dll` を必要とする。これらは npm パッケージへ同梱せず、Visual C++ Redistributable for Visual Studio 2015--2022 (x64) の事前インストールをリリース手順と実行時エラーで案内する。
   - ビルド: `cargo build --release -p skb-mcp --features ort --target ...`
   - ~/.cache/ort.pyke.io の actions/cache キャッシュ
   - スモークテスト（linux-x64 ネイティブ）: ldd/objdump 検証 + npm pack/install + mock config で MCP initialize ハンドシェイク
   - 対象別検証: Linux x64/arm64 は `ldd`、macOS arm64 は `otool -L`、Windows x64 は依存DLL検査で `libonnxruntime.so`/`.dylib`/`.dll` の要否を確認し、各artifactをクリーン環境で起動する。Linuxではglibc >= 2.38、ca-certificates、WindowsではVisual C++ Redistributableを検証する。

6. **ドキュメント更新**
   - SPECIFICATION.md §3.1/§13.1/§13.4: ort 静的リンクの実態に合わせて書き換え + ランタイム要件表
   - CONTRIBUTING.md: OpenSSL 不要の明記、ort ビルド手順、ランタイム要件
   - IMPLEMENTATION_PLAN.md: 本フェーズ追記

### 検証状況と残条件
- `cargo test --workspace -- --test-threads=1` はCIで実行済み。Phase 9変更時もシリアル実行を継続する。
- `cargo build --release -p skb-mcp --features ort` 成功
- `ldd target/release/skb-mcp`: libonnxruntime / libssl / libstdc++ 非含有
- CI run [31079912794](https://github.com/My-MC/surreal-knowledge-base/actions/runs/31079912794)（commit `50d83d9`）で、4ターゲットのbuild matrix（`npm-linux-x64`、`npm-linux-arm64`、`npm-darwin-arm64`、`npm-win32-x64`）を実行する。生成artifact名は各`pkg`に対応する`npm-<pkg>`である。
- 現行CIのsmokeは生成artifactのうち`npm-linux-x64`だけを使用し、`ldd`、`objdump`、npm pack/install、MCP `initialize`を検証する。他のtargetの`otool -L`、依存DLL、クリーン環境起動、`tools/list → upload → search`、bunx、Windows runtime prerequisiteはPhase 9-7で追加する。
- `cargo tree -i openssl-sys | grep "did not match"` 成功
- CIで生成される4ターゲットartifactと、リポジトリに存在するpackage.jsonテンプレートを区別して記録する。

---

## Phase 9: 仕様適合化と未実施機能

Phase 0〜8 で確定した方針（`tokenizers`、SurrealKV 組込み、ORT 静的リンク）を前提に、仕様書の未実装項目を依存順に実装する。各変更は専用ブランチで行い、対応する回帰テストと仕様更新を同じPRに含める。各項目には基本実装が存在する場合もあるが、ここに記載する完了条件を満たすまで **未完了** とする。

### 9-1: 設定・モデル・tokenizer整合性（完了）

- `KnowledgeBase::open` は明示設定か自動検出かを保持したまま、`max_input_tokens = 0` をモデル設定から解決し、dimension/max inputを正規化してから `Config::validate()` の `0 < overlap_tokens < max_tokens <= max_input_tokens` を適用する。明示値とモデル値が不一致の場合は `E_VALIDATION` とする。
- CLI引数、`SKB_*`環境変数、`./skb.toml`、ユーザー設定の優先順位を実装する。
- モデル設定からdimensionと最大入力トークン数を検出し、明示設定との不一致を`E_VALIDATION`にする。
- `embedding.tokenizer` の明示パスと`"auto"`を同じ解決経路として扱い、取得元、`tokenizers`のアルゴリズム/バージョン、対象構成をcanonical JSON serializationしてSHA-256 fingerprintを作成する。
- fingerprint schema version、canonicalization規則、取得元、アルゴリズム/バージョン、fingerprintを`meta`に保存し、`KnowledgeBase::open`と`reindex`で比較する。
- **完了条件**: 不正設定、環境変数上書き、model/dimension/max input mismatch、tokenizer fingerprint不一致、保存後の再起動検証が緑。

✅ 実装済み: `Config::validate()`（`0 < overlap < max <= max_input`ほか）と `Config::resolve_embedding_settings()`（dimension/max_input のモデル値解決・不一致は `E_VALIDATION`）、`SKB_*` 環境変数オーバーライド（`Config::load()` がファイルなし時も default+env を返す）、tokenizer fingerprint（canonical JSON + SHA-256、schema v1、meta 保存・`open`/`reindex` で比較、不一致は `E_MODEL_MISMATCH`）、reindex 成功時に model/tokenizer meta を更新、`search` のデフォルト mode/top_k を `config.search` から適用。MockEmbedder は固定 8 次元として検出値とみなす。CLI 引数レイヤは該当する設定キーの引数が存在しないため env が最上位。bge-m3 の自動検出値は 1024/8192（OrtEmbedder 固定、config.json からの検出は将来課題）。

### 9-2: 共通DTO・JSON Schema基盤（完了）

- Request/Response型へ`Serialize`、`Deserialize`、`JsonSchema`をderiveし、CLIとMCPが同じ型を利用する。
- MCPの手書きschemaを廃止し、必須項目、one-of、enum、範囲制約をschemaと実行時の双方で検証する。
- 公開APIと実装APIの引数・戻り値・エラー形式を統一する。
- **完了条件**: MCP `tools/list`のschema検証、upload one-of、graph queryの`from`必須検証、CLI/MCP同一DTOのコンパイル・契約テストが緑。

✅ 実装済み: 全DTOにJsonSchema derive、MCPの`tool_with_required`を`schema_for!`ベースに置換、`UploadRequest::validate`（one-of）、`SearchRequest`（query必須・mode enum・top_k/graph_expand範囲）、`GraphQueryRequest`（from必須・depth 1..=5・limit≥1）、`EntityInfo`/`LinkInfo`、`ListQuery`/`OrderBy`/`GetDocumentRequest`/`DeleteDocumentRequest`（公開APIの引数をDTO化）、`ReindexRequest`。CLIは同一DTOを構築。`skb list --limit 0`は`E_VALIDATION`に変更。

### 9-3: Upload安全性・原子性（部分完了）

- ファイル、stdin、base64、inline、URLの全入力に`upload.max_file_mb`を適用し、decode/extract前後のサイズを検査する。
- base64は任意バイナリとして保持し、MIME/拡張子に応じてPDF等を抽出する。未対応形式は`E_UNSUPPORTED_FORMAT`で拒否する。
- URLはHTTP(S)のみ許可し、redirect数、受信ストリーム、DNS解決後のprivate/reserved、loopback、link-local、multicast、metadata IPを検証する。検証済みIPへの接続固定または同一のDNS解決結果を使うresolver/connectorを用い、各redirectでもURL検証から接続まで同じ対策を適用する。
- 入力ストリーム、base64 decoded data、展開後データ、抽出出力、処理時間、メモリ、PDFページ数、圧縮/抽出のネスト深度に上限を設け、上限到達時は直ちに停止する。上限はdecode/extract前後の検査だけに依存しない。
- document、chunk、entity、mentions、force更新時の旧データ削除を一つのトランザクションで処理する。
- 複数入力時は成功結果と`errors[]`を集約し、一件の失敗で全体を中断しない。
- **完了条件**: サイズ超過base64、圧縮爆弾、PDF爆弾、decode/extractの時間・メモリ・ページ・ネスト上限、DNS rebinding、redirect再検証、未対応形式、部分失敗、rollbackのテストが緑。

### 9-4: Chunk・Graph・Search（部分完了）

- `EntityExtractor`トレイトを追加し、WikiLink、Markdownリンク先、frontmatterのtags/aliases、見出し階層を抽出する。
- Markdownの段落・文・見出し境界を優先してchunk化し、`Chunk.heading`を保存する。
- 検索結果に`title`、`source`、keywordの`highlights`、graph拡張時の`matched_entities`を追加する。
- chunk → entity → related entity → chunk のN-hop探索と、元スコア・距離による再ランクを実装する。
- **完了条件**: 抽出、heading、N-hop、再ランク、検索レスポンスの契約テストが緑。

### 9-5: Reindex・進捗（部分完了）

- `KnowledgeBase::open` は `Db::migrate` より前に保存済みの `embedding_model` と `embedding_dimension` を現在値と比較し、mismatch時は `E_MODEL_MISMATCH` を返してschema、field、index、metaを変更しない。dimension変更はreindex経路でのみ実施し、migrateはモデル一致時または本当に不足している定義への適用に限る。
- reindexを起動時のmodel mismatch状態から実行できる管理経路を用意する。
- dimension変更時に`chunk.embedding`フィールドとHNSWインデックスを再定義し、旧chunk/mentions削除、新chunk/entity索引、model metadata更新を同一transaction境界で処理する。
- 中断時は旧schema、旧chunk、旧metaを維持し、再起動後も整合性を検証する。
- MCP progress notificationとCLI progress barを実装する。
- **完了条件**: dimension変更、HNSW再構築、metadata更新、途中失敗rollback、再起動復旧、progressのテストが緑。

### 9-6: CLI・MCP parity（部分完了）

- CLIの複数パス、glob、`graph entity add`、`skb query`、doctor JSON、progress表示を仕様へ合わせる。
- listの`chunk_count`、deleteの`chunks_deleted`、不存在documentの`E_DOCUMENT_NOT_FOUND`を実装する。
- resource-not-foundなどMCPエラー形式を共通DTOに合わせる。
- CLI/MCPへ同一JSONを投入し、レスポンスの意味とエラーを比較するゴールデン契約テストを追加する。
- **完了条件**: 全CLIコマンド、MCP tools/resources、JSON/table出力、件数、エラー、progressの契約テストが緑。

### 9-7: 配布・E2E・CI（部分完了）

- 4ターゲットのnpm package生成、`npm pack`、npx/bunx起動を検証する。
- `initialize → tools/list → skb_upload → skb_search`をlinux-x64 smokeと各ターゲットE2Eで検証する。
- Linux x64/arm64、macOS arm64、Windows x64の各artifactについて、Linuxは`ldd`、macOSは`otool -L`、Windowsは依存DLL検査を実行し、ORT共有ライブラリの同梱要否、OSランタイム、証明書ストアを確認する。各対象をクリーン環境で起動し、WindowsのVisual C++ Redistributable prerequisiteとLinuxのglibc >= 2.38 + ca-certificatesを検証する。
- `cargo check`、`cargo clippy`、`cargo fmt --check`、シリアルテスト、契約テストをCIのリリースゲートに追加する。
- **完了条件**: 4ターゲットbuild、npm pack/install、npx/bunx、MCP upload/search、各対象のruntime dependency・証明書ストア・クリーン環境起動検証がすべて緑。
