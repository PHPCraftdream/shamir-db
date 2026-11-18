/**
 * Per-operation canonical bytes for the schema-migration canonical format v1
 * — TypeScript mirror of
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/canonical/ddl.rs`
 * (+ `function.rs` for the create-function / create-validator extensions).
 *
 * Every operation begins with
 * `b"shamir-db schema migration op v1\0" || tag:u16be`; DTO fields follow in
 * the exact Rust order. Tags: table create 1 / drop 2 / rename 3; index
 * create 4 / drop 5 / rename 6; schema set 7 / add 8 / remove 9; function
 * create 10 / drop 11 / rename 12 / check 13 / alter 14; validator create 15
 * / drop 16 / rename 17 / bind 18 / unbind 19.
 *
 * Drop table/index/function/validator and create function/validator reuse the
 * existing per-operation HMAC canonical v2 bytes (`canonicalBatchOp`, db `""`);
 * create function/validator append the optional exact source/module bytes
 * after the v2 bytes (Rust `function.rs`). Repo fields with the serde
 * `default_repo` default (`"main"`) encode `"main"` when absent.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationOp,
} from '../../../../types/schema/migration-body.js';
import type { WriteOpKind } from '../../../../types/schema/ddl.js';
import { canonicalBatchOp } from '../../../../protocol/hmac.js';
import { Encoder } from './encoder.js';
import { MigrationError } from '../client/error.js';
import { encodeRule, encodeRules } from './rules.js';
import { encodeQueryValue } from './values.js';

const utf8 = new TextEncoder();
const PREFIX = utf8.encode('shamir-db schema migration op v1\0');

const start = (tag: number): Encoder => {
  const e = new Encoder();
  e.raw(PREFIX);
  e.u16(tag);
  return e;
};

const writeOpByte = (kind: WriteOpKind): number => {
  switch (kind) {
    case 'insert':
      return 0;
    case 'update':
      return 1;
    case 'upsert':
      return 2;
    case 'delete':
      return 3;
    default: {
      const exhaustive: never = kind;
      throw new TypeError(`unknown write op: ${String(exhaustive)}`);
    }
  }
};

/** `count || (count || str*)*` — a list of string paths (order preserved). */
function paths(e: Encoder, values: readonly string[][]): void {
  e.count(values.length);
  for (const path of values) e.strings(path);
}

/**
 * Canonical bytes of ONE migration operation (the payload of the
 * `u32`-length-prefixed body entry — the length prefix is added by the body
 * encoder in `./body.ts`).
 */
export function encodeMigrationOperation(op: SchemaMigrationOp): Uint8Array {
  try {
    switch (op.operation) {
      case 'create_table': {
        const { create_table, repo, if_not_exists, retention, schema } = op.payload;
        const e = start(1);
        e.str(create_table);
        e.str(repo);
        e.bool(if_not_exists === true);
        e.opt(retention, (r) => {
          e.opt(r.max_age_secs, (v) => e.u64(v));
          e.opt(r.max_count, (v) => e.u64(v));
          e.opt(r.min_count, (v) => e.u64(v));
        });
        e.opt(schema, (s) => encodeRules(e, s));
        return e.finish();
      }
      case 'drop_table':
        return canonicalBatchOp('', op.payload);
      case 'rename_table': {
        const { rename_table, to, repo } = op.payload;
        const e = start(3);
        e.str(rename_table);
        e.str(to);
        e.str(repo ?? 'main');
        return e.finish();
      }
      case 'create_index': {
        const {
          create_index,
          table,
          fields,
          unique,
          sorted,
          repo,
          index_type,
          fts_tokenizer,
          fts_language,
          functional_op,
          functional_args,
          vector_dim,
          vector_metric,
          vector_quantization,
          include,
          if_not_exists,
        } = op.payload;
        const e = start(4);
        e.str(create_index);
        e.str(table);
        paths(e, fields);
        e.bool(unique);
        e.bool(sorted);
        e.str(repo);
        e.opt(index_type, (v) => e.str(v));
        e.opt(fts_tokenizer, (v) => e.str(v));
        e.opt(fts_language, (v) => e.str(v));
        e.opt(functional_op, (v) => e.str(v));
        e.opt(functional_args, (args) => {
          e.count(args.length);
          for (const value of args) encodeQueryValue(e, value, 0);
        });
        e.opt(vector_dim, (v) => e.u32(v));
        e.opt(vector_metric, (v) => e.str(v));
        e.opt(vector_quantization, (v) => e.str(v));
        paths(e, include ?? []);
        e.bool(if_not_exists === true);
        return e.finish();
      }
      case 'drop_index':
        return canonicalBatchOp('', op.payload);
      case 'rename_index': {
        const { rename_index, to, table, repo, if_exists } = op.payload;
        const e = start(6);
        e.str(rename_index);
        e.str(to);
        e.str(table);
        e.str(repo ?? 'main');
        e.bool(if_exists === true);
        return e.finish();
      }
      case 'set_table_schema': {
        const { set_table_schema, repo, schema, expected_version } = op.payload;
        const e = start(7);
        e.str(set_table_schema);
        e.str(repo);
        encodeRules(e, schema);
        e.opt(expected_version, (v) => e.u64(v));
        return e.finish();
      }
      case 'add_schema_rule': {
        const { add_schema_rule, repo, rule } = op.payload;
        const e = start(8);
        e.str(add_schema_rule);
        e.str(repo);
        encodeRule(e, rule);
        return e.finish();
      }
      case 'remove_schema_rule': {
        const { remove_schema_rule, repo, path } = op.payload;
        const e = start(9);
        e.str(remove_schema_rule);
        e.str(repo);
        e.strings(path);
        return e.finish();
      }
      case 'create_function': {
        // HMAC canonical v2 bytes, then the exact source/module extension
        // (Rust `function.rs::encode_create_function`).
        const bytes = canonicalBatchOp('', op.payload);
        const ext = new Encoder();
        ext.opt(op.payload.source, (v) => ext.str(v));
        ext.opt(op.payload.wasm, (v) => ext.str(v));
        const tail = ext.finish();
        const out = new Uint8Array(bytes.length + tail.length);
        out.set(bytes, 0);
        out.set(tail, bytes.length);
        return out;
      }
      case 'drop_function':
        return canonicalBatchOp('', op.payload);
      case 'rename_function': {
        const { rename_function, to } = op.payload;
        const e = start(12);
        e.str(rename_function);
        e.str(to);
        return e.finish();
      }
      case 'check_function': {
        const spec = op.payload.check_function;
        const resolvedLang = spec.lang ?? (spec.wasm !== undefined ? 'wasm' : 'rust');
        const resolvedOptimize = spec.optimize ?? 'auto';
        const e = start(13);
        e.str(resolvedLang);
        e.opt(spec.wasm, (v) => e.str(v));
        e.opt(spec.source, (v) => e.str(v));
        e.str(resolvedOptimize);
        return e.finish();
      }
      case 'alter_function': {
        const { alter_function, optimize } = op.payload;
        const e = start(14);
        e.str(alter_function);
        e.str(optimize);
        return e.finish();
      }
      case 'create_validator': {
        // HMAC canonical v2 bytes, then the exact source/module extension
        // (Rust `function.rs::encode_create_validator`).
        const bytes = canonicalBatchOp('', op.payload);
        const ext = new Encoder();
        ext.opt(op.payload.source, (v) => ext.str(v));
        ext.opt(op.payload.wasm, (v) => ext.str(v));
        const tail = ext.finish();
        const out = new Uint8Array(bytes.length + tail.length);
        out.set(bytes, 0);
        out.set(tail, bytes.length);
        return out;
      }
      case 'drop_validator':
        return canonicalBatchOp('', op.payload);
      case 'rename_validator': {
        const { rename_validator, to } = op.payload;
        const e = start(17);
        e.str(rename_validator);
        e.str(to);
        return e.finish();
      }
      case 'bind_validator': {
        const { bind_validator, db, repo, table, ops, priority } = op.payload;
        const e = start(18);
        e.str(bind_validator);
        e.str(db);
        e.str(repo);
        e.str(table);
        e.count(ops.length);
        for (const kind of ops) e.u8(writeOpByte(kind));
        e.u16(priority);
        return e.finish();
      }
      case 'unbind_validator': {
        const { unbind_validator, db, repo, table } = op.payload;
        const e = start(19);
        e.str(unbind_validator);
        e.str(db);
        e.str(repo);
        e.str(table);
        return e.finish();
      }
    }
  } catch (error) {
    throw new MigrationError(
      'canonical',
      `schema migration operation encoding failed: ${(error as Error).message}`,
    );
  }
}
