# ctxgate

**Claude Code のための context firewall。**
ツールの出力を生のままモデルに渡さない。まずローカルの vault に退避し、モデルには考えるのに十分な小ささの「見え方」だけを渡す。必要になれば、どの詳細でも取りに行ける。

```
Claude Code ── Bash · Read · Grep · Glob · WebFetch · MCP ──▶ ctxgate ──▶ モデル
                                                                │
                                                                ▼
                                                     ~/.ctxgate/store  (raw, content-addressed)
```

[English](README.md) · [Changelog](CHANGELOG.md)

---

## 問題

エージェントのコンテキストウィンドウは作業記憶で、ツールはそこに「一度読めば二度と全文は要らないもの」を流し込む。400 行の `Compiling …`、関数ひとつが欲しかっただけの 5,000 行のソース、9 割が lockfile の `git diff`、同じファイルの 3 回目の Read。その全行が以降の全ターンで課金され、ウィンドウが埋まるほど、本来頼んだ仕事の精度が落ちる。

モデルに「節約して」と頼んでも効かない。モデルの外側で強制すれば効く。

## ctxgate がやること

ctxgate は Claude Code の hook として動く。ツール実行前（`PreToolUse`）にリクエストを書き換え、実行後（`PostToolUse`）にモデルが見るものを決める。守る規則は 4 つ、この順で。

1. **何も失わない。** 大きな出力はまず content-addressed な vault に書く。モデルは常に id と、元の任意の断片を取り出す方法を知っている。
2. **バイトではなく形を見せる。** ソースはシンボル表に、テスト実行は合計・失敗名・各失敗の stanza に、diff はファイル表と変更行に、再取得は 1 行に、編集後の再 Read は差分になる。
3. **ウィンドウが埋まるほど強く絞る。** セッションの transcript から実トークン使用量を読み、4 段階で閾値を締める。誰も頼まなくても、最後の 3 割は守られる。
4. **fail open。** 壊れた入力、無い helper、書けないディスク。どの場合も元の出力がそのまま通り、hook はエージェントを止めない。

ctxgate の判断はすべてコードが下す。モデルには任せない。

## モデルが見るもの

**テスト実行。** `cargo test --workspace` が 12 KB 吐いたとき、モデルが受け取るのは:

```
[ctxgate] cargo test output: 403 lines / 12 KB → vaulted as ctx:9258de38065e (NOT in context).
  source: cargo test --workspace
  full text: ctxgate show 9258de38065e --grep <re> | --around <re> [n] | --lines A-B
cargo test: FAILED — 238 passed, 3 failed, 0 ignored
failed:
  parser::match_nested
  wasm::rc_release
  typecheck::generic_union
── parser::match_nested ──
  thread 'parser::match_nested' panicked at src/parser.rs:412:9:
  assertion `left == right` failed
    left: Some(Expr::Match { arms: 2 })
   right: Some(Expr::Match { arms: 3 })
```

**大きなファイル。** 4,720 行の Rust（生で 64k トークン）の Read は目次になる:

```
[ctxgate] Read src/cmds/git/git.rs: 4720 lines, rust, 231 symbols → vaulted as ctx:b0c8c343b22f (NOT in context)
  one symbol: ctxgate show b0c8c343b22f --symbol <name>   |   a range: Read offset=N limit=M
── outline ──
  enum    GitCommand                     L20-34
  fn      run_diff                       L112-216
  struct  HunkHeader                     L500-508
  fn      HunkHeader::consume            L530-550
  fn      compact_diff                   L648-821
  …
  mod     tests                          L2657-4720
  … 147 test symbols collapsed
```

**diff。** lockfile を含む 795 行の `git diff` は 39 行になる:

```
git diff: 8 files, +111 −113
  M  src/lower.rs                       +3 −3      4 hunks
  M  package-lock.json                  +100 −100  lockfile — hunks omitted
  A  src/new.rs                         +5
  R  src/rename_me.rs → src/renamed.rs
── src/lower.rs @@ -3 +3 @@ fn f1() -> i32 { 1 } ──
   fn f4() -> i32 { 4 }
  -fn f5() -> i32 { 5 }
  +fn f5() -> i32 { 500 } // changed
   fn f6() -> i32 { 6 }
```

**再取得。** 変わっていないファイルの 2 回目の Read:

```
[ctxgate] Read src/config.almd: unchanged since you last saw it this session (76 lines / 3 KB, ctx:4826ab3f8d38).
```

もっと要るときは、要る分だけ取りに行く:

```bash
ctxgate show b0c8c343b22f --symbol compact_diff     # 関数ひとつを実ファイルから
ctxgate show 9258de --around "rc_release" 20        # id は前方一致で省略可
ctxgate show 9258de38065e --lines 180-220
```

## 数字

ctxgate 自身を開発したセッションでの実測（Claude Code 2.1、1M トークンモデル）:

| 状況 | 生 | モデルが見る量 |
|---|---|---|
| `cargo test`、403 行 | 12 KB | 1.1 KB |
| 4,720 行の Rust の Read | 64k トークン | 6.9 KB の目次 |
| `git diff`、8 ファイル（lockfile 込み） | 795 行 / 17 KB | 39 行 / 1.4 KB |
| `seq 1 4000`（Claude Code が永続化） | 65 KB | 0.7 KB |
| 同じファイルの 2 回目の Read | 3 KB | 1 行 |
| 作業セッション 1 本の合計 | 195 KB（≈50k トークン）をウィンドウの外に | |

hook のオーバーヘッドは 2.4 MB の transcript 読み込み込みで 1 回 20〜70 ms。サイズはバイト、トークンは rtk と同じく bytes / 4 の推定。

### ベンチマーク（実トークン）

`bench/bench.almd` は同じタスクを `claude -p` で ctxgate あり・なしの新しい clone で走らせ、JSON 結果の実 usage を読む。rtk のコードベース（10 万行、1,700 コミット）に対する読み取り専用の探索タスク 5 本、Sonnet、各 1 回。1 行ずつはノイズ、合計が信号:

| タスク | off: 入力トークン / turn | on: 入力トークン / turn |
|---|---|---|
| `main.rs`（4,190 行）を読んで概要 | 123,715 / 2 | 158,097 / 3 |
| 4,720 行のファイルの `run_*` 関数一覧 | 99,439 / 3 | 99,468 / 3 |
| `git log --stat -60` のホットスポット | 148,469 / 3 | 197,758 / 4 |
| `git diff HEAD~8 HEAD` のレビュー | 383,194 / 7 | 637,751 / 11 |
| `src/` 全体の `.unwrap()` 監査（広い grep） | 1,582,949 / 27 | 743,112 / 28 |
| **合計** | **2,337,766** | **1,836,186（−21%）** |

成功率は両モード 5/5。正直に書くべき点が 2 つ:

- **turn 数が支配的。** 1 turn 増えるごとに文脈全体が再送されるので、圧縮した出力のせいでモデルが聞き直すと、生の出力より高くつく。このベンチの初回はまさにそれで、ベースラインより 6% **悪化**した。ページ Read（`offset` / `limit`）が全文 Read と dedup され、Grep の `head_limit` が `count` モードにも注入されていたため。どちらも修正済み。ページ Read は素通り、`head_limit` は予算圧迫時のみ。
- **短いタスクは不利なケース。** ctxgate の節約はセッションの長さで複利になる。ウィンドウの外に置いたものは、以降の全 turn で外のまま。4,720 行のファイルを抱えた 30 turn のセッションはそれを 30 回払うが、`-p` の 1 回実行は 1 回しか払わない。それでも上のベンチは正味で得をしているが、開発中のセッションで出た 88% という数字は長いセッションの効果。

自分で回すには: `almide run bench/bench.almd -- --repo <clone> --runs 3 --model sonnet`。

## できること

| | |
|---|---|
| **Vault** | content-addressed（`sha256` 接頭辞が id、同一内容の再退避はゼロコスト）。`show` の `--grep` / `--around` / `--lines` / `--head` / `--tail` / `--symbol`。`list`、`stats` |
| **shape を保った置換** | Bash `stdout`、Read `file.content`、`content` / `text` / `output`、MCP の `content[]` 配列。同じ形で差し戻し、隣のフィールドは触らない |
| **シンボル目次** | Rust / Go / TypeScript / TSX / JavaScript / Python は tree-sitter、Almide は自前パーサ。目次が溢れたらテストモジュールは 1 行に畳む |
| **ランナー解析** | `cargo test` / `build` / `clippy`、`go test` / `build`、`pytest`、`vitest`、`jest`、`tsc`。コマンドから判定し、`make test` のような包みは出力の形から判定 |
| **git** | `diff` / `show` / `log -p`: ファイル表と絞った hunk。lockfile・生成物・バイナリは stat のみ。`status` と `log` は 1 項目 1 行 |
| **Context Budgeter** | transcript 末尾の `assistant.usage` から使用量を取得。ウィンドウは自動判定（`…[1m]` なら 1M、他は 200k）。NORMAL → COMPRESS 40% → AGGRESSIVE 60% → ISOLATE 75%。短くなった理由をバナー 1 行でモデルに伝える |
| **セッション内 dedup** | 同一出力は 1 行。変更は行 diff（共通接頭辞・接尾辞を剥いで LCS）を前後 1 行付きで。Claude Code が compaction したらそのセッションの記憶を捨てる |
| **Grep / Glob** | Grep の一致をファイルごとに件数付きでまとめ、サンプルを上限付きで見せる。Glob はディレクトリごとの件数と一部のファイル名に。全件は vault に残る |
| **秘密情報** | 資格情報の形をした文字列（クラウドのキー、GitHub / Anthropic / OpenAI / Slack / Stripe のトークン、JWT、bearer、`PASSWORD=` 形式の代入、PEM 秘密鍵）を vault・dedup・モデルに渡す前に `[REDACTED:kind]` に伏せる。ctxgate が意図的に保存しない唯一のもの |
| **保持期間** | `ctxgate gc` が 14 日（`CTXGATE_RETAIN_DAYS`）より古い vault とセッション記録を消す。hook が 1 日 1 回自動で実行するので、放置しても vault は肥大化しない |
| **compaction をまたぐ記憶** | セッション日誌が置換 1 件につき 1 行を残す（`Bash cargo test → cargo test: FAILED — 238 passed, 3 failed  ctx:9258…`）。compaction は transcript の `isCompactSummary` 行と `PreCompact` / `PostCompact` hook で正確に検出し、dedup の記憶を捨て（「前に見た」と言わないため）、`compact` / `resume` の SessionStart hook で `ctxgate recall`（読んだファイル一覧 + 見たものの時系列 + vault id）をモデルに渡す。要約で何か落ちたと感じたら、モデル自身が `ctxgate recall` を呼べる |
| **ステータスライン** | `ctxgate statusline` が Claude Code のステータスライン JSON を読み、正確なコンテキスト % をセッションに記録（Budgeter は新しい間それを優先）、`ctxgate COMPRESS 48% · saved 299 KB` を 1 行出す |
| **自己調律** | 置換のせいでモデルが聞き直した（`ctxgate show`、再 Read、目次化したファイルへの Grep、4 分以内の同じコマンド）ら、その種類の置換に対する *miss* として記録する。miss が続く種類（直近で 3 回以上かつ 50% 以上）はセッションの残りで緩める: 閾値を上げる、行数を増やす、その表示を止める。`ctxgate report` で hit / miss / 緩めた種類が見える。指標はバイトではなく turn |
| **モデルへの案内** | `init` が CLAUDE.md にマーカー付きの短いブロックを書く（再実行で更新）。`show --symbol`、`--grep`、`recall` を必要になる前に知っている状態にする |
| **rtk 委譲** | [rtk](https://github.com/rtk-ai/rtk) があれば Bash コマンドを先に `rtk rewrite` に通し、rtk の allow / ask / deny 契約を守る。コマンド面は rtk、その上は ctxgate |
| **Claude Code の実態に合わせた処理** | 30 KB 超の出力を Claude Code が永続化するファイルを読み、失敗検出を全文に効かせる。モデルにはその先頭 2 KB しか見えないことを知って描画する。ページ Read には `lines A-B of N` を付ける |
| **CLI** | `ctxgate summarize "<cmd>" < output` で hook と同じ解析を CI や端末で使える |

## インストール

```bash
almide install github.com/O6lvl4/ctxgate        # → ~/.local/bin/ctxgate

# シンボル目次（任意。無ければ Read は head/tail 表示に落ちる）
git clone https://github.com/O6lvl4/ctxgate && cd ctxgate/tools/ctxgate-outline
cargo build --release && cp target/release/ctxgate-outline ~/.local/bin/

# Bash コマンド層（任意）
brew install rtk

cd your-project && ctxgate init        # .claude/settings.json
ctxgate init --global                  # または ~/.claude/settings.json
```

Claude Code を再起動するか `/hooks` を実行。`init` は再実行時に自分のエントリを置き換えるので、アップグレードは `ctxgate init` をもう一度打つだけ。最初のツール呼び出しの後に `ctxgate status` で動作を確認できる。

ctxgate は [Almide](https://github.com/almide/almide) で書かれている。`almide install` はランタイム不要の単一ネイティブバイナリ（約 800 KB）を作る。

## コマンド

```
ctxgate show <id> [--grep RE] [--around RE [N]] [--lines A-B] [--head N] [--tail N] [--symbol NAME]
ctxgate outline <file>             ソースのシンボル表
ctxgate summarize "<command>"      stdin → hook が出すのと同じ要約
ctxgate list [N]                   最近の vault エントリ
ctxgate stats                      累計: 退避したバイト数と見せたバイト数
ctxgate status                     現在のセッションの使用量と予算レベル
ctxgate recall [N]                 セッションの要約: 読んだファイル、置換した出力の時系列
ctxgate report                     種類別の置換数と miss、緩めているもの、節約量
ctxgate init [--global]            hook の登録
ctxgate gc [--days N] [--dry-run]  N 日より古い vault エントリを削除
ctxgate statusline                 ステータスライン用の 1 行（Claude Code のステータス JSON を stdin に）
ctxgate hook pre | post | session  hook 本体（stdin に JSON）
```

ステータスラインに出すには、Claude Code が実行するスクリプトにこれを足す:

```bash
seg=$(printf '%s' "$input" | ctxgate statusline 2>/dev/null) && [[ -n "$seg" ]] && segments+=("$seg")
```

## 設定

環境変数のみ。設定ファイルはない。

| 変数 | 既定 | 意味 |
|---|---|---|
| `CTXGATE_HOME` | `~/.ctxgate` | vault の場所 |
| `CTXGATE_MAX_BASH` / `_READ` / `_GREP` / `_OTHER` | 8000 / 12000 / 8000 / 10000 | このバイト数を超えたら退避 |
| `CTXGATE_HEAD` / `CTXGATE_TAIL` | 30 / 30 | 汎用表示で両端に残す行数 |
| `CTXGATE_SALIENT` | 40 | 拾う error 系行の上限 |
| `CTXGATE_LINE_CLIP` | 200 | これより長い行は切る |
| `CTXGATE_GREP_HEAD_LIMIT` | 60 | `head_limit` の無い Grep に注入する値 |
| `CTXGATE_OUTLINE_BIN` / `CTXGATE_OUTLINE_MAX` | `ctxgate-outline` / 120 | 目次 helper と表示上限 |
| `CTXGATE_MAX_BLOCK` | 20 | 失敗 stanza / hunk の行数上限 |
| `CTXGATE_WINDOW` | 自動 | コンテキストウィンドウ（トークン） |
| `CTXGATE_LVL_COMPRESS` / `_AGGRESSIVE` / `_ISOLATE` | 40 / 60 / 75 | 予算レベル（ウィンドウの %） |
| `CTXGATE_DEDUP_MIN` | 600 | このバイト数以上を dedup 対象に（0 で無効） |
| `CTXGATE_DIFF_MAX_CELLS` | 4,000,000 | 再 Read 差分の LCS 上限 |
| `CTXGATE_RTK` / `CTXGATE_RTK_BIN` | 1 / `rtk` | rtk 委譲の on/off とバイナリ |
| `CTXGATE_REDACT` | 1 | 資格情報の形をした文字列を伏せる（0 で無効） |
| `CTXGATE_RETAIN_DAYS` | 14 | `gc` と 1 日 1 回の自動 gc の保持日数（0 で無期限） |
| `CTXGATE_DEBUG` | | `1` で hook の生入力を `<home>/debug/` に保存 |

## 比較

| | rtk | token-crunch | Claude Code 単体 | ctxgate |
|---|---|---|---|---|
| 範囲 | Bash コマンド | PostToolUse のテキスト | Read のキャップ、出力の永続化 | 全ツールの出力 + リクエスト |
| 生の出力 | 捨てる（失敗時のみ tee） | 捨てる | 巨大な Bash だけ永続化 | 常に退避、常に取り出せる |
| ソースファイル | `rtk read` のシグネチャ | 汎用の畳み込み | 25k トークンのキャップ | 完全なシンボル表と `--symbol` 取得 |
| テスト / ビルド出力 | 100 超のコマンドフィルタ | 構造ヒューリスティック | なし | ランナー解析、コンパイルエラー対応 |
| コンテキスト認識 | なし | compaction の促し | 自動 compaction | 4 段階の予算が全閾値を動かす |
| 再取得 | なし | dedup | なし | dedup + 編集差分、compaction 対応 |
| 併用 | ctxgate は rtk があれば Bash を委譲する | | | |

rtk は成熟した Bash 層で、ctxgate は競合せずその上に乗る。

## 設計

- **プロンプトではなく hook。** モデルが節約を思い出す必要はない。`PreToolUse updatedInput` がリクエストを、`PostToolUse updatedToolOutput` が結果を形作る。
- **中核は純粋関数。** 各パーサ・レンダラは文字列に対する純粋関数で、それぞれに test ブロックと実フィクスチャがある。IO はその外側の薄い殻。
- **Almide でない部品は 1 つ、境界は 1 枚。** シンボル目次は JSON を吐くだけの小さな Rust helper で、判断はしない。Almide 版 tree-sitter ができたら helper を差し替えるだけで、呼び出し側は変わらない。
- **Claude Code の実態に合わせて測った。** hook のペイロード形状、30 KB の永続化規則、2 KB のプレビュー、25k の Read キャップ、transcript の形式は、ドキュメントではなく実セッションから採取した。

```
src/
  hook_pre.almd    リクエスト整形: Grep head_limit、rtk 委譲
  hook_post.almd   出力整形: shape 抽出、永続化ファイル、要約チェーン
  vault.almd       store + index          budget.almd    Context Budgeter
  compress.almd    汎用表示               dedup.almd     セッション記憶
  outline.almd     シンボル表             textdiff.almd  行 diff
  testout.almd     ランナー解析           gitout.almd    git
tools/ctxgate-outline/   Rust + tree-sitter → {path, lang, total_lines, symbols:[{kind,name,start,end}]}
```

```bash
almide check && almide test && almide build --release -o bin/ctxgate
```

## ロードマップ

- ベンチマーク: 同じタスクを ctxgate あり・なしで走らせ、バイト推定ではなく実トークン数とタスク成功率で測る。
- `init` が CLAUDE.md に短い使い方を書き、モデルが必要になる前に `show --symbol` を知っている状態にする。
- Heavy Task Router: 探索的なツール連打を検知して subagent に振る。
- Codex / Gemini CLI / Cursor の hook 形式。

## ライセンス

MIT または Apache-2.0、お好みで。
