# ctxgate

Claude Code 向けの **agent context firewall**。
ツール出力が LLM のコンテキストに入る前に横取りし、生データはローカルの vault に退避、モデルには圧縮版だけを渡す。

```
Claude Code ─ Read / Grep / Bash / MCP ─▶ ctxgate ─▶ Claude
                                            │
                                            ▼
                                     ~/.ctxgate/store   (raw, content-addressed)
```

RTK (`rtk-ai/rtk`) が「Bash コマンドを賢くする」プロキシなのに対し、ctxgate は
Claude Code の **PreToolUse / PostToolUse hooks** で「Claude に入る情報そのもの」を管理する。
Bash だけでなく Read / Grep / Glob / WebFetch / MCP ツールの出力まで面倒を見る。

実装言語は [Almide](https://github.com/almide/almide)。判断はすべて Almide 側にあり、唯一の例外として
ソース解析だけ tree-sitter（Rust 製の小さな helper `ctxgate-outline`）にあやかっている。
Almide 版 tree-sitter ができた時点で helper を差し替えるだけで済むよう、境界は JSON 一枚に絞ってある。

[English README](README.md)

## 入っているもの

### v0.7 — rtk にあやかる

| 機能 | 状態 | 内容 |
|---|---|---|
| Bash の書き換え委譲 | ✅ | `rtk` が PATH にあれば、PreToolUse で `rtk rewrite "<cmd>"` に渡し、`git status` → `rtk git status` のように置き換える |
| 権限の扱い | ✅ | rtk の exit code 契約に従う。0 = allow 付きで書き換え、3 = 書き換えのみ（Claude Code が確認を出す）、1/2 = 触らない |
| 役割分担 | ✅ | Bash コマンドの網羅（100+）は rtk、vault・予算・dedup・Read/Grep/MCP は ctxgate。`CTXGATE_RTK=0` で無効 |
| `init` の更新 | ✅ | 既存の ctxgate エントリを置き換えるので、バージョンアップ後は `ctxgate init` を再実行するだけ |

### v0.6 — セッション内 dedup

| 機能 | 状態 | 内容 |
|---|---|---|
| 同一出力の再取得 | ✅ | 同じ Read / 同じコマンドで内容が同一なら「unchanged since you last saw it（N lines, ctx:id）」の1行 |
| 変更後の再取得 | ✅ | 内容が変わっていれば差分だけ（変更行 + 前後1行、`@@ L14` 形式）。差分が本文の 60% 未満のときのみ |
| diff エンジン | ✅ | 共通接頭辞・接尾辞を剥いでから LCS。上限セル数を超えたら差分を諦めて通常経路 |
| 対象 | ✅ | 閾値未満の小さい出力でも 600B 以上なら対象（`CTXGATE_DEDUP_MIN`） |
| compaction 対応 | ✅ | usage が 30% 以上落ちたら Claude Code が compaction したとみなし、そのセッションの記憶を捨てる |

```
[ctxgate] Read src/compress.almd: changed since you last saw it — +2 −2 lines of 130 (ctx:ed94… → ctx:4ca9…). Only the diff is shown; the rest is as before.
  full current text: ctxgate show 4ca9d3c5824c   |   Read offset/limit
── @@ L14 ──
  -let NOISE_MARKERS = [...]
  +let NOISE_MARKERS2 = [...]
── @@ L22 ──
  -fn clip(line: String, max: Int) -> String =
  +fn clip(line: String, max: Int) -> String =   // edited
```

### v0.5 — Context Budgeter

| 機能 | 状態 | 内容 |
|---|---|---|
| 使用量の取得 | ✅ | hook が受け取る `transcript_path`（JSONL）の最新 assistant 行の `usage`（input + cache_read + cache_creation）を読む。2.4MB の transcript で 1ms |
| ウィンドウ推定 | ✅ | `~/.claude/settings.json` の model に `[1m]` があれば 1M、なければ 200k。`CTXGATE_WINDOW` で上書き |
| レベル | ✅ | NORMAL → COMPRESS（≥40%）→ AGGRESSIVE（≥60%）→ ISOLATE（≥75%）。閾値は `CTXGATE_LVL_*` |
| 締め付け | ✅ | レベルに応じて退避閾値・head/tail・salient・目次の行数・Grep の `head_limit`・stanza 長を段階的に縮める |
| バナー | ✅ | NORMAL 以外では置換出力の先頭に1行「context 72% → AGGRESSIVE」を付け、モデルに理由を伝える |
| `ctxgate status` | ✅ | 最新セッションの使用量・レベル・実効閾値・そのセッションで退避した量 |

```
$ ctxgate status
ctxgate context budget  (window 1000k; levels: COMPRESS ≥40%  AGGRESSIVE ≥60%  ISOLATE ≥75%)
  session   992c7837-…
  used      375k tokens  (37%)  → NORMAL
  effective budgets  bash 8000  read 12000  grep 8000  head/tail 30/30  outline 120
  this session: 6 outputs vaulted, 195 KB kept out of context (≈50070 tokens)
```

| レベル | 退避閾値 | head/tail | 目次 | Grep head_limit |
|---|---|---|---|---|
| NORMAL | 8k / 12k / 8k | 30 / 30 | 120 | 60 |
| COMPRESS | 1/2 | 15 / 15 | 80 | 40 |
| AGGRESSIVE | 1/4 | 10 / 10 | 50 | 30 |
| ISOLATE | 1.5k | 6 / 6 | 30 | 20 |

Claude 自身に「節約して」と頼むのではなく、hook 側で強制的に絞る。

### v0.4 — git diff / status / log 圧縮

| 機能 | 状態 | 内容 |
|---|---|---|
| git diff / show / log -p | ✅ | ファイル表（M/A/D/R/B、+N −M、hunk 数）+ 意味のある hunk だけ |
| hunk の絞り込み | ✅ | 変更行と前後1行のみ。離れた変更は `⋮` で区切る。hunk ごと・ファイルごと・全体で行数上限 |
| ノイズ除外 | ✅ | lockfile（package-lock / Cargo.lock / go.sum …）、生成物（dist/ / *.min.js / *.pb.go …）、バイナリは stat 行だけ |
| rename / 削除 | ✅ | `R old → new`、`D path −N` の1行に |
| git status | ✅ | ブランチ + ahead/behind + staged / unstaged / untracked をそれぞれ1行に |
| git log | ✅ | 1コミット1行（sha, 日付, author, subject, `--stat` があれば集計） |

実測: lockfile を含む 8 ファイルの `git diff HEAD`、795 行 / 17KB → 39 行 / 1.4KB。

```
git diff: 8 files, +111 −113
  M  README.md                          +2
  B  logo.png                                      binary
  M  package-lock.json                  +100 −100  lockfile — hunks omitted
  M  src/lower.rs                       +3 −3      4 hunks
  A  src/new.rs                         +5
  D  src/old.rs                         −10        deleted
  R  src/rename_me.rs → src/renamed.rs
── src/lower.rs @@ -3 +3 @@ fn f1() -> i32 { 1 } ──
   fn f4() -> i32 { 4 }
  -fn f5() -> i32 { 5 }
  +fn f5() -> i32 { 500 } // changed
   fn f6() -> i32 { 6 }
```

### v0.3 — テスト / ビルド出力パーサ

| 機能 | 状態 | 内容 |
|---|---|---|
| ランナー検出 | ✅ | コマンド（`cargo test` / `go test` / `pytest` / `vitest` / `jest` / `tsc` …）と、`make test` のような場合は出力の形から判定 |
| cargo test | ✅ | 合計（passed / failed / ignored、複数バイナリは合算）+ 失敗名 + 各失敗の stanza（`---- name stdout ----`）。コンパイル失敗時は rustc 診断に切り替え |
| cargo build / check / clippy | ✅ | error は全文（上限あり）、warning は1行（メッセージ + `-->` 位置）に畳む。`Compiling …` は消える |
| go test | ✅ | `--- FAIL:` ごとの stanza。panic のスタックから `testing.go` / `runtime` のフレームを落とし、自分のコードのフレームだけ残す |
| pytest | ✅ | サマリ行 + `FAILED path::test` + 失敗ごとに `E` 行・`>` 行・`path:N: Error` だけ |
| vitest / jest / tsc / go build | ✅ | 失敗ブロックと診断行のみ |
| `ctxgate summarize "<cmd>"` | ✅ | hook と同じパーサを stdin に適用。CI や手元でも使える |
| fail-open | ✅ | 形が合わなければ `none` を返し、v0.1 の汎用要約に落ちる |

```
$ cargo test 2>&1 | ctxgate summarize "cargo test"
cargo test: FAILED — 2 passed, 2 failed, 1 ignored
failed:
  tests::fails_eq
  tests::fails_panic
── tests::fails_eq ──
  thread 'tests::fails_eq' (142821343) panicked at src/lib.rs:7:29:
  assertion `left == right` failed: math is hard
    left: 2
   right: 3
```

フィクスチャは `tests/fixtures/`（cargo / go は実出力を採取）。`CTXGATE_MAX_BLOCK`（既定 20 行）で stanza の長さを調整できる。

### v0.2 — AST-aware Read

| 機能 | 状態 | 内容 |
|---|---|---|
| Read → 目次 | ✅ | 閾値超えの Read は、head/tail ではなく **シンボル一覧（kind / name / 行範囲）** に置換 |
| 対応言語 | ✅ | Rust / Go / TypeScript / TSX / JavaScript / Python（tree-sitter）、`.almd`（Almide 自前パーサ） |
| `show <id> --symbol NAME` | ✅ | 元ファイルからそのシンボルの行範囲だけ取り出す。`compile` で `Compiler::compile` にも当たる |
| `ctxgate outline <file>` | ✅ | 目次を直接見る |
| テスト畳み込み | ✅ | 目次が上限を超えたら `mod tests` / `test_*` を1行にまとめる（git.rs: 231 シンボル → 84 行 + 「147 test symbols collapsed」） |
| 全ファイル目次 | ✅ | Read が1ページ目しか返していなくても、目次はファイル全体（4721 行なら 4721 行分） |

Read の置換例（rtk の `src/cmds/git/git.rs`、4720 行 / 64k トークン → 6.9KB）:

```
[ctxgate] Read .../git.rs: 4720 lines, rust, 231 symbols → vaulted as ctx:b0c8c343b22f (NOT in context) (lines 1-1555 of 4721; page with Read offset/limit).
  one symbol: ctxgate show b0c8c343b22f --symbol <name>   |   a range: Read offset=N limit=M   |   search: ctxgate show b0c8c343b22f --grep <re>
── outline ──
  enum    GitCommand                                        L20-34
  fn      run_diff                                          L112-216
  struct  HunkHeader                                        L500-508
  impl    HunkHeader                                        L510-551
  fn      HunkHeader::consume                               L530-550
  fn      compact_diff                                      L648-821
  ...
  mod     tests                                             L2657-4720
  … 147 test symbols collapsed (ctxgate outline <file>)
── head ──
   1│ //! Filters git output — log, status, diff, and more — keeping just the essential info.
```

### v0.1 — Output Vault

| 機能 | 状態 | 内容 |
|---|---|---|
| Output Vault | ✅ | 閾値超過の出力を `~/.ctxgate/store/<sha256[:12]>.txt` に保存。同一内容は同じ id |
| PostToolUse 圧縮 | ✅ | salient 行（error / FAIL / panic …）+ head + tail + 「N 行省略」マーカーに置換 |
| shape 対応 | ✅ | Bash `stdout` / Read `file.content` / `content` / `text` / `output` / MCP `content[]` |
| persistedOutputPath | ✅ | Claude Code が ~30KB 超を永続化した場合、その全文を読んで salient 判定する |
| Read のページ情報 | ✅ | `lines 1-1643 of 4191` を付けて offset/limit で続きを読めるようにする |
| PreToolUse | ✅ | `head_limit` のない Grep に上限を注入 |
| `ctxgate show` | ✅ | `--grep RE` / `--around RE [N]` / `--lines A-B` / `--head N` / `--tail N` で部分取得 |
| `ctxgate list` / `stats` | ✅ | 何を退避したか、何バイト（≒何トークン）コンテキストから追い出したか |
| `ctxgate init` | ✅ | `.claude/settings.json`（`--global` で `~/.claude`）に hooks をマージ。冪等、`.bak` 作成 |
| fail-open | ✅ | JSON が壊れていても、vault に書けなくても、元の出力をそのまま通す。exit 0 以外を返さない |

## インストール

```bash
almide install github.com/O6lvl4/ctxgate      # ~/.local/bin/ctxgate
# または手元でビルド
almide build --release -o bin/ctxgate && cp bin/ctxgate ~/.local/bin/

# Read の目次に必要な tree-sitter helper（無くても v0.1 の head/tail 表示に自動で落ちる）
(cd tools/ctxgate-outline && cargo build --release && cp target/release/ctxgate-outline ~/.local/bin/)

cd your-project
ctxgate init            # .claude/settings.json に hooks を追加
ctxgate init --global   # ~/.claude/settings.json に追加
```

Claude Code を再起動（または `/hooks`）すると有効になる。

## モデルが見るもの

`cargo test --workspace` が 12KB 吐いたとき、Claude に渡るのはこれだけ:

```
[ctxgate] Bash output: 403 lines / 12 KB → vaulted as ctx:9258de38065e (NOT in context).
  source: cargo test --workspace
  retrieve: ctxgate show 9258de38065e --grep <re> | --around <re> [n] | --lines A-B | --head N | --tail N
── salient (3) ──
201│ error[E0308]: mismatched types in src/lower.rs:42
301│ test parser::match_nested ... FAILED
403│ test result: FAILED. 238 passed; 3 failed; 0 ignored
── head ──
  1│    Compiling crate0 v0.0.0
  ...
   ⋮  (343 lines omitted — in the vault)
── tail ──
  ...
```

必要になったら Claude 自身が取りに行く:

```bash
ctxgate show 9258de38065e --around FAILED 20
ctxgate show 9258de --grep 'error\[E'      # id は前方一致で省略可
ctxgate show 9258de38065e --lines 180-220
```

## 設定

環境変数のみ（設定ファイルは v0.1 にはない）。

| 変数 | 既定 | 意味 |
|---|---|---|
| `CTXGATE_HOME` | `~/.ctxgate` | vault の場所 |
| `CTXGATE_MAX_BASH` | 8000 | Bash stdout がこのバイト数を超えたら退避 |
| `CTXGATE_MAX_READ` | 12000 | Read |
| `CTXGATE_MAX_GREP` | 8000 | Grep / Glob |
| `CTXGATE_MAX_OTHER` | 10000 | MCP / WebFetch / その他 |
| `CTXGATE_HEAD` / `CTXGATE_TAIL` | 30 / 30 | 残す行数 |
| `CTXGATE_SALIENT` | 40 | salient 行の上限 |
| `CTXGATE_LINE_CLIP` | 200 | 長い行はここで切る |
| `CTXGATE_GREP_HEAD_LIMIT` | 60 | Grep に注入する `head_limit` |
| `CTXGATE_WINDOW` | 自動（1M or 200k） | コンテキストウィンドウのトークン数 |
| `CTXGATE_LVL_COMPRESS` / `_AGGRESSIVE` / `_ISOLATE` | 40 / 60 / 75 | レベル切替の % |
| `CTXGATE_OUTLINE_BIN` / `CTXGATE_OUTLINE_MAX` | ctxgate-outline / 120 | 目次 helper と表示上限 |
| `CTXGATE_MAX_BLOCK` | 20 | 失敗 stanza / hunk の行数上限 |
| `CTXGATE_DEDUP_MIN` | 600 | このバイト数以上の出力を dedup 対象に（0 で無効） |
| `CTXGATE_DIFF_MAX_CELLS` | 4,000,000 | 差分計算（LCS）の上限セル数 |
| `CTXGATE_DEBUG` | | `1` で hook の生 stdin を `<home>/debug/` に保存（shape 調査用） |

## Claude Code 側の挙動（実測、Claude Code 2.1.x）

- hook 入力は `tool_response` に入る。Bash は `{stdout, stderr, interrupted, isImage, noOutputExpected}`、Read は `{type:"text", file:{filePath, content, numLines, startLine, totalLines, truncatedByTokenCap}}`。
- Bash stdout が約 30KB を超えると Claude Code が全文を `persistedOutputPath` に書き、hook には先頭 30KB だけ渡す。この場合モデルに見えるのは置換後 stdout の **先頭 2KB のみ**。ctxgate はこのケースを検出して 6+6 行のコンパクト表示に切り替える。
- Read は Claude Code 側で 25k トークンにキャップされる。ctxgate はその中身をさらに縮める。
- 出力置換は `hookSpecificOutput.updatedToolOutput` に `tool_response` と同じ形で返す。

## 参照している OSS（`reference/`、git 管理外）

| repo | 何を参考にしたか |
|---|---|
| `rtk-ai/rtk` | PreToolUse `updatedInput` でのコマンド書き換え、fail-open 契約、tee による raw 退避 |
| `micaelmalta/token-crunch` | PostToolUse `updatedToolOutput` の shape ごとの差し戻し、セッション単位の dedup |
| `martinstannard/openrtk` / `ousamabenyounes/rtk-mcp` | 他エージェントへの展開パターン |

## 開発

```bash
almide check          # 型検査
almide test           # 各モジュール内の test ブロック
almide build --release -o bin/ctxgate
echo '{"tool_name":"Bash","tool_input":{"command":"x"},"tool_response":{"stdout":"..."}}' | bin/ctxgate hook post
```

```
src/
  main.almd       CLI dispatch, show/list/stats/init
  config.almd     閾値と env
  compress.almd   純粋な要約ロジック（salient / head / tail / render）
  vault.almd      store + index.jsonl
  hook_pre.almd   PreToolUse
  hook_post.almd  PostToolUse（shape 抽出・差し戻し・persistedOutputPath・Read 目次・ランナー要約）
  testout.almd    cargo / go / pytest / vitest / jest / tsc の出力パーサ（純粋、fixtures でテスト）
  gitout.almd     git diff / status / log の圧縮（純粋、fixtures でテスト）
  budget.almd     Context Budgeter: transcript から usage を読み、レベル別に Config を締める
  dedup.almd      セッション内 dedup: 同一出力は1行、変更は差分。compaction で忘れる
  textdiff.almd   行単位 diff（共通接頭辞・接尾辞 + LCS）
  outline.almd    目次: .almd は自前パース、他は helper の JSON を読む。find / render / テスト畳み込み
tools/
  ctxgate-outline/  Rust + tree-sitter。JSON {path, lang, total_lines, symbols:[{kind,name,start,end}]} を吐くだけ
```

`CTXGATE_OUTLINE_BIN`（既定 `ctxgate-outline`）と `CTXGATE_OUTLINE_MAX`（既定 120 シンボル）で調整できる。

## ロードマップ

| 版 | 内容 |
|---|---|
| ~~v0.2~~ | ✅ AST-aware Read（目次 + `--symbol`） |
| ~~v0.3~~ | ✅ テスト / ビルド出力パーサ（cargo / go / pytest / vitest / jest / tsc） |
| ~~v0.4~~ | ✅ git diff / status / log 圧縮 |
| ~~v0.5~~ | ✅ Context Budgeter（transcript の usage → NORMAL / COMPRESS / AGGRESSIVE / ISOLATE） |
| ~~v0.6~~ | ✅ セッション内 dedup（同一は1行、変更は差分だけ） |
| v0.7 | Heavy Task Router: 探索的ツール連打を検知して subagent 委譲を促す |
| v0.8 | Codex / Gemini CLI / Cursor の hook 形式にも対応 |
