#!/usr/bin/env python3
"""Independent stdlib reference encoder for schema-migration v1 golden data.

This script constructs fixture bytes from the neutral operation descriptors; it
is fixture tooling only and deliberately does not invoke production Rust or run tests.
"""
import hashlib
import json
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent
FIXTURE = ROOT / "schema_migration_v1.json"
OP_PREFIX = b"shamir-db schema migration op v1\0"
HMAC_PREFIX = b"shamir-db hmac canonical v2\0"
DOMAIN = b"shamir-db schema migration\0"


def u8(value): return bytes((value,))
def u16(value): return struct.pack(">H", value)
def u32(value): return struct.pack(">I", value)
def u64(value): return struct.pack(">Q", value)
def i64(value): return struct.pack(">q", value)
def string(value):
    raw = value.encode("utf-8")
    return u32(len(raw)) + raw
def strings(values): return u32(len(values)) + b"".join(string(v) for v in values)
def optional(value, encoder): return b"\0" if value is None else b"\1" + encoder(value)
def paths(values): return u32(len(values)) + b"".join(strings(path) for path in values)


def migration_op(op):
    name, f = op["variant"], op["fields"]
    if name == "create_table":
        payload = string(f["create_table"]) + string(f["repo"]) + u8(f["if_not_exists"])
        retention = f["retention"]
        payload += b"\0" if retention is None else b"\1" + optional(retention["max_age_secs"], u64) + optional(retention["max_count"], u64) + optional(retention["min_count"], u64)
        schema = f["schema"]
        payload += b"\0" if schema is None else b"\1" + u32(len(schema)) + b"".join(rule(x) for x in schema)
        return OP_PREFIX + u16(1) + payload
    if name == "drop_table":
        payload = string("") + string(f["repo"]) + string(f["drop_table"]) + u8(f["cascade"]) + u8(f["if_exists"])
        return HMAC_PREFIX + u16(3) + payload
    if name == "rename_table":
        return OP_PREFIX + u16(3) + string(f["rename_table"]) + string(f["to"]) + string(f["repo"])
    if name == "create_index":
        payload = string(f["create_index"]) + string(f["table"]) + paths(f["fields"])
        payload += u8(f["unique"]) + u8(f["sorted"]) + string(f["repo"])
        for key in ("index_type", "fts_tokenizer", "fts_language", "functional_op"):
            payload += optional(f[key], string)
        payload += optional(f["functional_args"], lambda xs: u32(len(xs)) + b"".join(query_value(x) for x in xs))
        payload += optional(f["vector_dim"], u32) + optional(f["vector_metric"], string)
        payload += optional(f["vector_quantization"], string) + paths(f["include"]) + u8(f["if_not_exists"])
        return OP_PREFIX + u16(4) + payload
    if name == "drop_index":
        payload = string("") + string(f["repo"]) + string(f["table"]) + string(f["drop_index"]) + u8(f["if_exists"])
        return HMAC_PREFIX + u16(4) + payload
    if name == "rename_index":
        payload = string(f["rename_index"]) + string(f["to"]) + string(f["table"]) + string(f["repo"]) + u8(f["if_exists"])
        return OP_PREFIX + u16(6) + payload
    if name == "set_table_schema":
        schema = f["schema"]
        return OP_PREFIX + u16(7) + string(f["set_table_schema"]) + string(f["repo"]) + u32(len(schema)) + b"".join(rule(x) for x in schema) + optional(f["expected_version"], u64)
    if name == "add_schema_rule":
        return OP_PREFIX + u16(8) + string(f["add_schema_rule"]) + string(f["repo"]) + rule(f["rule"])
    if name == "remove_schema_rule":
        return OP_PREFIX + u16(9) + string(f["remove_schema_rule"]) + string(f["repo"]) + strings(f["path"])
    if name == "create_function":
        source, wasm = f["source"], f["wasm"]
        payload = string(f["create_function"]) + u8(f["replace"]) + string(f["security"] or "invoker")
        payload += optional(f["visibility"], string) + strings(f["secret_grants"]) + strings(f["net_grants"])
        payload += optional(source, lambda s: hashlib.sha256(s.encode()).digest())
        payload += optional(wasm, lambda s: hashlib.sha256(s.encode()).digest())
        payload += string(f["lang"] or ("wasm" if wasm is not None else "rust")) + string(f["optimize"] or "auto")
        payload += optional(source, string) + optional(wasm, string)
        return HMAC_PREFIX + u16(0x16) + payload
    if name == "drop_function":
        return HMAC_PREFIX + u16(0x1A) + string(f["drop_function"]) + u8(f["if_exists"])
    if name == "rename_function":
        return OP_PREFIX + u16(12) + string(f["rename_function"]) + string(f["to"])
    if name == "check_function":
        spec = f["check_function"]
        return OP_PREFIX + u16(13) + string(spec["lang"] or ("wasm" if spec["wasm"] is not None else "rust")) + optional(spec["wasm"], string) + optional(spec["source"], string) + string(spec["optimize"] or "auto")
    if name == "alter_function":
        return OP_PREFIX + u16(14) + string(f["alter_function"]) + string(f["optimize"])
    if name == "create_validator":
        source, wasm = f["source"], f["wasm"]
        payload = string(f["create_validator"]) + u8(f["replace"])
        payload += optional(source, lambda s: hashlib.sha256(s.encode()).digest())
        payload += optional(wasm, lambda s: hashlib.sha256(s.encode()).digest())
        payload += optional(source, string) + optional(wasm, string)
        return HMAC_PREFIX + u16(0x1C) + payload
    if name == "drop_validator":
        return HMAC_PREFIX + u16(0x1B) + string(f["drop_validator"]) + u8(f["if_exists"])
    if name == "rename_validator":
        return OP_PREFIX + u16(17) + string(f["rename_validator"]) + string(f["to"])
    if name == "bind_validator":
        tags = {"insert": 0, "update": 1, "upsert": 2, "delete": 3}
        payload = string(f["bind_validator"]) + string(f["db"]) + string(f["repo"]) + string(f["table"])
        payload += u32(len(f["ops"])) + b"".join(u8(tags[x]) for x in f["ops"]) + u16(f["priority"])
        return OP_PREFIX + u16(18) + payload
    if name == "unbind_validator":
        return OP_PREFIX + u16(19) + string(f["unbind_validator"]) + string(f["db"]) + string(f["repo"]) + string(f["table"])
    raise ValueError(name)


def rule(value):
    payload = strings(value["path"]) + string(value["type"])
    for key in ("required", "nullable", "unsigned"):
        payload += b"\0" if value.get(key) is None else b"\1" + u8(value[key])
    for key in ("min", "max"):
        n = value.get(key)
        if n is None:
            payload += b"\0"
        elif isinstance(n, int):
            payload += b"\1\0" + i64(n)
        else:
            payload += b"\1\1" + struct.pack(">d", n)
    for key in ("len", "max_len", "min_len"):
        payload += optional(value.get(key), u64)
    payload += b"\0"  # one_of absent
    payload += (b"\1\0" if "default" in value else b"\0")  # present null encodes FilterValue::Null
    payload += optional(value.get("array_of"), string)
    payload += optional(value.get("format"), string)
    payload += b"\0"  # foreign_key absent
    payload += b"\0"  # unique absent
    payload += u8(value.get("auto_now", False)) + u8(value.get("auto_now_add", False))
    return payload


def query_value(value):
    kind, payload = next(iter(value.items()))
    if kind == "null": return b"\0"
    if kind == "bool": return b"\1" + u8(payload)
    if kind == "int": return b"\2" + i64(payload)
    if kind == "f64": return b"\3" + struct.pack(">d", payload)
    if kind == "str": return b"\6" + string(payload)
    if kind == "bin": return b"\7" + bytes.fromhex(payload)
    if kind == "list": return b"\10" + u32(len(payload)) + b"".join(query_value(x) for x in payload)
    raise ValueError(kind)


def main():
    data = json.loads(FIXTURE.read_text(encoding="utf-8"))
    body = data["body"]
    encoded = string(body["id"]) + u64(body["ordinal"]) + string(body["display_name"])
    dependencies = sorted(body["dependencies"], key=lambda x: x["function"].encode())
    encoded += u32(len(dependencies))
    for dep in dependencies:
        encoded += string(dep["function"])
        names = sorted(dep["depends_on"], key=lambda x: x.encode())
        encoded += strings(names)
    operations = [migration_op(op) for op in body["operations"]]
    encoded += u32(len(operations)) + b"".join(u32(len(op)) + op for op in operations)
    checksum = hashlib.sha256(DOMAIN + u16(data["version"]) + encoded).hexdigest()
    data["vectors"] = [{"name": "every-admitted-operation", "expected_canonical_bytes_hex": encoded.hex(), "expected_checksum_hex": checksum}]
    FIXTURE.write_text(json.dumps(data, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
