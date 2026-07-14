import csv

MAP_PATH = "docs/prompts/git-history-cleanup/junk-commits-message-map.tsv"

_message_map = {}
with open(MAP_PATH, "r", encoding="utf-8", newline="") as f:
    for row in csv.reader(f, delimiter="|"):
        if len(row) != 3:
            continue
        old_hash, _old_message, new_message = row
        _message_map[old_hash.strip().encode("ascii")] = new_message.strip().encode(
            "utf-8"
        )


def commit_callback(commit, metadata):
    new_message = _message_map.get(commit.original_id)
    if new_message is not None:
        commit.message = new_message + b"\n"
