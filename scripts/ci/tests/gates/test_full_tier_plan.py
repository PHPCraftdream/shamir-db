"""Unit tests for scripts/ci/gates/full_tier_plan.py (the `plan` job's tier decision)."""

import os
import sys
import tempfile
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "gates",
    ),
)

import full_tier_plan as ftp  # noqa: E402


class HasMarkerTests(unittest.TestCase):
    def test_marker_alone_on_a_body_line(self):
        self.assertTrue(ftp.has_marker("fix(x): y\n\nbody\n[ci full]\n"))

    def test_marker_as_the_whole_subject_line(self):
        self.assertTrue(ftp.has_marker("[ci full]"))

    def test_marker_tolerates_surrounding_blanks_and_crlf(self):
        self.assertTrue(ftp.has_marker("subject\r\n\r\n  [ci full]  \r\n"))

    def test_marker_in_prose_does_not_trigger(self):
        self.assertFalse(ftp.has_marker("docs: explain the [ci full] marker"))
        self.assertFalse(ftp.has_marker("subject\n\nuse [ci full] to run everything\n"))

    def test_marker_is_case_sensitive_and_exact(self):
        self.assertFalse(ftp.has_marker("[CI FULL]"))
        self.assertFalse(ftp.has_marker("[ci  full]"))
        self.assertFalse(ftp.has_marker("[ci full] extra"))
        self.assertFalse(ftp.has_marker("ci full"))

    def test_empty_message(self):
        self.assertFalse(ftp.has_marker(""))


class DecideTests(unittest.TestCase):
    def test_non_pull_request_events_are_always_full(self):
        for event in ("push", "workflow_dispatch", "schedule"):
            self.assertTrue(ftp.decide(event, False, ""), event)
            self.assertTrue(ftp.decide(event, True, ""), event)

    def test_ready_pull_request_is_full_without_marker(self):
        self.assertTrue(ftp.decide("pull_request", False, "plain message"))

    def test_draft_pull_request_is_fast_without_marker(self):
        self.assertFalse(ftp.decide("pull_request", True, "plain message"))

    def test_draft_pull_request_is_full_with_marker(self):
        self.assertTrue(ftp.decide("pull_request", True, "subject\n\n[ci full]\n"))

    def test_draft_pull_request_with_prose_mention_stays_fast(self):
        self.assertFalse(ftp.decide("pull_request", True, "mention [ci full] inline"))


class MainTests(unittest.TestCase):
    def run_main(self, event, draft, message):
        with tempfile.TemporaryDirectory() as tmp:
            message_path = os.path.join(tmp, "message.txt")
            output_path = os.path.join(tmp, "github_output")
            with open(message_path, "w", encoding="utf-8") as handle:
                handle.write(message)
            previous = os.environ.get("GITHUB_OUTPUT")
            os.environ["GITHUB_OUTPUT"] = output_path
            try:
                code = ftp.main(
                    [
                        "--event",
                        event,
                        "--draft",
                        draft,
                        "--message-file",
                        message_path,
                    ]
                )
            finally:
                if previous is None:
                    del os.environ["GITHUB_OUTPUT"]
                else:
                    os.environ["GITHUB_OUTPUT"] = previous
            with open(output_path, encoding="utf-8") as handle:
                return code, handle.read()

    def test_draft_without_marker_writes_full_false(self):
        self.assertEqual(self.run_main("pull_request", "true", "plain\n"), (0, "full=false\n"))

    def test_draft_with_marker_writes_full_true(self):
        self.assertEqual(
            self.run_main("pull_request", "true", "plain\n\n[ci full]\n"),
            (0, "full=true\n"),
        )

    def test_dispatch_writes_full_true(self):
        self.assertEqual(self.run_main("workflow_dispatch", "false", ""), (0, "full=true\n"))

    def test_unreadable_message_file_fails_closed(self):
        code = ftp.main(
            [
                "--event",
                "pull_request",
                "--draft",
                "true",
                "--message-file",
                os.path.join(tempfile.gettempdir(), "no-such-dir-xyz", "message.txt"),
            ]
        )
        self.assertEqual(code, 1)


if __name__ == "__main__":
    unittest.main()
