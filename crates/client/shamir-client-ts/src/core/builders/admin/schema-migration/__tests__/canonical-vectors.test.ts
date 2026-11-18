/**
 * Cross-client canonical vector tests (release-1 R1c step 3b): every case in
 * `schema_migration_canonical_vectors.json` is rebuilt from the typed
 * migration builders (`schemaMigration` + the 19 `migrationDdl` wrappers)
 * using the language-neutral structured args from the fixture, then pinned
 * against the expected canonical bytes and SHA-256 checksum. No fixture
 * operation data is hardcoded here — only structured DTO args are read.
 */

import { readFileSync } from 'node:fs';

import { describe, it, expect } from 'vitest';

import { field } from '../../../ddl/field-builder.js';
import {
  MIGRATION_OPERATION_KINDS,
  migrationDdl,
  migrationFloat,
  schemaMigration,
} from '../model/index.js';
import { ApplicationMigrationSet } from '../index.js';
import { canonicalBody, checksum } from '../canonical/body.js';
import { compareUtf8 } from '../canonical/encoder.js';
import { decode, encode } from '../../../../protocol/framing.js';
import type {
  FunctionLang,
  FunctionOptimize,
  WriteOpKind,
} from '../../../../types/schema/ddl.js';
import type { MigrationFieldRuleDto, SchemaMigrationBody } from '../../../../types/schema/migration-body.js';

interface FixtureOp {
  family: string;
  variant: string;
  args: Record<string, unknown>;
}
interface FixtureDep {
  function: string;
  depends_on: string[];
}
interface FixtureMigration {
  id: string;
  ordinal: number;
  display_name: string;
  function_dependencies: FixtureDep[];
  operations: FixtureOp[];
  expected_canonical_bytes_hex: string;
  expected_checksum_hex: string;
}
interface FixtureCase {
  name: string;
  migrations: FixtureMigration[];
}
interface Fixture {
  version: number;
  domain_separator_hex: string;
  cases: FixtureCase[];
}

const fixture: Fixture = JSON.parse(
  readFileSync(
    new URL(
      '../../../../../../../../query/shamir-query-builder/tests/fixtures/schema_migration_canonical_vectors.json',
      import.meta.url,
    ),
    'utf8',
  ),
) as Fixture;

/** Read one structured arg from a fixture op (typed, never raw wire). */
function arg<T>(args: Record<string, unknown>, key: string): T {
  const v = args[key];
  if (v === undefined) {
    throw new Error(`fixture op missing required arg "${key}"`);
  }
  return v as T;
}

function opt<T>(args: Record<string, unknown>, key: string): T | undefined {
  return args[key] as T | undefined;
}

/** Dispatch one fixture op to its exact `migrationDdl` wrapper. */
function buildOp(op: FixtureOp): Parameters<typeof schemaMigration>[3][number] {
  const a = op.args;
  switch (op.variant) {
    case 'create_table':
      return migrationDdl.createTable(arg<string>(a, 'create_table'), {
        repo: opt<string>(a, 'repo'),
        if_not_exists: opt<boolean>(a, 'if_not_exists'),
        retention: opt<{ max_age_secs: number; min_count: number }>(a, 'retention'),
        schema: opt<MigrationFieldRuleDto[]>(a, 'schema'),
      });
    case 'drop_table':
      return migrationDdl.dropTable(arg<string>(a, 'drop_table'), {
        repo: opt<string>(a, 'repo'),
        if_exists: opt<boolean>(a, 'if_exists'),
        cascade: opt<boolean>(a, 'cascade'),
      });
    case 'rename_table':
      return migrationDdl.renameTable(
        arg<string>(a, 'rename_table'),
        arg<string>(a, 'to'),
        { repo: opt<string>(a, 'repo') },
      );
    case 'create_index':
      return migrationDdl.createIndex(
        arg<string>(a, 'create_index'),
        arg<string>(a, 'table'),
        arg<string[][]>(a, 'fields'),
        {
          repo: opt<string>(a, 'repo'),
          index_type: opt<string>(a, 'index_type') as 'fts' | undefined,
          fts_tokenizer: opt<string>(a, 'fts_tokenizer'),
        },
      );
    case 'drop_index':
      return migrationDdl.dropIndex(
        arg<string>(a, 'table'),
        arg<string>(a, 'drop_index'),
        { repo: opt<string>(a, 'repo'), if_exists: opt<boolean>(a, 'if_exists') },
      );
    case 'rename_index':
      return migrationDdl.renameIndex(
        arg<string>(a, 'table'),
        arg<string>(a, 'rename_index'),
        arg<string>(a, 'to'),
        { repo: opt<string>(a, 'repo'), if_exists: opt<boolean>(a, 'if_exists') },
      );
    case 'set_table_schema': {
      // `schema` omitted means the empty rule list.
      const schema = opt<MigrationFieldRuleDto[]>(a, 'schema') ?? [];
      return migrationDdl.setTableSchema(arg<string>(a, 'set_table_schema'), schema, {
        repo: opt<string>(a, 'repo'),
        expectedVersion: opt<number>(a, 'expected_version'),
      });
    }
    case 'add_schema_rule':
      return migrationDdl.addSchemaRule(
        arg<string>(a, 'add_schema_rule'),
        migrationDdl.withBounds(
          field(arg<string[]>(a, 'rule_path')).typeTag(arg<string>(a, 'rule_type')).build(),
          {
            min: migrationFloat(arg<number>(a, 'rule_min')),
            max: migrationFloat(arg<number>(a, 'rule_max')),
          },
        ),
        { repo: opt<string>(a, 'repo') },
      );
    case 'remove_schema_rule':
      return migrationDdl.removeSchemaRule(
        arg<string>(a, 'remove_schema_rule'),
        arg<string[]>(a, 'path'),
        { repo: opt<string>(a, 'repo') },
      );
    case 'create_function':
      return migrationDdl.createFunction(arg<string>(a, 'create_function'), {
        source: opt<string>(a, 'source'),
        wasm: opt<string>(a, 'wasm'),
        lang: opt<FunctionLang>(a, 'lang'),
        optimize: opt<FunctionOptimize>(a, 'optimize'),
        replace: opt<boolean>(a, 'replace'),
        visibility: opt<string>(a, 'visibility'),
      });
    case 'drop_function':
      return migrationDdl.dropFunction(arg<string>(a, 'drop_function'), {
        if_exists: opt<boolean>(a, 'if_exists'),
      });
    case 'rename_function':
      return migrationDdl.renameFunction(
        arg<string>(a, 'rename_function'),
        arg<string>(a, 'to'),
      );
    case 'check_function': {
      // Exactly one of source/wasm is passed through verbatim — no defaulting
      // (`checkFunction` itself throws when neither or both are present).
      return migrationDdl.checkFunction({
        source: opt<string>(a, 'check_source'),
        wasm: opt<string>(a, 'check_wasm'),
      });
    }
    case 'alter_function':
      return migrationDdl.alterFunction(
        arg<string>(a, 'alter_function'),
        arg<FunctionOptimize>(a, 'optimize'),
      );
    case 'create_validator':
      return migrationDdl.createValidator(arg<string>(a, 'create_validator'), {
        source: opt<string>(a, 'source'),
        wasm: opt<string>(a, 'wasm'),
        replace: opt<boolean>(a, 'replace'),
      });
    case 'drop_validator':
      return migrationDdl.dropValidator(arg<string>(a, 'drop_validator'), {
        if_exists: opt<boolean>(a, 'if_exists'),
      });
    case 'rename_validator':
      return migrationDdl.renameValidator(
        arg<string>(a, 'rename_validator'),
        arg<string>(a, 'to'),
      );
    case 'bind_validator':
      return migrationDdl.bindValidator(
        arg<string>(a, 'bind_validator'),
        arg<string>(a, 'table'),
        arg<WriteOpKind[]>(a, 'ops'),
        // Binding priority defaults to 1500 when the fixture omits it.
        opt<number>(a, 'priority') ?? 1500,
        { db: arg<string>(a, 'db'), repo: opt<string>(a, 'repo') },
      );
    case 'unbind_validator':
      return migrationDdl.unbindValidator(arg<string>(a, 'unbind_validator'), {
        db: arg<string>(a, 'db'),
        table: arg<string>(a, 'table'),
        repo: opt<string>(a, 'repo'),
      });
    default:
      throw new Error(`unhandled fixture op variant: ${op.variant}`);
  }
}

/** Rebuild the typed bodies of one fixture case from its structured args. */
function buildBodies(testCase: FixtureCase): SchemaMigrationBody[] {
  return testCase.migrations.map((mig) =>
    schemaMigration(
      mig.id,
      BigInt(mig.ordinal),
      mig.display_name,
      mig.operations.map(buildOp),
      mig.function_dependencies,
    ),
  );
}

describe('migrationCanonicalVectorsMatchRust', () => {
  it('every fixture case instantiates an ordered ApplicationMigrationSet whose entries match the fixture', () => {
    for (const testCase of fixture.cases) {
      const set = new ApplicationMigrationSet(buildBodies(testCase));
      expect(set.length).toBe(testCase.migrations.length);
      const entries = set.entries;
      for (let i = 0; i < testCase.migrations.length; i++) {
        const mig = testCase.migrations[i]!;
        const entry = entries[i]!;
        expect(entry.ordinal).toBe(BigInt(mig.ordinal));
        expect(entry.body.metadata.id).toBe(mig.id);
        expect(Buffer.from(entry.checksum).toString('hex')).toBe(mig.expected_checksum_hex);
        expect(Buffer.from(canonicalBody(entry.body)).toString('hex')).toBe(
          mig.expected_canonical_bytes_hex,
        );
      }
    }
  });

  it('the two-body ordering case pins the set order by ordinal and id', () => {
    const orderingCases = fixture.cases.filter((c) => c.migrations.length >= 2);
    expect(orderingCases.length).toBeGreaterThan(0);
    for (const testCase of orderingCases) {
      const set = new ApplicationMigrationSet(buildBodies(testCase));
      const entries = set.entries;
      expect(entries.map((e) => e.body.metadata.id)).toEqual(
        testCase.migrations.map((m) => m.id),
      );
      for (const mig of testCase.migrations) {
        expect(set.atOrdinal(BigInt(mig.ordinal))?.body.metadata.id).toBe(mig.id);
      }
    }
  });

  it('fixture version is 1 and the cases cover canonical version 1 with all 19 operation kinds', () => {
    expect(fixture.version).toBe(1);
    const kinds = new Set<string>();
    for (const testCase of fixture.cases) {
      for (const body of buildBodies(testCase)) {
        expect(body.metadata.canonical_version).toBe(1);
        for (const op of body.operations) kinds.add(op.operation);
      }
    }
    expect(kinds.size).toBe(MIGRATION_OPERATION_KINDS.length);
    for (const kind of MIGRATION_OPERATION_KINDS) {
      expect(kinds.has(kind)).toBe(true);
    }
  });

  it('canonical dependency sorting orders astral characters after high BMP (UTF-8 bytes, not JS string order)', () => {
    // JS string comparison puts '\u{10000}' (surrogate pair, first code unit
    // 0xD800) BEFORE '\uFFFF' (0xD800 < 0xFFFF); UTF-8 byte order
    // (EF BF BF < F0 90 80 80) is the opposite — the canonical encoder must
    // sort by bytes, so astral sorts AFTER the high BMP character.
    expect('\u{10000}' < '\uFFFF').toBe(true);
    expect(compareUtf8('\u{10000}', '\uFFFF')).toBe(1);
    expect(compareUtf8('\uFFFF', '\u{10000}')).toBe(-1);
    expect(compareUtf8('\u{10000}', '\u{10000}')).toBe(0);
    const names = ['\u{10000}', '\uFFFF', '\u00E9', '\u0065\u0301'];
    const sorted = [...names].sort(compareUtf8);
    expect(sorted).toEqual(['\u0065\u0301', '\u00E9', '\uFFFF', '\u{10000}']);
  });

  it('wire f64 bound markers are raw msgpack float64 via production encode/decode (negative zero preserved)', () => {
    expect(Buffer.from(encode(migrationFloat(-0))).toString('hex')).toBe(
      'cb8000000000000000',
    );
    expect(Buffer.from(encode(migrationFloat(2))).toString('hex')).toBe(
      'cb4000000000000000',
    );
    // Integer bound 2 is DIFFERENT msgpack bytes from float 2.0.
    expect(Buffer.from(encode(2)).toString('hex')).toBe('02');
    const result = decode(encode([migrationFloat(-0)])) as number[];
    expect(Object.is(result[0], -0)).toBe(true);
  });
});

for (const testCase of fixture.cases) {
  describe(`canonical vectors: ${testCase.name}`, () => {
    for (const mig of testCase.migrations) {
      it(`migration ${mig.id} reproduces the canonical bytes and checksum`, () => {
        const body = schemaMigration(
          mig.id,
          BigInt(mig.ordinal),
          mig.display_name,
          mig.operations.map(buildOp),
          mig.function_dependencies,
        );
        expect(Buffer.from(canonicalBody(body)).toString('hex')).toBe(
          mig.expected_canonical_bytes_hex,
        );
        expect(Buffer.from(checksum(body)).toString('hex')).toBe(
          mig.expected_checksum_hex,
        );
      });
    }
  });
}
