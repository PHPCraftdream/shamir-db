"""unittest suite for scripts/ci/gates/dependency_admission_check.py.

Lockfiles, the exceptions file, the registry and the advisory database are all
injected; no subprocess, no filesystem fixture, no network.
"""

import datetime
import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "gates",
    ),
)

import dependency_admission_check as dac  # noqa: E402

SRC = dac.REGISTRY
CONFIG = '[registry]\nglobal-min-publish-age = "30 days"\n'
TODAY = datetime.date(2026, 10, 21)
UTC = datetime.timezone.utc

PUBLISHED = {
    ("old", "1.0.0"): datetime.datetime(2026, 1, 1, 12, 0, 0, tzinfo=UTC),
    ("fresh", "2.0.0"): datetime.datetime(2026, 10, 19, 19, 16, 54, tzinfo=UTC),
    ("dep", "3.0.0"): datetime.datetime(2026, 10, 19, 19, 15, 0, tzinfo=UTC),
    ("other", "4.0.0"): datetime.datetime(2026, 10, 20, 8, 0, 0, tzinfo=UTC),
}


def lock(*packages):
    """Render a Cargo.lock; each package is (name, version, [deps])."""
    out = ["version = 4\n"]
    for name, version, deps in packages:
        out.append("[[package]]")
        out.append(f'name = "{name}"')
        out.append(f'version = "{version}"')
        out.append(f'source = "{SRC}"')
        out.append(f'checksum = "sum-{name}-{version}"')
        if deps:
            rendered = ", ".join(f'"{d}"' for d in deps)
            out.append(f"dependencies = [{rendered}]")
        out.append("")
    return "\n".join(out)


def entry(name, version, **extra):
    published = PUBLISHED[(name, version)]
    expires = published.date() + datetime.timedelta(days=30)
    fields = {
        "package": name,
        "version": version,
        "source": SRC,
        "checksum": f"sum-{name}-{version}",
        "published": published.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "expires": expires.isoformat(),
        "kind": "security",
        "reason": "test",
        "approved_by": "owner",
        "approved_on": "2026-10-21",
    }
    fields.update(extra)
    lines = ["[[exception]]"]
    for key, value in fields.items():
        if isinstance(value, list):
            rendered = ", ".join(f'"{v}"' for v in value)
            lines.append(f"{key} = [{rendered}]")
        else:
            lines.append(f'{key} = "{value}"')
    lines.append("")
    return "\n".join(lines)


def registry():
    return dac.Registry(fetch=lambda n, v: PUBLISHED[(n, v)], pause=0)


def run(base, head, exceptions="", advisory_exists=lambda p, a: True, today=TODAY,
        expiry_only=False):
    return dac.check(
        {"Cargo.lock": base},
        {"Cargo.lock": head},
        exceptions,
        CONFIG,
        today,
        registry(),
        advisory_exists,
        expiry_only=expiry_only,
    )


class AdmissionTests(unittest.TestCase):
    def test_nothing_new_passes(self):
        base = lock(("old", "1.0.0", []))
        errors, warnings = run(base, base)
        self.assertEqual(errors, [])
        self.assertEqual(warnings, [])

    def test_mature_admission_needs_no_exception(self):
        base = lock(("other", "4.0.0", []))
        head = lock(("other", "4.0.0", []), ("old", "1.0.0", []))
        errors, _ = run(base, head)
        self.assertEqual(errors, [])

    def test_young_admission_without_exception_fails(self):
        base = lock(("old", "1.0.0", []))
        head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", []))
        errors, _ = run(base, head)
        self.assertEqual(len(errors), 1)
        self.assertIn("fresh 2.0.0", errors[0])
        self.assertIn("no matching entry", errors[0])

    def test_young_admission_with_exception_passes(self):
        base = lock(("old", "1.0.0", []))
        head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", []))
        errors, _ = run(base, head, entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]))
        self.assertEqual(errors, [])

    def test_young_version_already_in_base_is_not_an_admission(self):
        both = lock(("fresh", "2.0.0", []))
        errors, _ = run(both, both)
        self.assertEqual(errors, [])

    def test_checksum_mismatch_is_not_a_match(self):
        base = lock(("old", "1.0.0", []))
        head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", []))
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"], checksum="forged")
        errors, _ = run(base, head, text)
        self.assertTrue(any("no matching entry" in e for e in errors))
        self.assertTrue(any("unused exception" in e for e in errors))

    def test_pre_existing_lock_changes_are_not_admissions(self):
        base = lock(("old", "1.0.0", []))
        head = lock(("old", "1.0.0", []))
        errors, _ = run(base, head, entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]))
        self.assertTrue(any("unused exception" in e for e in errors))


class EntryRuleTests(unittest.TestCase):
    base = lock(("old", "1.0.0", []))
    head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", []))

    def errors_for(self, text, **kwargs):
        return run(self.base, self.head, text, **kwargs)[0]

    def test_published_must_match_registry(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"],
                     published="2026-10-19T00:00:00Z")
        self.assertTrue(any("differs from the registry" in e for e in self.errors_for(text)))

    def test_expires_must_be_maturity_date(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"], expires="2027-01-01")
        self.assertTrue(any("`expires` must be 2026-11-18" in e for e in self.errors_for(text)))

    def test_expired_entry_fails(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        errors, _ = run(self.base, self.head, text, today=datetime.date(2026, 11, 19))
        self.assertTrue(any("expired on 2026-11-18" in e for e in errors))

    def test_entry_warns_in_the_last_week(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        _, warnings = run(self.base, self.head, text, today=datetime.date(2026, 11, 12))
        self.assertEqual(len(warnings), 1)

    def test_expiry_day_itself_still_passes(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        errors, _ = run(self.base, self.head, text, today=datetime.date(2026, 11, 18))
        self.assertEqual(errors, [])

    def test_unknown_advisory_fails(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        errors = self.errors_for(text, advisory_exists=lambda package, adv: False)
        self.assertTrue(any("not found" in e for e in errors))

    def test_advisory_lookup_uses_package_and_id(self):
        seen = []
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        self.errors_for(text, advisory_exists=lambda p, a: seen.append((p, a)) or True)
        self.assertEqual(seen, [("fresh", "RUSTSEC-2026-0001")])

    def test_needs_exactly_one_of_advisories_or_companion(self):
        neither = entry("fresh", "2.0.0")
        both = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"], companion_of="x@1.0.0")
        self.assertTrue(any("exactly one of" in e for e in self.errors_for(neither)))
        self.assertTrue(any("exactly one of" in e for e in self.errors_for(both)))

    def test_malformed_advisory_id_fails(self):
        text = entry("fresh", "2.0.0", advisories=["CVE-2026-1"])
        self.assertTrue(any("malformed advisory id" in e for e in self.errors_for(text)))

    def test_wildcard_is_not_an_exception(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]).replace(
            'version = "2.0.0"', 'version = "2.*"'
        )
        self.assertTrue(any("wildcard" in e for e in self.errors_for(text)))

    def test_missing_field_fails(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]).replace(
            'approved_by = "owner"\n', ""
        )
        self.assertTrue(any("missing field `approved_by`" in e for e in self.errors_for(text)))

    def test_duplicate_entry_fails(self):
        one = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"])
        errors = self.errors_for(one + "\n" + one)
        self.assertTrue(any("duplicate" in e for e in errors))

    def test_kind_must_be_security(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"], kind="convenience")
        self.assertTrue(any("`kind` must be" in e for e in self.errors_for(text)))


class CompanionTests(unittest.TestCase):
    base = lock(("old", "1.0.0", []))

    def head(self, with_edge=True):
        deps = ["dep 3.0.0"] if with_edge else []
        return lock(("old", "1.0.0", []), ("fresh", "2.0.0", deps), ("dep", "3.0.0", []))

    def texts(self):
        return (
            entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]) + "\n"
            + entry("dep", "3.0.0", companion_of="fresh@2.0.0")
        )

    def test_companion_reachable_from_primary_passes(self):
        errors, _ = run(self.base, self.head(), self.texts())
        self.assertEqual(errors, [])

    def test_companion_not_reachable_fails(self):
        errors, _ = run(self.base, self.head(with_edge=False), self.texts())
        self.assertTrue(any("does not depend on dep 3.0.0" in e for e in errors))

    def test_companion_of_unknown_primary_fails(self):
        text = entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]) + "\n" + entry(
            "dep", "3.0.0", companion_of="ghost@9.9.9"
        )
        errors, _ = run(self.base, self.head(), text)
        self.assertTrue(any("is not an entry with advisories" in e for e in errors))

    def test_transitive_dependency_counts(self):
        head = lock(
            ("old", "1.0.0", []),
            ("fresh", "2.0.0", ["other 4.0.0"]),
            ("other", "4.0.0", ["dep 3.0.0"]),
            ("dep", "3.0.0", []),
        )
        errors, _ = run(self.base, head, self.texts())
        self.assertEqual([e for e in errors if "does not depend" in e], [])


class ExpiryOnlyTests(unittest.TestCase):
    def test_expiry_only_skips_admission_diff(self):
        head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", []))
        errors, _ = run("", head, entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]),
                        expiry_only=True)
        self.assertEqual(errors, [])

    def test_expiry_only_reports_expired(self):
        head = lock(("fresh", "2.0.0", []))
        errors, _ = run("", head, entry("fresh", "2.0.0", advisories=["RUSTSEC-2026-0001"]),
                        today=datetime.date(2026, 12, 1), expiry_only=True)
        self.assertTrue(any("expired" in e for e in errors))


class FetchFailureTests(unittest.TestCase):
    def test_registry_failure_fails_closed(self):
        def boom(name, version):
            raise RuntimeError(f"cannot read the publication time of {name} {version}: down")

        reg = dac.Registry(fetch=boom, pause=0)
        errors, _ = dac.check({"Cargo.lock": ""}, {"Cargo.lock": lock(("fresh", "2.0.0", []))},
                              "", CONFIG, TODAY, reg, lambda p, a: True)
        self.assertEqual(len(errors), 1)
        self.assertIn("cannot read the publication time", errors[0])


class ParsingTests(unittest.TestCase):
    def test_min_age_parses(self):
        self.assertEqual(dac.parse_min_age_days(CONFIG), 30)

    def test_min_age_rejects_other_units(self):
        with self.assertRaises(ValueError):
            dac.parse_min_age_days('[registry]\nglobal-min-publish-age = "4 weeks"\n')

    def test_dependency_forms(self):
        text = (
            'version = 4\n[[package]]\nname = "a"\nversion = "1.0.0"\n'
            'dependencies = ["b", "c 2.0.0", "d 3.0.0 (registry+https://x)"]\n'
            '[[package]]\nname = "b"\nversion = "1.5.0"\n'
        )
        graph = dac.build_graph(dac.parse_lock(text))
        self.assertEqual(graph[("a", "1.0.0")], {("b", "1.5.0"), ("c", "2.0.0"), ("d", "3.0.0")})

    def test_empty_lock_is_empty(self):
        self.assertEqual(dac.parse_lock(""), [])


class SuggestTests(unittest.TestCase):
    def test_suggest_marks_primary_and_companion(self):
        head = lock(("fresh", "2.0.0", ["dep 3.0.0"]), ("dep", "3.0.0", []), ("old", "1.0.0", []))
        out = dac.suggest(
            {"Cargo.lock": lock(("old", "1.0.0", []))}, {"Cargo.lock": head}, CONFIG, TODAY,
            registry(), {"fresh": ["RUSTSEC-2026-0001"]}, "why", "owner", "2026-10-21",
        )
        self.assertIn('advisories = ["RUSTSEC-2026-0001"]', out)
        self.assertIn('companion_of = "fresh@2.0.0"', out)
        self.assertNotIn('package = "old"', out)

    def test_suggested_entries_satisfy_the_checker(self):
        base = lock(("old", "1.0.0", []))
        head = lock(("old", "1.0.0", []), ("fresh", "2.0.0", ["dep 3.0.0"]), ("dep", "3.0.0", []))
        out = dac.suggest({"Cargo.lock": base}, {"Cargo.lock": head}, CONFIG, TODAY, registry(),
                          {"fresh": ["RUSTSEC-2026-0001"]}, "why", "owner", "2026-10-21")
        errors, _ = run(base, head, out)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
