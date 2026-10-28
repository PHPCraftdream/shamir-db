#!/usr/bin/env python3
"""Independent stdlib-only generator for admin HMAC v2 known-answer vectors.

Reads fixture input fields and prints independently encoded canonical bytes and
HMAC tags; it does not modify the fixture file.
"""
import hashlib
import hmac
import json
import pathlib
import struct
import sys

PREFIX = b"shamir-db hmac canonical v2\0"
KEY_PREFIX = b"shamir-db hmac key v1\0"


def u16(n): return struct.pack(">H", n)
def u32(n): return struct.pack(">I", n)
def u64(n): return struct.pack(">Q", int(n))
def string(value):
    raw = value.encode("utf-8")
    return u32(len(raw)) + raw


def optional(value, encode):
    return b"\0" if value is None else b"\1" + encode(value)


def strings(values):
    return u32(len(values)) + b"".join(string(value) for value in values)


def resource(kind, values):
    tags = {"database": 0, "store": 1, "table": 2, "function": 3, "function_folder": 4, "function_namespace": 5}
    return bytes([tags[kind]]) + (strings(values) if kind == "function_folder" else b"" if kind == "function_namespace" else b"".join(string(v) for v in values))


def group(kind, value):
    return b"\0" + string(value) if kind == "name" else b"\1" + u64(value)


def fields(vector):
    """Encode named schema fields; values are supplied in vector['fields']."""
    f = vector["fields"]
    op = vector["op"]
    s = lambda key: string(f[key])
    b = lambda key: bytes([int(f[key])])
    opt_field = lambda key, encode: optional(f[key] if key in f else None, encode)
    schema = {
        "drop_db": lambda: s("name") + b("cascade") + b("if_exists"),
        "drop_repo": lambda: s("db") + s("name") + b("cascade") + b("if_exists"),
        "drop_table": lambda: s("db") + s("repo") + s("name") + b("cascade") + b("if_exists"),
        "drop_index": lambda: s("db") + s("repo") + s("table") + s("name") + b("if_exists"),
        "drop_user": lambda: s("name") + b("if_exists"),
        "start_migration": lambda: s("db") + s("repo") + s("name") + s("dst_repo") + s("dst_engine") + optional(f["dst_path"], string),
        "commit_migration": lambda: s("db") + s("name"), "rollback_migration": lambda: s("db") + s("name"),
        "grant_role": lambda: s("role") + s("user"), "revoke_role": lambda: s("role") + s("user"),
        "chmod": lambda: resource(f["resource_kind"], f["resource"]) + u16(f["mode"]),
        "chown": lambda: resource(f["resource_kind"], f["resource"]) + u64(f["owner"]),
        "chgrp": lambda: resource(f["resource_kind"], f["resource"]) + optional(f["group_id"], u64),
        "create_user": lambda: s("name") + strings(f["roles"]) + optional(f["database"], string),
        "set_retention": lambda: s("db") + s("repo") + s("name") + b"".join(optional(f[k], u64) for k in ("max_age_secs", "max_count", "min_count")),
        "purge_history": lambda: s("db") + s("repo") + s("name") + (b"\0" + u64(f["timestamp"]) if f["scope"] == "older_than" else b"\1" + u64(f["age_secs"])),
        "create_group": lambda: s("name"), "drop_group": lambda: group(f["group_kind"], f["group"]) + b("if_exists"),
        "rename_group": lambda: group(f["group_kind"], f["group"]) + s("to"),
        "add_group_member": lambda: group(f["group_kind"], f["group"]) + u64(f["user"]),
        "remove_group_member": lambda: group(f["group_kind"], f["group"]) + u64(f["user"]),
        "create_function": lambda: s("name") + b("replace") + string("invoker" if "security" not in f else f["security"]) + optional(f["visibility"], string) + strings(f["secret_grants"]) + strings(f["net_grants"]) + optional(f["source"], lambda x: hashlib.sha256(x.encode()).digest()) + optional(f["wasm"], lambda x: hashlib.sha256(x.encode()).digest()),
        "set_superuser": lambda: s("user") + b("on"), "set_replicator": lambda: s("user") + b("on"),
        "create_scram_user": lambda: s("name") + strings(f["roles"]),
        "drop_function": lambda: s("name") + b("if_exists"), "drop_validator": lambda: s("name") + b("if_exists"),
        "create_validator": lambda: s("name") + b("replace") + optional(f["source"], lambda x: hashlib.sha256(x.encode()).digest()) + optional(f["wasm"], lambda x: hashlib.sha256(x.encode()).digest()),
        "drop_replication_profile": lambda: s("name"), "drop_publication": lambda: s("name"), "drop_subscription": lambda: s("name"),
    }
    return schema[op]()


def main():
    fixture = pathlib.Path(__file__).resolve().parent / "admin_hmac_v2.json"
    data = json.loads(fixture.read_text(encoding="utf-8"))
    session_id = bytes.fromhex(data["session_id_hex"])
    key = hashlib.sha256(KEY_PREFIX + session_id).digest()
    for vector in data["vectors"]:
        encoded = fields(vector)
        canonical = PREFIX + u16(int(vector["op_tag"], 16)) + encoded
        vector["fields_hex"] = encoded.hex()
        vector["canonical_hex"] = canonical.hex()
        vector["tag_hex"] = hmac.new(key, canonical, hashlib.sha256).hexdigest()
    print(json.dumps(data, indent=2, ensure_ascii=True))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
