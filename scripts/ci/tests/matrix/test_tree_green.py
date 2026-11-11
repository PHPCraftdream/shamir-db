import io
import json
import os
import sys
import unittest
from contextlib import redirect_stdout
from types import SimpleNamespace

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../matrix")))

import tree_green as tg


AFTER = "a" * 40
HEAD = "b" * 40
AFTER_TREE = "c" * 40
OTHER_TREE = "d" * 40
RUNS_ENDPOINT = (
    "actions/workflows/ci.yml/runs?event=pull_request&status=success&per_page=30"
)


def response(data, returncode=0):
    return SimpleNamespace(stdout=json.dumps(data), stderr="", returncode=returncode)


def commit(sha, tree):
    return {
        "sha": sha,
        "commit": {
            "tree": {
                "sha": tree,
            },
        },
    }


def run(run_id=123, sha=HEAD):
    return {
        "id": run_id,
        "event": "pull_request",
        "status": "completed",
        "conclusion": "success",
        "head_sha": sha,
    }


def api_runner(responses):
    calls = []

    def runner(argv, **kwargs):
        calls.append(argv)
        endpoint = argv[-1].split("repos/example/project/", 1)[-1]
        value = responses[endpoint]
        if isinstance(value, list):
            value = value.pop(0)
        return value

    return runner, calls


def green_run_responses(gate_conclusion="success"):
    return {
        RUNS_ENDPOINT: response(
            {
                "workflow_runs": [
                    run(),
                ],
            }
        ),
        "commits/" + AFTER: response(commit(AFTER, AFTER_TREE)),
        "commits/" + HEAD: response(commit(HEAD, AFTER_TREE)),
        "actions/runs/123/jobs?per_page=100": response(
            {
                "jobs": [
                    {
                        "name": "merge gate (required)",
                        "conclusion": gate_conclusion,
                    },
                ],
            }
        ),
    }


class TreeGreenTest(unittest.TestCase):
    def test_same_tree_with_successful_required_gate_is_green(self):
        runner, calls = api_runner(green_run_responses())
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (True, "matching_full_run"),
        )
        self.assertEqual(len(calls), 4)
        self.assertEqual(calls[0][1], "api")

    def test_different_trees_are_not_green(self):
        runner, _ = api_runner(
            {
                RUNS_ENDPOINT: response(
                    {
                        "workflow_runs": [
                            run(),
                        ],
                    }
                ),
                "commits/" + AFTER: response(commit(AFTER, AFTER_TREE)),
                "commits/" + HEAD: response(commit(HEAD, OTHER_TREE)),
            }
        )
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "no_matching_full_run"),
        )

    def test_api_failure_fails_closed(self):
        runner, _ = api_runner({RUNS_ENDPOINT: response({}, returncode=1)})
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "api_error"),
        )

    def test_malformed_response_fails_closed(self):
        runner, _ = api_runner(
            {
                RUNS_ENDPOINT: response(
                    {
                        "workflow_runs": [
                            {"id": 1},
                        ],
                    }
                ),
            }
        )
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "malformed_response"),
        )

    def test_success_without_full_tier_gate_is_not_green(self):
        responses = green_run_responses()
        responses["actions/runs/123/jobs?per_page=100"] = response(
            {
                "jobs": [
                    {
                        "name": "fast",
                        "conclusion": "success",
                    },
                ],
            }
        )
        runner, _ = api_runner(responses)
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "no_matching_full_run"),
        )

    def test_query_is_bounded_to_last_30_runs(self):
        runner, calls = api_runner(
            {
                RUNS_ENDPOINT: response(
                    {
                        "workflow_runs": [],
                    }
                ),
            }
        )
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "no_matching_full_run"),
        )
        self.assertIn("per_page=30", calls[0][-1])

    def test_query_does_not_follow_additional_pages(self):
        runner, calls = api_runner(
            {
                RUNS_ENDPOINT: response(
                    {
                        "workflow_runs": [],
                    }
                ),
            }
        )
        tg.check_tree_green("example/project", AFTER, runner)
        self.assertEqual(len(calls), 1)
        self.assertNotIn("--paginate", calls[0])

    def test_response_over_30_runs_fails_closed(self):
        runner, _ = api_runner(
            {
                RUNS_ENDPOINT: response(
                    {
                        "workflow_runs": [
                            run(run_id=index + 1) for index in range(31)
                        ],
                    }
                ),
            }
        )
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "malformed_response"),
        )

    def test_failed_required_gate_is_not_green(self):
        runner, _ = api_runner(green_run_responses(gate_conclusion="failure"))
        self.assertEqual(
            tg.check_tree_green("example/project", AFTER, runner),
            (False, "no_matching_full_run"),
        )

    def test_cli_outputs_fixed_safe_reasons(self):
        output = io.StringIO()
        with redirect_stdout(output):
            result = tg.main(
                ["--repo", "example/project", "--after", AFTER],
                runner=lambda *args, **kwargs: response({"workflow_runs": []}),
            )
        self.assertEqual(result, 0)
        self.assertEqual(
            output.getvalue(),
            "tree_green=false\nreason=no_matching_full_run\n",
        )

    def test_command_arguments_do_not_use_shell(self):
        options = []

        def runner(argv, **kwargs):
            self.assertIsInstance(argv, list)
            options.append(kwargs)
            return response(
                {
                    "workflow_runs": [],
                }
            )

        tg.check_tree_green("example/project", AFTER, runner)
        self.assertTrue(all(call.get("shell") is not True for call in options))


if __name__ == "__main__":
    unittest.main()
