# ctxgate

**An agent context firewall for Claude Code.**

ctxgate sits between Claude Code's tools and the model. Every tool output passes through it
before it reaches the context window: the raw text is kept in a local vault, the model sees a
compressed view, and it can pull exactly the part it needs back out.

```
Claude Code ── Read / Grep / Bash / MCP ──▶ ctxgate ──▶ Claude
                                              │
                                              ▼
                                       ~/.ctxgate/store   (raw, content-addressed)
```

[日本語版 README](README_ja.md)

## Why

Expensive models should make decisions, not read logs. A `cargo test` run, a 5,000-line source
file, a `git diff` with a lockfile in it — each of those is tens of thousands of tokens the model
reads once and never needs in full again. Tools like [rtk](https://github.com/rtk-ai/rtk) make
*Bash commands* cheaper. ctxgate manages *what enters the context at all*, across Bash, Read,
Grep, Glob, WebFetch and MCP tools, and it never throws information away: everything is one
`ctxgate show` away.

Measured on the session that built it: **195 KB (≈50k tokens) of tool output kept out of a 1M
window**, a 4,720-line Rust file read as a 6.9 KB symbol table, a 795-line diff shown in 39 lines.

## What it does

| Layer | What the model sees instead of the raw output |
|---|---|
| **Output Vault** (v0.1) | Anything over a size budget is stored as `~/.ctxgate/store/<sha256>.txt` and replaced by salient lines (errors, failures) + head + tail with an explicit "N lines omitted" marker |
| **AST-aware Read** (v0.2) | A large source file becomes a symbol table (kind · name · line range) for Rust, Go, TypeScript, JavaScript, Python and Almide. `ctxgate show <id> --symbol name` returns just that function |
| **Test / build parsers** (v0.3) | `cargo test`, `cargo build/clippy`, `go test`, `pytest`, `vitest`, `jest`, `tsc`: totals, the names that failed, each failure's own stanza. Compile noise and passing tests disappear |
| **git compression** (v0.4) | `git diff/show/log -p`: a file table plus only the changed lines with one line of context; lockfiles, generated files and binaries are stats only. `git status` and `git log` in one line per item |
| **Context Budgeter** (v0.5) | Reads the current token usage from the session transcript and tightens every threshold as the window fills: NORMAL → COMPRESS (40%) → AGGRESSIVE (60%) → ISOLATE (75%) |
| **Session dedup** (v0.6) | A second identical Read or command becomes one line ("unchanged since you last saw it"); a re-read after an edit becomes just the diff. Memory resets when Claude Code compacts |
| **rtk delegation** (v0.7) | If [rtk](https://github.com/rtk-ai/rtk) is installed, Bash commands are rewritten through `rtk rewrite` before they run. rtk owns the command surface; ctxgate owns the vault, the budget and dedup on top |

Everything fails open: malformed input, a missing helper, an unwritable vault — the original
output passes through untouched, and the hook never exits non-zero.

## Example

`cargo test --workspace` prints 12 KB. The model receives:

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

A Read of a 4,720-line file receives a table of contents instead of page one:

```
[ctxgate] Read src/cmds/git/git.rs: 4720 lines, rust, 231 symbols → vaulted as ctx:b0c8c343b22f (NOT in context)
  one symbol: ctxgate show b0c8c343b22f --symbol <name>   |   a range: Read offset=N limit=M
── outline ──
  enum    GitCommand                     L20-34
  fn      run_diff                       L112-216
  struct  HunkHeader                     L500-508
  fn      HunkHeader::consume            L530-550
  fn      compact_diff                   L648-821
  ...
  mod     tests                          L2657-4720
  … 147 test symbols collapsed
```

When the model needs more, it asks for exactly that:

```bash
ctxgate show b0c8c343b22f --symbol compact_diff     # one function, from the real file
ctxgate show 9258de --around "rc_release" 20        # ids can be abbreviated
ctxgate show 9258de38065e --lines 180-220
```

## Install

ctxgate is written in [Almide](https://github.com/almide/almide); the only non-Almide piece is
a small tree-sitter helper in Rust for the symbol outlines.

```bash
almide install github.com/O6lvl4/ctxgate                  # → ~/.local/bin/ctxgate
# outlines for Rust/Go/TS/JS/Python (optional; without it Read falls back to head/tail)
git clone https://github.com/O6lvl4/ctxgate && cd ctxgate/tools/ctxgate-outline
cargo build --release && cp target/release/ctxgate-outline ~/.local/bin/

cd your-project
ctxgate init              # adds the hooks to .claude/settings.json
ctxgate init --global     # or ~/.claude/settings.json
```

Restart Claude Code (or run `/hooks`). Optional: `brew install rtk` to get the Bash layer too.

`init` is idempotent and replaces its own entries on re-run, so upgrading is `ctxgate init` again.

## Commands

```
ctxgate hook pre | post            hook handlers (JSON on stdin)
ctxgate show <id> [--grep RE] [--around RE [N]] [--lines A-B] [--head N] [--tail N] [--symbol NAME]
ctxgate outline <file>             symbol table of a source file
ctxgate summarize "<command>"      stdin = that command's output → the same summary the hook would produce
ctxgate list [N]                   recent vault entries
ctxgate stats                      bytes vaulted vs bytes shown, all time
ctxgate status                     context usage and budget level of the latest session
ctxgate init [--global]            register the hooks
```

## Configuration

Environment variables only (no config file yet).

| Variable | Default | Meaning |
|---|---|---|
| `CTXGATE_HOME` | `~/.ctxgate` | vault location |
| `CTXGATE_MAX_BASH` / `_READ` / `_GREP` / `_OTHER` | 8000 / 12000 / 8000 / 10000 | bytes above which an output is vaulted |
| `CTXGATE_HEAD` / `CTXGATE_TAIL` | 30 / 30 | lines kept at each end |
| `CTXGATE_SALIENT` | 40 | max error-like lines surfaced |
| `CTXGATE_LINE_CLIP` | 200 | longer lines are clipped |
| `CTXGATE_GREP_HEAD_LIMIT` | 60 | `head_limit` injected into Grep calls without one |
| `CTXGATE_OUTLINE_BIN` / `CTXGATE_OUTLINE_MAX` | `ctxgate-outline` / 120 | outline helper and max symbols shown |
| `CTXGATE_MAX_BLOCK` | 20 | max lines per failure stanza / hunk |
| `CTXGATE_WINDOW` | auto (1M if the model is `…[1m]`, else 200k) | context window in tokens |
| `CTXGATE_LVL_COMPRESS` / `_AGGRESSIVE` / `_ISOLATE` | 40 / 60 / 75 | budget levels, % of window |
| `CTXGATE_DEDUP_MIN` | 600 | outputs at least this big take part in dedup (0 disables) |
| `CTXGATE_DIFF_MAX_CELLS` | 4,000,000 | LCS budget for "changed since last seen" diffs |
| `CTXGATE_RTK` / `CTXGATE_RTK_BIN` | 1 / `rtk` | rtk delegation on/off, and the binary |
| `CTXGATE_DEBUG` | | `1` dumps raw hook input under `<home>/debug/` |

## Claude Code facts this relies on (measured on 2.1.x)

- Hooks receive `tool_response`. Bash: `{stdout, stderr, interrupted, isImage, noOutputExpected,
  persistedOutputPath?, persistedOutputSize?}`. Read: `{type:"text", file:{filePath, content,
  numLines, startLine, totalLines, truncatedByTokenCap}}`.
- Bash stdout above ~30 KB is persisted to a file by Claude Code and the hook gets the first
  30 KB; the model then sees only the first 2 KB of whatever the hook returns. ctxgate reads the
  persisted file for full salient detection and switches to a compact view.
- Read is capped at 25k tokens by Claude Code. ctxgate shrinks what is inside that cap.
- Replacement goes back through `hookSpecificOutput.updatedToolOutput` in the same shape as
  `tool_response`. PreToolUse rewrites go through `updatedInput`.
- The transcript at `transcript_path` is JSONL; the last `assistant` line's `usage` is the
  current prompt size (input + cache read + cache creation).

## Development

```bash
almide check                       # type check
almide test                        # test blocks in every module (fixtures in tests/fixtures)
almide build --release -o bin/ctxgate
echo '{"tool_name":"Bash","tool_input":{"command":"x"},"tool_response":{"stdout":"..."}}' | bin/ctxgate hook post
```

```
src/
  main.almd       CLI, show/list/stats/status/init
  config.almd     thresholds, env
  compress.almd   generic summary: salient / head / tail
  vault.almd      content-addressed store + index.jsonl
  hook_pre.almd   PreToolUse: Grep head_limit, rtk delegation
  hook_post.almd  PostToolUse: shape extraction, persisted output, Read outline, summarizer chain
  outline.almd    symbol tables (.almd parsed here, others via tools/ctxgate-outline)
  testout.almd    cargo / go / pytest / vitest / jest / tsc parsers
  gitout.almd     git diff / status / log
  budget.almd     Context Budgeter
  dedup.almd      session dedup
  textdiff.almd   line diff (prefix/suffix strip + LCS)
tools/
  ctxgate-outline/  Rust + tree-sitter; emits {path, lang, total_lines, symbols:[{kind,name,start,end}]}
```

The Rust helper is deliberately dumb: it prints JSON and makes no decisions, so the planned
Almide tree-sitter can replace it without touching callers.

## Reference projects

| Repo | What was studied |
|---|---|
| [rtk-ai/rtk](https://github.com/rtk-ai/rtk) | PreToolUse `updatedInput` rewriting, fail-open contract, raw output tee; now used directly for the Bash layer |
| [micaelmalta/token-crunch](https://github.com/micaelmalta/token-crunch) | shape-aware `updatedToolOutput` replacement, session-level dedup |

## Roadmap

- Benchmark harness: the same tasks with and without ctxgate, real token counts and task success, not byte estimates
- `init` writes a short usage note into CLAUDE.md so the model knows about `show --symbol` and `summarize`
- Heavy Task Router: detect exploratory tool storms and steer them to a subagent
- Hook formats for Codex, Gemini CLI and Cursor

## License

Dual-licensed under MIT or Apache-2.0, at your option.
