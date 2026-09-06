# Changelog

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
