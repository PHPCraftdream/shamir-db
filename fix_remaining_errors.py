#!/usr/bin/env python3
import re

# Fix files that have the wrong pattern from my earlier replacement
fixes = {
    "crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs": [
        (r'\.try_delete\(([^)]+)\)\)\);', r'.try_delete(\1).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs": [
        (r'\.try_update\(([^)]+)\)\)\);', r'.try_update(\1).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs": [
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/fk_reverse_cache_tests.rs": [
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs": [
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/executor_tests/basic_ops_tests.rs": [
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/executor_tests/interactive_tx_tests.rs": [
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/executor_tests/permissions_tests.rs": [
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
    ],
    "crates/shamir-engine/src/query/batch/tests/planner_tests.rs": [
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.upsert\("([^"]+)", write::upsert\(([^)]+)\)\);', r'.try_upsert("\1", write::upsert(\2)).unwrap();'),
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
        (r'\.upsert\("([^"]+)", write::upsert\(([^)]+)\)\);', r'.try_upsert("\1", write::upsert(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.update\("([^"]+)", write::update\(([^)]+)\)\);', r'.try_update("\1", write::update(\2)).unwrap();'),
        (r'\.upsert\("([^"]+)", write::upsert\(([^)]+)\)\);', r'.try_upsert("\1", write::upsert(\2)).unwrap();'),
        (r'\.delete\("([^"]+)", write::delete\(([^)]+)\)\);', r'.try_delete("\1", write::delete(\2)).unwrap();'),
    ],
    "crates/shamir-db/tests/cas_sequenced_e2e.rs": [
        (r'\.update\("([^"]+)", update\(([^)]+)\)\);', r'.try_update("\1", update(\2)).unwrap();'),
    ],
    "crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs": [
        (r'\.update\("([^"]+)", shamir_query_builder::write::update\(([^)]+)\)\);', r'.try_update("\1", shamir_query_builder::write::update(\2)).unwrap();'),
    ],
    "crates/shamir-db/tests/declarative_schema_fk_ondelete_e2e.rs": [
        (r'\.delete\("([^"]+)", delete\(([^)]+)\)\);', r'.try_delete("\1", delete(\2)).unwrap();'),
    ],
    "crates/shamir-db/tests/purge_history.rs": [
        (r'\.update\("([^"]+)", update\(([^)]+)\)\);', r'.try_update("\1", update(\2)).unwrap();'),
    ],
    "crates/shamir-db/tests/declarative_schema_unique_e2e.rs": [
        (r'\.update\("([^"]+)", update\(([^)]+)\)\);', r'.try_update("\1", update(\2)).unwrap();'),
    ],
}

for filepath, patterns in fixes.items():
    with open(filepath, 'r', encoding='utf-8') as f:
        content = f.read()

    original = content
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content, flags=re.DOTALL)

    if content != original:
        with open(filepath, 'w', encoding='utf-8') as f:
            f.write(content)
        print(f"Fixed: {filepath}")

print("Done!")