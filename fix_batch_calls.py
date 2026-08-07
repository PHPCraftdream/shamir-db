#!/usr/bin/env python3
import re
import sys

def fix_file(filepath):
    with open(filepath, 'r', encoding='utf-8') as f:
        content = f.read()

    original = content

    # Fix .upsert("alias", upsert(...)) -> .try_upsert("alias", upsert(...)).unwrap()
    # This handles multiline calls
    content = re.sub(
        r'(\w+)\.upsert\s*\(\s*"([^"]+)",\s*upsert\s*\([^;]*?\)\s*\);',
        lambda m: f'{m.group(1)}.try_upsert("{m.group(2)}", upsert({m.group(0).split("upsert(")[1].rstrip(");")})).unwrap();',
        content,
        flags=re.DOTALL
    )

    # Fix .update("alias", update(...)) -> .try_update("alias", update(...)).unwrap()
    content = re.sub(
        r'(\w+)\.update\s*\(\s*"([^"]+)",\s*update\s*\([^;]*?\)\s*\);',
        lambda m: f'{m.group(1)}.try_update("{m.group(2)}", update({m.group(0).split("update(")[1].rstrip(");")})).unwrap();',
        content,
        flags=re.DOTALL
    )

    # Fix .delete("alias", delete(...)) -> .try_delete("alias", delete(...)).unwrap()
    content = re.sub(
        r'(\w+)\.delete\s*\(\s*"([^"]+)",\s*delete\s*\([^;]*?\)\s*\);',
        lambda m: f'{m.group(1)}.try_delete("{m.group(2)}", delete({m.group(0).split("delete(")[1].rstrip(");")})).unwrap();',
        content,
        flags=re.DOTALL
    )

    if content != original:
        with open(filepath, 'w', encoding='utf-8') as f:
            f.write(content)
        return True
    return False

if __name__ == "__main__":
    for filepath in sys.argv[1:]:
        if fix_file(filepath):
            print(f"Fixed: {filepath}")
        else:
            print(f"No changes: {filepath}")