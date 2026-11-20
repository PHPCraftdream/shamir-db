"""CI-only schema, expression contract, and exporter checks for backup rules."""

import copy
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
RULES = ROOT / "deploy/alerts/shamir-backup.rules.yml"
EXPORTER = (
    ROOT / "crates/server/shamir-server-security/src/security/backup_metrics.rs"
)

STAMP = "shamir_backup_last_success_timestamp_seconds"
START = "shamir_backup_schedule_started_timestamp_seconds"
ENABLED = "shamir_backup_schedule_enabled == 1"
EXPECTED = {
    "BackupStale": (
        f"(time() - (({STAMP} > 0) or (({START} > 0) and ({STAMP} == 0)))"
        f" > 129600) and {ENABLED}",
        "5m",
    ),
    "BackupFailing": (
        "shamir_backup_scheduled_failure_streak >= 3 and ignoring(result) "
        'shamir_backup_scheduled_runs_total{result="failed"} >= 3'
        f" and {ENABLED}",
        None,
    ),
    "BackupRetentionFailing": (
        "increase(shamir_backup_retention_failed_total[1h]) > 0",
        None,
    ),
}


def unique_keys(pairs):
    obj = {}
    for key, value in pairs:
        if key in obj:
            raise ValueError(f"duplicate YAML key: {key}")
        obj[key] = value
    return obj


class BackupRulesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # YAML permits a leading comment before the JSON-form document.
        body = RULES.read_text(encoding="utf-8").splitlines()
        while body and body[0].startswith("#"):
            body.pop(0)
        cls.document = json.loads("\n".join(body), object_pairs_hook=unique_keys)
        source = EXPORTER.read_text(encoding="utf-8")
        # Count recorder calls, not describe-only registrations or comments.
        cls.exported = set(re.findall(
            r'metrics::(?:gauge|counter)!\(\s*"(shamir_backup_\w+)"', source
        ))
        results = re.search(r'const SCHEDULED_RESULTS[^=]*=\s*\[(.*?)\];', source, re.S)
        cls.results = set(re.findall(r'"(\w+)"', results[1])) if results else set()

    def assert_rule_contract(self, document, exported):
        self.assertEqual(set(document), {"groups"})
        (group,) = document["groups"]
        self.assertEqual(set(group), {"name", "rules"})
        self.assertEqual(group["name"], "shamir-backup")
        self.assertCountEqual([rule["alert"] for rule in group["rules"]], EXPECTED)
        for rule in group["rules"]:
            expr, hold = EXPECTED[rule["alert"]]
            fields = {"alert", "expr", "annotations"} | ({"for"} if hold else set())
            self.assertEqual(set(rule), fields)
            self.assertEqual(rule.get("for"), hold)
            self.assertEqual(set(rule["annotations"]), {"summary"})
            self.assertTrue(rule["annotations"]["summary"].strip())
            actual = rule["expr"]
            refs = set(re.findall(r"\bshamir_backup_\w+\b", actual))
            self.assertFalse(refs - exported, f"unexported metrics: {refs - exported}")
            self.assertEqual(" ".join(actual.split()), expr)
            for result in re.findall(r'\bresult="(\w+)"', actual):
                self.assertIn(result, self.results)

    def test_shipped_rule_contract(self):
        self.assert_rule_contract(self.document, self.exported)

    def test_rejects_missing_export(self):
        for metric in (STAMP, START):
            with self.subTest(metric=metric):
                with self.assertRaisesRegex(AssertionError, "unexported metrics"):
                    self.assert_rule_contract(self.document, self.exported - {metric})

    def test_rejects_semantic_regressions(self):
        for alert, old, new in (
            ("BackupStale", f" and {ENABLED}", ""),
            ("BackupStale", f" and ({STAMP} == 0)", ""),
            ("BackupStale", f"{START} > 0", f"{START} >= 0"),
            ("BackupFailing", 'result="failed"', 'result="completed"'),
            ("BackupRetentionFailing", "[1h]", "[1m]"),
        ):
            with self.subTest(alert=alert, replaced=old):
                document = copy.deepcopy(self.document)
                rule = next(r for r in document["groups"][0]["rules"] if r["alert"] == alert)
                rule["expr"] = rule["expr"].replace(old, new, 1)
                with self.assertRaises(AssertionError):
                    self.assert_rule_contract(document, self.exported)
