#!/usr/bin/env python3
import re
import os

def fix_engine_tests():
    # Fix files in shamir-engine/src/query/batch/tests/
    test_dir = "crates/shamir-engine/src/query/batch/tests"

    # Get all test files
    for filename in os.listdir(test_dir):
        if not filename.endswith(".rs"):
            continue

        filepath = os.path.join(test_dir, filename)
        with open(filepath, 'r', encoding='utf-8') as f:
            content = f.read()

        original = content

        # Fix .update("alias", write::update(...)) -> .try_update("alias", write::update(...)).unwrap()
        content = re.sub(
            r'(\w+)\.update\s*\(\s*"([^"]+)",\s*write::update\s*\([^;]*?\)\s*\);',
            lambda m: f'{m.group(1)}.try_update("{m.group(2)}", write::update({m.group(0).split("write::update(")[1].rstrip(");")})).unwrap();',
            content,
            flags=re.DOTALL
        )

        # Fix .delete("alias", write::delete(...)) -> .try_delete("alias", write::delete(...)).unwrap()
        content = re.sub(
            r'(\w+)\.delete\s*\(\s*"([^"]+)",\s*write::delete\s*\([^;]*?\)\s*\);',
            lambda m: f'{m.group(1)}.try_delete("{m.group(2)}", write::delete({m.group(0).split("write::delete(")[1].rstrip(");")})).unwrap();',
            content,
            flags=re.DOTALL
        )

        # Fix .upsert("alias", write::upsert(...)) -> .try_upsert("alias", write::upsert(...)).unwrap()
        content = re.sub(
            r'(\w+)\.upsert\s*\(\s*"([^"]+)",\s*write::upsert\s*\([^;]*?\)\s*\);',
            lambda m: f'{m.group(1)}.try_upsert("{m.group(2)}", write::upsert({m.group(0).split("write::upsert(")[1].rstrip(");")})).unwrap();',
            content,
            flags=re.DOTALL
        )

        if content != original:
            with open(filepath, 'w', encoding='utf-8') as f:
                f.write(content)
            print(f"Fixed: {filepath}")

if __name__ == "__main__":
    fix_engine_tests()