"""Offline CLI integration tests. Run: python3 tests/codex_integration.py /path/to/ctxgate."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

BIN = str(Path(sys.argv.pop(1)).resolve())


class CodexIntegration(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ctxgate-codex-test-")
        self.root = Path(self.tmp.name)
        self.vault = self.root / "vault"
        self.env = dict(os.environ, CTXGATE_HOME=str(self.vault),
                        CODEX_THREAD_ID="test-thread", CTXGATE_RTK="0", CTXGATE_PEEK="0")
        for name in ("CTXGATE_WINDOW", "CTXGATE_MAX_BASH", "CTXGATE_DEDUP_MIN"):
            self.env.pop(name, None)

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, *args, data=None, cwd=None):
        return subprocess.run([BIN, *args], input=data, text=True, capture_output=True,
                              env=self.env, cwd=cwd or self.root, timeout=30)

    def hook(self, kind, **extra):
        payload = dict(session_id="test-thread", tool_name="Bash",
                       tool_input={"command": "ctxgate exec cargo test"}, **extra)
        return self.run_cli("hook", "codex", kind, data=json.dumps(payload))

    def test_argv_and_failure_preserved(self):
        arg = "spaces 'quotes' $(touch SHOULD_NOT_EXIST) ; dollar$"
        result = self.run_cli("exec", "--", sys.executable, "-c",
                              "import sys; print(sys.argv[1]); print('failure details', file=sys.stderr); sys.exit(7)", arg)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(result.stdout, arg + "\n")
        self.assertEqual(result.stderr, "failure details\n")
        self.assertFalse((self.root / "SHOULD_NOT_EXIST").exists())

    def verbose(self):
        # Logs go to stderr, as cargo build commonly does. The failure is in the
        # middle so a head/tail-only truncation would lose it.
        return self.run_cli("exec", sys.executable, "-c", "import sys; "
                            "print(''.join('test case_%d ... ok\\n'%i for i in range(2000)), file=sys.stderr); "
                            "print('test result: FAILED. 2000 passed; 1 failed; 0 ignored', file=sys.stderr); sys.exit(101)")

    def test_summary_vault_dedup_and_compaction(self):
        first = self.verbose()
        self.assertEqual(first.returncode, 101, first.stderr)
        self.assertIn("command exit code: 101", first.stdout)
        self.assertIn("FAILED", first.stdout)
        self.assertLess(len(first.stdout), 5000)
        match = re.search(r"ctx:([0-9a-f]+)", first.stdout)
        self.assertIsNotNone(match, first.stdout)
        raw = self.run_cli("show", match[1])
        self.assertIn("test case_1000 ... ok", raw.stdout)
        self.assertIn("FAILED", raw.stdout)
        self.assertIn(match[1], self.run_cli("recall").stdout)
        self.assertIn("unchanged since", self.verbose().stdout)
        self.assertEqual(self.hook("compact").stdout, "")
        self.assertNotIn("unchanged since", self.verbose().stdout)
        recall = self.hook("session")
        envelope = json.loads(recall.stdout)
        self.assertEqual(envelope["hookSpecificOutput"]["hookEventName"], "SessionStart")
        self.assertIn("ctx:", envelope["hookSpecificOutput"]["additionalContext"])

    def test_codex_budget_uses_last_input_not_cumulative_or_cached(self):
        transcript = self.root / "rollout.jsonl"
        transcript.write_text(json.dumps({"type": "event_msg", "payload": {
            "type": "token_count", "info": {"model_context_window": 100000,
            "total_token_usage": {"input_tokens": 9999999},
            "last_token_usage": {"input_tokens": 45000, "cached_input_tokens": 44000}}}}) + "\n")
        result = self.hook("pre", transcript_path=str(transcript))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")  # no approval / command rewrite
        state = json.loads((self.vault / "sessions/codex-test-thread.json").read_text())
        self.assertEqual((state["used"], state["window"], state["pct"]), (45000, 100000, 45))
        self.assertEqual(state["level"], "COMPRESS")
        self.env["CTXGATE_WINDOW"] = "200000"
        self.hook("pre", transcript_path=str(transcript))
        state = json.loads((self.vault / "sessions/codex-test-thread.json").read_text())
        self.assertEqual(state["pct"], 22)

    def test_pressure_changes_wrapper_threshold(self):
        self.env["CTXGATE_DEDUP_MIN"] = "0"
        script = "print(''.join('test case_%d ... ok\\n'%i for i in range(700))); print('test result: ok. 700 passed; 0 failed; 0 ignored')"
        plain = self.run_cli("exec", sys.executable, "-c", script)
        self.assertNotIn("[ctxgate]", plain.stdout)
        transcript = self.root / "rollout.jsonl"
        transcript.write_text(json.dumps({"type": "event_msg", "payload": {
            "type": "token_count", "info": {"model_context_window": 100000,
            "last_token_usage": {"input_tokens": 45000}}}}) + "\n")
        self.hook("pre", transcript_path=str(transcript))
        compressed = self.run_cli("exec", sys.executable, "-c", script)
        self.assertEqual(compressed.returncode, 0, compressed.stderr)
        self.assertIn("vaulted as ctx:", compressed.stdout)
        self.assertLess(len(compressed.stdout), len(plain.stdout) // 2)

    def test_cwd_changes_do_not_deduplicate_other_project(self):
        self.verbose()
        other = self.root / "other"
        other.mkdir()
        # Same argv, different cwd.
        saved = self.root
        self.root = other
        try:
            self.assertNotIn("unchanged since", self.verbose().stdout)
        finally:
            self.root = saved

    def test_init_preserves_other_hooks_and_is_idempotent(self):
        config_dir = self.root / ".codex"
        config_dir.mkdir()
        hooks = config_dir / "hooks.json"
        hooks.write_text(json.dumps({"description": "mine", "hooks": {"PreToolUse": [
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "ctxgate hook codex pre"},
                                          {"type": "command", "command": "my-check"}]}]}}))
        agents = self.root / "AGENTS.md"
        agents.write_text("Existing instructions\n")
        first = self.run_cli("init", "--codex")
        self.assertEqual(first.returncode, 0, first.stderr)
        before = (hooks.read_text(), agents.read_text())
        self.assertEqual(self.run_cli("init", "--codex").returncode, 0)
        self.assertEqual(before, (hooks.read_text(), agents.read_text()))
        data = json.loads(before[0])
        self.assertEqual(data["description"], "mine")
        self.assertIn("my-check", before[0])
        self.assertEqual(before[0].count("ctxgate hook codex pre"), 1)
        self.assertNotIn("PostToolUse", data["hooks"])
        self.assertNotIn("PermissionRequest", data["hooks"])
        self.assertTrue(before[1].startswith("Existing instructions"))
        self.assertFalse((self.root / ".claude").exists())

    def test_global_respects_codex_home(self):
        self.env["CODEX_HOME"] = str(self.root / "custom-codex")
        result = self.run_cli("init", "--codex", "--global")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / "custom-codex/hooks.json").exists())
        self.assertTrue((self.root / "custom-codex/AGENTS.md").exists())

    def test_malformed_hook_is_noop_invalid_init_is_not_overwritten(self):
        result = self.run_cli("hook", "codex", "pre", data="not JSON")
        self.assertEqual((result.returncode, result.stdout), (0, ""))
        directory = self.root / ".codex"
        directory.mkdir()
        path = directory / "hooks.json"
        path.write_text("[invalid")
        self.assertNotEqual(self.run_cli("init", "--codex").returncode, 0)
        self.assertEqual(path.read_text(), "[invalid")

    def test_missing_program_is_failure(self):
        self.assertNotEqual(self.run_cli("exec", "/nonexistent/ctxgate-program").returncode, 0)

    def test_child_separator_is_preserved(self):
        result = self.run_cli("exec", sys.executable, "-c", "import sys; print(repr(sys.argv[1:]))", "--", "path with spaces")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "['--', 'path with spaces']\n")

    def test_stderr_secrets_are_masked_in_summary_and_vault(self):
        result = self.run_cli("exec", sys.executable, "-c", "import sys; print('AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE', file=sys.stderr)")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", result.stdout + result.stderr)
        self.assertIn("REDACTED", result.stdout)
        for entry in (self.vault / "store").glob("*.txt"):
            self.assertNotIn("AKIAIOSFODNN7EXAMPLE", entry.read_text())

    def test_invalid_hook_shape_is_preserved(self):
        directory = self.root / ".codex"
        directory.mkdir()
        path = directory / "hooks.json"
        for invalid in ('[]', '{"hooks": []}', '{"hooks":{"PreToolUse":"bad"}}'):
            path.write_text(invalid)
            self.assertNotEqual(self.run_cli("init", "--codex").returncode, 0)
            self.assertEqual(path.read_text(), invalid)


if __name__ == "__main__":
    unittest.main()
