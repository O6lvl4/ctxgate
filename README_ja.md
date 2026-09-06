# ctxgate

Claude Code のコンテキスト・ファイアウォール。ツールの hook に入り、生の出力はすべてローカルの
vault に保存しておき、モデルにどこまで見せるかを決める。

[English](README.md) · [Changelog](CHANGELOG.md)

## やること

1. **窓に余裕がある間は、何もしない。** モデルは ctxgate なしと同じ出力を見る。
   計測済み: トークン数も答えも同じ。
2. **窓が 40% 埋まったら、絞る。** 大きい Bash 出力、大きい Read、`git diff` を短い表示
   （テストの結果、シンボル一覧、ファイル表）に置き換え、生の本文は id 付きで vault に保存する。
   Sonnet での計測: 長いタスクで入力トークン 16〜22% 減、成功率は変わらず。
3. **検索は絶対に絞らない。** Grep / Glob の結果はモデルが探しているものそのもの。
   隠せば再検索になるだけ。

レベルに関係なく常に効くもの:

- **繰り返し** — 変わっていないファイルの再 Read、同じコマンドの再実行は 1 行になる。
- **巨大な出力** — Claude Code 自身が 2 KB に切り詰める出力が、ちゃんとした要約になる。
- **compaction 後の recall** — 何を見たかの時系列を vault の id 付きで返す。
- **秘密情報** — 認証情報らしき文字列はモデルに届く前にマスクする。
- **report** — `ctxgate report` が窓を埋めているものの内訳を出す: ツール出力、ハーネスの添付、
  自分の編集。ctxgate が縮められるのは最初の一つだけ。

## モデルが見るもの

絞っている時、12 KB の `cargo test` はこうなる:

```
[ctxgate] cargo test output: 403 lines / 12 KB → vaulted as ctx:9258de38065e (NOT in context).
  full text: ctxgate show 9258de38065e [--grep RE | --around RE N | --lines A-B | --tail N]
cargo test: FAILED — 238 passed, 3 failed, 0 ignored
failed:
  parser::match_nested
  wasm::rc_release
── parser::match_nested ──
  thread 'parser::match_nested' panicked at src/parser.rs:412:9:
  assertion `left == right` failed
```

4,720 行の Rust ファイルは目次になり、シンボル 1 つを名前で取り出せる:

```
[ctxgate] Read src/cmds/git/git.rs: 4720 lines, rust, 231 symbols → vaulted as ctx:b0c8c343b22f
  enum    GitCommand                     L20-34
  fn      run_diff                       L112-216
  fn      compact_diff                   L648-821
  …
  mod     tests                          L2657-4720   (147 test symbols collapsed)
```

```bash
ctxgate show b0c8c343b22f --symbol run_diff
```

変わっていないファイルの 2 回目の Read は、どのレベルでも:

```
[ctxgate] Read src/config.almd: unchanged since you last saw it this session (76 lines / 3 KB, ctx:4826ab3f8d38).
```

## インストール

```bash
almide install github.com/O6lvl4/ctxgate        # ネイティブバイナリ 1 つ → ~/.local/bin/ctxgate
cd your-project && ctxgate init                 # .claude/settings.json に hook、CLAUDE.md に案内
```

Claude Code を再起動（または `/hooks`）。`ctxgate doctor` で設定を確認できる。

任意:

```bash
# Rust / Go / TypeScript / Python のシンボル目次（Almide は内蔵）
git clone https://github.com/O6lvl4/ctxgate && cd ctxgate/tools/ctxgate-outline
cargo build --release && cp target/release/ctxgate-outline ~/.local/bin/

# rtk: 入っていれば Bash コマンドを `rtk rewrite` に通す
brew install rtk
```

## 期待していい効果

`bench/bench.almd` で計測: 同じタスクを ctxgate あり/なしで `claude -p` に流し、run ごとに
新しい clone、JSON 結果の実 usage を読む。Sonnet、各モード 3 回。

| 状況 | 入力トークン | 成功 |
|---|---|---|
| 短いタスク 5 つ、既定設定 | なしと同じ（±ノイズ） | 両方 15/15 |
| `git diff` レビュー、窓が詰まった状態 | **−16%** | 両方 3/3 |
| リポジトリ全体の `.unwrap()` 監査、窓が詰まった状態 | **−22%** | 両方 3/3 |

数字から学んだこと 2 つ。既定値がこうなっている理由でもある:

- **モデルに聞き直させる要約は負ける。** turn が増えるたびに窓全体が再送される。最初の既定値
  （初手から絞る）は ctxgate なしより 13% *悪かった*。丸ごと必要な目次をモデルがページ送り
  したから。だから、余裕がある間は何もしない。
- **バイト数はトークン数ではない。** 「40% 節約」と出す道具は、自分が触った出力のバイト数を
  数えている。モデルは回り道して（`git diff --no-compact`、もう一度 Read）結局同じ大きさの
  窓になる。出力ではなく請求額を測ること。

計測は Sonnet のみ。自分で回すには:

```bash
almide run bench/bench.almd -- --repo <clone> --runs 3 --model sonnet
```

## コマンド

```
ctxgate show <id> [--grep RE] [--around RE N] [--lines A-B] [--head N] [--tail N] [--symbol NAME]
                                   vault から必要な部分だけ取り出す
ctxgate recall [N]                 このセッションで読んだファイルと、置き換えた出力の時系列（compaction 後に使う）
ctxgate report                     置き換えた回数、モデルが取りに戻った回数、窓を占めているものの内訳
ctxgate status                     現在のコンテキスト使用率とレベル
ctxgate list [N]                   vault の最近のエントリ
ctxgate stats                      保存したバイト数と見せたバイト数の累計
ctxgate gc [--days N]              古いエントリを削除
ctxgate outline <file>             ソースファイルのシンボル一覧
ctxgate summarize "<command>"      stdin の内容を hook と同じ形に要約して表示
ctxgate init [--global]            hook を登録する
ctxgate doctor                     インストール状態の確認
ctxgate statusline                 ステータスライン用の 1 区画（Claude Code の status JSON を stdin に渡す）
```

id は他と区別できる長さまで省略できる。

## 設定

環境変数のみ、設定ファイルはない。重要なもの:

| 変数 | 既定 | |
|---|---|---|
| `CTXGATE_LVL_COMPRESS` / `_AGGRESSIVE` / `_ISOLATE` | 40 / 60 / 75 | 各レベルが始まる窓の % |
| `CTXGATE_WINDOW` | auto | 窓のトークン数（モデル名に `[1m]` があれば 1M、なければ 200k） |
| `CTXGATE_MAX_BASH` / `_READ` | 30000 / 1000000 | NORMAL で Bash / Read を置き換えるバイト数。レベルで 8k/60k、4k/24k、3k/12k に下がる |
| `CTXGATE_REDACT` | 1 | 秘密情報のマスク（0 で無効） |
| `CTXGATE_RETAIN_DAYS` | 14 | vault の保持日数（0 で無期限） |
| `CTXGATE_RTK` | 1 | rtk 委譲（0 で無効） |
| `CTXGATE_HOME` | `~/.ctxgate` | vault の置き場所 |

その他: `CTXGATE_HEAD` / `_TAIL`（30）、`_SALIENT`（40）、`_LINE_CLIP`（200）、`_MAX_BLOCK`（20）、
`_OUTLINE_MAX`（120）、`_DEDUP_MIN`（600）、`_MAX_GREP` / `_MAX_OTHER`（30000）。

## アーキテクチャ

![ctxgate architecture](docs/architecture.svg)

上段が 1 回のツール呼び出しの流れ。hook post の中で、マスク → vault に保存 → transcript から
レベル決定 → 既視判定、の順に進み、そのまま通す / 1 行 / 行の差分 / 要約表示のどれかになる。
モデルは `ctxgate show` で vault から必要な部分だけ取り出せる。下段はコンテキストウィンドウの
埋まり具合で決まるレベル。Grep と Glob はレベルの対象外。モデルに聞き直させ続ける種類の置き換え
（*miss*）は、レベルに関係なくそのセッションの残りで緩める。

## hook の役割

- **PreToolUse** — rtk があれば Bash コマンドを渡す。要約が隠したものをモデルが取りに戻った
  呼び出し（*miss*）を記録し、miss が続く種類はそのセッションの残りで緩める。
- **PostToolUse** — 生の出力を `~/.ctxgate/store` に保存（内容アドレス）、セッションの transcript
  から今のコンテキスト量を読んでレベルを決め、ツールが出したのと同じ JSON の形で表示を返す。
- **PreCompact / SessionStart** — 繰り返しの記憶をリセットし、compaction 後に recap を渡す。

表示の種類: `cargo test` / `go test` / `pytest` / `vitest` / `jest` / `tsc` の結果、`git diff` /
`show` / `log` / `status`、tree-sitter のシンボル目次、Grep のファイル別まとめと Glob のツリー、
汎用の先頭 / 末尾 / エラー行。hook のオーバーヘッドは 20〜70 ms。

[Almide](https://github.com/almide/almide) 製。MIT / Apache-2.0 のデュアルライセンス。
