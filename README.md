# ctxgate

**The context firewall for Claude Code.**
Tool output never reaches the model raw. It goes into a local vault, the model gets a view
small enough to think with, and it can pull any detail back on demand.

```
Claude Code ── Bash · Read · Grep · Glob · WebFetch · MCP ──▶ ctxgate ──▶ the model
                                                                │
                                                                ▼
                                                     ~/.ctxgate/store  (raw, content-addressed)
```

[日本語](README_ja.md) · [Changelog](CHANGELOG.md)

---

## The problem

An agent's context window is its working memory, and tools flood it with things it will read
once and never need in full again: 400 lines of `Compiling …`, a 5,000-line source file when it
wanted one function, a `git diff` that is 90% lockfile, the same file re-read three times.
Every one of those lines is paid for on every subsequent turn, and when the window fills, the
agent gets worse at the thing you actually asked for.

Asking the model to "be economical" does not work. Enforcing it outside the model does.

## What ctxgate does

ctxgate runs as Claude Code hooks: before a tool runs (`PreToolUse`) it can rewrite the request,
after it runs (`PostToolUse`) it decides what the model sees. Four rules, in this order:

1. **Nothing is lost.** Every large output is written to a content-addressed vault first.
   The model always knows the id and how to get any slice of the original back.
2. **Show the shape, not the bytes.** A source file becomes its symbol table. A test run becomes
   the totals, the failing names and each failure's own stanza. A diff becomes the file list plus
   the changed lines. A repeat becomes one line; a re-read after an edit becomes the diff.
3. **Squeeze harder as the window fills.** ctxgate reads the real token usage from the session
   transcript and tightens every threshold through four levels, so the last 30% of the window is
   protected without anyone asking.
4. **Fail open.** Malformed input, a missing helper, an unwritable disk: the original output
   passes through untouched and the hook never blocks the agent.

Everything decided in ctxgate is decided by code, not by the model.

## What the model sees

**A test run.** `cargo test --workspace` prints 12 KB; the model receives:

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

**A big file.** A Read of a 4,720-line Rust file (64k tokens raw) receives a table of contents:

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

**A diff.** 795 lines of `git diff` with a lockfile in it become 39 lines:

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

**A repeat.** The second Read of an unchanged file:

```
[ctxgate] Read src/config.almd: unchanged since you last saw it this session (76 lines / 3 KB, ctx:4826ab3f8d38).
```

And when the model needs more, it asks for exactly that much:

```bash
ctxgate show b0c8c343b22f --symbol compact_diff     # one function, from the real file
ctxgate show 9258de --around "rc_release" 20        # ids can be abbreviated
ctxgate show 9258de38065e --lines 180-220
```

## Numbers

Measured on the sessions that built ctxgate (Claude Code 2.1, 1M-token model):

| Situation | Raw | Model sees |
|---|---|---|
| `cargo test`, 403 lines | 12 KB | 1.1 KB |
| Read of a 4,720-line Rust file | 64k tokens | 6.9 KB outline |
| `git diff`, 8 files incl. lockfile | 795 lines / 17 KB | 39 lines / 1.4 KB |
| `seq 1 4000` (persisted by Claude Code) | 65 KB | 0.7 KB |
| Second Read of the same file | 3 KB | 1 line |
| One working session, all tools | 195 KB (≈50k tokens) kept out of the window | |

Hook overhead is 20–70 ms per call including reading a 2.4 MB transcript. Sizes are bytes;
token counts are estimated as bytes / 4, the same approximation rtk uses.

### Benchmark (real tokens)

`bench/bench.almd` runs the same tasks through `claude -p` with and without ctxgate in fresh
clones and reads the real usage from the JSON result. Five read-only exploration tasks on the
rtk codebase (100k lines, 1,700 commits), Sonnet, one run each — so treat single rows as noise
and the total as the signal:

| task | off: input tokens / turns | on: input tokens / turns |
|---|---|---|
| read `main.rs` (4,190 lines) and outline it | 123,715 / 2 | 158,097 / 3 |
| list `run_*` fns in a 4,720-line file | 99,439 / 3 | 99,468 / 3 |
| `git log --stat -60` hotspots | 148,469 / 3 | 197,758 / 4 |
| review `git diff HEAD~8 HEAD` | 383,194 / 7 | 637,751 / 11 |
| audit `.unwrap()` across `src/` (wide greps) | 1,582,949 / 27 | 743,112 / 28 |
| **total** | **2,337,766** | **1,836,186 (−21%)** |

Success was 5/5 in both modes. Two honest notes:

- **Levels do not chase bytes.** Budget levels shrink what a summary shows (head/tail lines, outline length, stanza length) far more than they lower the size at which outputs get replaced, because a 3 KB output is cheaper inline than as a summary plus a retry. The self-tuning loop softens any kind that still misses.
- **Turns dominate.** Every extra turn re-sends the whole context, so a compressed output that
  makes the model ask again costs more than the raw output would have. The first run of this
  benchmark was 6% *worse* than baseline for exactly that reason: paged Reads (`offset`/`limit`)
  were being deduplicated against the whole-file read, and Grep `head_limit` was injected in
  `count` mode. Both are fixed; paged Reads now pass through untouched and `head_limit` only
  applies under budget pressure.
- **Short tasks are the unfavourable case.** ctxgate's savings compound with session length,
  because everything it keeps out of the window stays out on every later turn. A 30-turn
  session with a 4,720-line file in it pays for that file 30 times; a `-p` run pays once. The
  benchmark above still shows a net gain, but the long-session effect is where the 88% figure
  from the sessions that built ctxgate comes from.

Run it yourself: `almide run bench/bench.almd -- --repo <clone> --runs 3 --model sonnet`.

## Capabilities

| | |
|---|---|
| **Vault** | Content-addressed store (`sha256` prefix as id, so a repeat costs nothing). `show` with `--grep`, `--around`, `--lines`, `--head`, `--tail`, `--symbol`. `list`, `stats` |
| **Shape-aware replacement** | Bash `stdout`, Read `file.content`, `content` / `text` / `output`, MCP `content[]` arrays: the replacement goes back in the same shape, sibling fields untouched |
| **Symbol outlines** | Rust, Go, TypeScript, TSX, JavaScript, Python via tree-sitter; Almide via its own parser. Test modules collapse to one line when the outline overflows |
| **Runner parsers** | `cargo test` / `build` / `clippy`, `go test` / `build`, `pytest`, `vitest`, `jest`, `tsc`. Detected from the command, or from the output shape for `make test`-style wrappers |
| **git** | `diff` / `show` / `log -p`: file table, squeezed hunks, lockfiles / generated / binaries as stats only. `status` and `log` in one line per item |
| **Context Budgeter** | Usage from the transcript's last `assistant.usage`; window auto-detected (1M for `…[1m]` models, else 200k); NORMAL → COMPRESS 40% → AGGRESSIVE 60% → ISOLATE 75%; a one-line banner tells the model why outputs got shorter |
| **Session dedup** | Identical output → one line. Changed → line diff (prefix/suffix strip + LCS) with one line of context. Forgets the session when Claude Code compacts |
| **Grep / Glob** | Grep matches grouped per file with counts and a capped sample; Glob results as a per-directory tree with counts. The full list stays in the vault |
| **Secrets** | Credential-shaped strings (cloud keys, GitHub / Anthropic / OpenAI / Slack / Stripe tokens, JWTs, bearer tokens, `PASSWORD=` style assignments, PEM private keys) are masked as `[REDACTED:kind]` before the vault, the dedup store and the model. The one thing ctxgate deliberately does not preserve |
| **Retention** | `ctxgate gc` drops vault entries and session records older than 14 days (`CTXGATE_RETAIN_DAYS`); hooks run it at most once a day, so the vault stays bounded without attention |
| **Compaction memory** | A session journal keeps one line per replaced output (`Bash cargo test → cargo test: FAILED — 238 passed, 3 failed  ctx:9258…`). Compactions are detected exactly (the transcript's `isCompactSummary` line, plus `PreCompact` / `PostCompact` hooks): the dedup memory is reset so nothing is called "unchanged since you saw it", and a `SessionStart` hook on `compact` / `resume` hands the model `ctxgate recall`: the files it had read and the timeline of what it had seen, with vault ids. The model can run `ctxgate recall` itself whenever it feels the summary lost something |
| **Status line** | `ctxgate statusline` reads Claude Code's status-line JSON, records the authoritative context percentage for the session (the Budgeter prefers it while fresh) and prints `ctxgate COMPRESS 48% · saved 299 KB` |
| **Self-tuning** | A replacement that makes the model ask again (a `ctxgate show`, a re-read, a grep into the file it just outlined, the same command twice within 4 minutes) is recorded as a *miss* against that kind of replacement. When a kind keeps missing (≥3 misses and ≥50% of its recent replacements) it is softened for the rest of the session: bigger budgets, more lines, or the view is switched off. `ctxgate report` shows hits, misses and what is softened. Turns are the metric, not bytes |
| **Model onboarding** | `init` writes a short block into CLAUDE.md (between markers, refreshed on re-run) so the model knows `show --symbol`, `--grep`, `recall` before it needs them |
| **rtk delegation** | With [rtk](https://github.com/rtk-ai/rtk) installed, Bash commands are rewritten through `rtk rewrite` first, honouring rtk's allow/ask/deny contract. rtk owns the command surface; ctxgate owns everything above it |
| **Claude Code specifics** | Reads the file Claude Code persists for >30 KB outputs so failure detection covers the whole thing; knows the model only sees 2 KB of it and renders accordingly; annotates paged Reads with `lines A-B of N` |
| **CLI** | `ctxgate summarize "<cmd>" < output` applies the same parsers outside the hook, for CI or a terminal |

## Install

```bash
almide install github.com/O6lvl4/ctxgate        # → ~/.local/bin/ctxgate

# symbol outlines (optional; without it, Read falls back to head/tail)
git clone https://github.com/O6lvl4/ctxgate && cd ctxgate/tools/ctxgate-outline
cargo build --release && cp target/release/ctxgate-outline ~/.local/bin/

# Bash command layer (optional)
brew install rtk

cd your-project && ctxgate init        # .claude/settings.json
ctxgate init --global                  # or ~/.claude/settings.json
```

Restart Claude Code or run `/hooks`. `init` replaces its own entries on re-run, so upgrading is
`ctxgate init` again. Check it is working with `ctxgate status` after the first tool call.

ctxgate is written in [Almide](https://github.com/almide/almide); `almide install` builds a
single native binary (~800 KB, no runtime).

## Commands

```
ctxgate show <id> [--grep RE] [--around RE [N]] [--lines A-B] [--head N] [--tail N] [--symbol NAME]
ctxgate outline <file>             symbol table of a source file
ctxgate summarize "<command>"      stdin → the summary the hook would produce
ctxgate list [N]                   recent vault entries
ctxgate stats                      bytes vaulted vs bytes shown, all time
ctxgate status                     context usage and budget level of the current session
ctxgate recall [N]                 recap of the session: files read, timeline of replaced outputs
ctxgate report                     replacements vs misses per kind, what is softened, savings
ctxgate init [--global]            register the hooks
ctxgate gc [--days N] [--dry-run]  drop vault entries older than N days
ctxgate statusline                 status-line segment (pipe Claude Code's status JSON in)
ctxgate hook pre | post | session  the hook entry points (JSON on stdin)
```

To show the segment in your status line, add to the script Claude Code runs:

```bash
seg=$(printf '%s' "$input" | ctxgate statusline 2>/dev/null) && [[ -n "$seg" ]] && segments+=("$seg")
```

## Configuration

Environment variables; there is no config file.

| Variable | Default | Meaning |
|---|---|---|
| `CTXGATE_HOME` | `~/.ctxgate` | vault location |
| `CTXGATE_MAX_BASH` / `_READ` / `_GREP` / `_OTHER` | 8000 / 12000 / 8000 / 10000 | bytes above which an output is vaulted |
| `CTXGATE_HEAD` / `CTXGATE_TAIL` | 30 / 30 | lines kept at each end in the generic view |
| `CTXGATE_SALIENT` | 40 | max error-like lines surfaced |
| `CTXGATE_LINE_CLIP` | 200 | longer lines are clipped |
| `CTXGATE_GREP_HEAD_LIMIT` | 60 | `head_limit` injected into Grep calls that lack one |
| `CTXGATE_OUTLINE_BIN` / `CTXGATE_OUTLINE_MAX` | `ctxgate-outline` / 120 | outline helper, max symbols listed |
| `CTXGATE_MAX_BLOCK` | 20 | max lines per failure stanza or hunk |
| `CTXGATE_WINDOW` | auto | context window in tokens |
| `CTXGATE_LVL_COMPRESS` / `_AGGRESSIVE` / `_ISOLATE` | 40 / 60 / 75 | budget levels, % of window |
| `CTXGATE_DEDUP_MIN` | 600 | outputs at least this big take part in dedup (0 disables) |
| `CTXGATE_DIFF_MAX_CELLS` | 4,000,000 | LCS budget for re-read diffs |
| `CTXGATE_RTK` / `CTXGATE_RTK_BIN` | 1 / `rtk` | rtk delegation on/off, binary |
| `CTXGATE_REDACT` | 1 | mask credential-shaped strings (0 disables) |
| `CTXGATE_RETAIN_DAYS` | 14 | vault retention for `gc` and the daily auto-gc (0 keeps forever) |
| `CTXGATE_DEBUG` | | `1` dumps raw hook input under `<home>/debug/` |

## How it compares

| | rtk | token-crunch | Claude Code alone | ctxgate |
|---|---|---|---|---|
| Scope | Bash commands | PostToolUse text | Read cap, output persistence | every tool's output, plus the request |
| Raw output | discarded (tee on failure) | discarded | persisted for very large Bash only | always vaulted, always retrievable |
| Source files | `rtk read` signatures | generic collapse | 25k-token cap | full symbol table, `--symbol` fetch |
| Test / build output | 100+ command filters | structure heuristics | none | runner parsers, compile-error aware |
| Context awareness | none | compaction nudge | auto-compact | four budget levels driving every threshold |
| Repeats | none | dedup | none | dedup + edit diff, compaction-aware |
| Together | ctxgate delegates Bash to rtk when present | | | |

rtk is the mature Bash layer and ctxgate uses it rather than competing with it.

## Design

- **Hooks, not prompts.** The model never has to remember to be frugal. `PreToolUse
  updatedInput` shapes the request; `PostToolUse updatedToolOutput` shapes the result.
- **Pure core.** Every parser and renderer is a pure function over strings with its own test
  block and real fixtures. IO lives in a thin shell around them.
- **One boundary for the one non-Almide piece.** Symbol outlines come from a small Rust helper
  that prints JSON and decides nothing. When Almide gets its own tree-sitter, the helper is
  swapped and no caller changes.
- **Measured against Claude Code as it is.** Hook payload shapes, the 30 KB persistence rule,
  the 2 KB preview, the 25k Read cap and the transcript format were all captured from live
  sessions, not from documentation.

```
src/
  hook_pre.almd    request shaping: Grep head_limit, rtk delegation
  hook_post.almd   output shaping: shape extraction, persisted files, summarizer chain
  vault.almd       store + index          budget.almd    Context Budgeter
  compress.almd    generic view           dedup.almd     session memory
  outline.almd     symbol tables          textdiff.almd  line diff
  testout.almd     runner parsers         gitout.almd    git
tools/ctxgate-outline/   Rust + tree-sitter → {path, lang, total_lines, symbols:[{kind,name,start,end}]}
```

```bash
almide check && almide test && almide build --release -o bin/ctxgate
```

## Roadmap

- A benchmark harness: identical tasks with and without ctxgate, real token counts and task
  success rate, not byte estimates.
- `init` writes a short usage note into CLAUDE.md so the model knows `show --symbol` exists
  before it needs it.
- Heavy Task Router: detect exploratory tool storms and steer them into a subagent.
- Hook formats for Codex, Gemini CLI and Cursor.

## License

MIT or Apache-2.0, at your option.
