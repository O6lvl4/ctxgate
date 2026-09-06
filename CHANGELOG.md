# Changelog

## Unreleased
- ctxgate-outline: 11 more tree-sitter grammars (C, C++, Java, Ruby, C#, PHP, Bash, Lua, Kotlin, Swift, Scala); keyword tokens no longer produce phantom symbols. Usable standalone as peek's `PEEK_OUTLINE_BIN`.
- Generic head/tail view only for outputs over 12 KB and never for model-narrowed commands (grep / sed -n / awk / peek …).
- The CLAUDE.md note recommends peek; hooks are registered globally by `init --global`.

## 0.11.0 — 2026-09-06
- NORMAL is turn-safe: defaults Bash/Grep/Other 30 KB, Read 1 MB (only outputs Claude Code itself truncates, plus repeats, are replaced); levels are absolute (COMPRESS 8k/60k, AGGRESSIVE 4k/24k, ISOLATE 3k/12k for Bash/Read) and never touch Grep/Glob. n=3 Sonnet bench: 0.10 defaults were +13% worse; 0.11 is neutral on short tasks and −16% / −22% with the levels forced on the two long tasks, 3/3 success throughout.
- `ctxgate report` now shows what actually fills the context window (since the last compaction): tool output vs harness attachments vs the model's own Write/Edit/Bash inputs. ctxgate governs only the first; the table makes a small saving number readable.
- Finding from the Claude Code binary: a file the model has read that is changed outside Edit/Write (shell patch, script) makes the harness attach its diff (`edited_text_file`, up to 16 KB per turn) — no hook sees it. The CLAUDE.md block now tells the model to edit with Edit/Write. Session records carry the transcript path.

## 0.10.0 — 2026-09-06
- A generic replacement that would not shrink the output below 60% is skipped (the original passes through); shorter hint line; `ctxgate doctor`.
- Budget levels re-tuned from dogfooding at 65%: thresholds fall to 3/4, 1/2, 1/3 (floors 3 KB / 8 KB) instead of 1/2, 1/4, 1.5 KB; views shrink to 15/15 and 10/10 lines instead of 10/10 and 6/6; shorter banner.
- Self-tuning: recovery calls after a replacement are recorded as misses per replacement kind; kinds that keep missing are softened for the session. `ctxgate report`.
- `init` writes a ctxgate block into CLAUDE.md (project, or ~/.claude/CLAUDE.md with --global).
- Bash outputs that already went through rtk get a doubled budget before ctxgate intervenes again.

## 0.9.0 — 2026-09-06
- Session journal: one line per replaced output (tool, label, the summary's first sentence, vault id); `ctxgate recall [N]` prints files read + timeline.
- Exact compaction detection from the transcript (`isCompactSummary`) plus `PreCompact` / `PostCompact` hooks: dedup memory reset, journal divider. The usage-drop heuristic is gone.
- `SessionStart(compact|resume)` now returns `recall` (files + timeline) instead of a bare id list.
- Bench-driven fixes: paged Reads pass through untouched; Grep `head_limit` only under budget pressure and only in `content` mode; `CTXGATE_MAX_READ` default 24000.

## 0.8.0 — 2026-09-06
- Grep results grouped per file with counts and a capped sample; Glob results as a per-directory tree (shapes taken from cli.js 2.1.x: Grep `content`, Glob `filenames`).
- SessionStart hook (`compact|resume`): prints a digest of this session's vault entries so the model keeps its vault ids across compaction.
- `ctxgate statusline`: reads Claude Code's status-line JSON, records the authoritative `context_window.used_percentage` per session (the Budgeter prefers it while fresh), prints one segment; `ctxgate status --line`.
- Secrets: credential-shaped strings (AWS/GitHub/Anthropic/OpenAI/Slack/Google/Stripe keys, JWTs, bearer tokens, `PASSWORD=`-style assignments, PEM private keys) are masked before the vault, dedup and the model. Small outputs containing one are replaced too. `CTXGATE_REDACT=0` disables.
- Retention: `ctxgate gc [--days N] [--dry-run]` drops vault entries and session records older than `CTXGATE_RETAIN_DAYS` (14); hooks run it at most once a day.
- E2E tests: real captured PostToolUse payloads (`tests/hooks/`) piped through `hook_post.run`, checking the envelope Claude Code would receive.
- Workaround for almide/almide#1931 (top-level record list iteration).

## 0.7.0 — 2026-09-06
- rtk delegation: with `rtk` on PATH, Bash commands go through `rtk rewrite` in PreToolUse, honouring rtk's exit-code contract (0 allow, 3 ask, 1/2 untouched). `CTXGATE_RTK=0` disables.
- `init` now replaces its own hook entries on re-run; PreToolUse matcher is `Bash|Grep`.
- English README; Japanese README moved to `README_ja.md`; dual MIT/Apache license.

## 0.6.0
- Session dedup: identical repeat → one line; changed → line diff (prefix/suffix strip + LCS). Applies to outputs ≥ `CTXGATE_DEDUP_MIN` (600 B). Session memory is cleared when usage drops ≥30% (compaction).

## 0.5.0
- Context Budgeter: current usage from the transcript's last `assistant.usage`; window auto-detect (1M for `[1m]` models); NORMAL / COMPRESS 40% / AGGRESSIVE 60% / ISOLATE 75% tighten every threshold; banner line; `ctxgate status`.

## 0.4.0
- git: `diff` / `show` / `log -p` → file table + squeezed hunks, lockfile/generated/binary as stats only; `status` and `log` one line per item.

## 0.3.0
- Runner parsers: cargo test/build/clippy, go test/build, pytest, vitest, jest, tsc; `ctxgate summarize "<cmd>"`.

## 0.2.0
- AST-aware Read: symbol outline via `tools/ctxgate-outline` (Rust + tree-sitter) for Rust/Go/TS/TSX/JS/Python, Almide parsed in Almide; `show --symbol`; `ctxgate outline`; test-module collapsing.

## 0.1.0
- Output vault, shape-aware PostToolUse replacement (Bash / Read / content / text / output / MCP arrays), persisted-output handling, Grep `head_limit` injection, `show` / `list` / `stats` / `init`, fail-open.
