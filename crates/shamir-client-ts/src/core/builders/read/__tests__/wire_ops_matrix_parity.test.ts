import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Query } from '../query.js';
import * as filter from '../filter.js';
import { insert, update, upsert, del } from '../../write/write.js';
import { Batch } from '../../write/batch.js';
import { call } from '../../write/call.js';
import { subscribe } from '../../stream/subscribe.js';
import {
  createTable,
  renameTable,
  getTableSchema,
  removeSchemaRule,
  addSchemaRule,
  setTableSchema,
} from '../../ddl/ddl-schema.js';
import { field } from '../../ddl/field-builder.js';
import { dropIndex } from '../../ddl/ddl-index.js';
import {
  createValidator,
  bindValidator,
  unbindValidator,
  dropValidator,
  renameValidator,
  listValidators,
} from '../../ddl/ddl-func.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const fixturePath = resolve(
  __dirname,
  '../../../../../..',
  'shamir-query-builder',
  'tests',
  'fixtures',
  'wire_ops_matrix.json',
);

type Case = {
  name: string;
  builder: string;
  wire: unknown;
  known_gap?: string;
};
const cases = JSON.parse(readFileSync(fixturePath, 'utf8')) as Case[];
const signer = { hmacTagHex: (_canonical: Uint8Array) => 'aa' };

function build(name: string): unknown {
  switch (name) {
    case 'read_complex':
      return Query.from('users')
        .select(['name'])
        .where(
          filter.and(
            filter.eq('active', true),
            filter.or([
              filter.in_('role', ['admin', 'user']),
              filter.not(filter.like('name', 'x%')),
              filter.regex('name', '^A'),
            ]),
          ),
        )
        .groupBy('name')
        .having(filter.eq('active', true))
        .orderByDesc('name')
        .limit(10)
        .offset(2)
        .build();
    case 'read_page':
      return Query.from('users').select(['name']).page(2, 25).build();
    case 'read_default_ts':
      return Query.from('users').build();
    case 'insert':
      return insert('users', { name: 'Ada' });
    case 'update':
      return update('users')
        .set({ name: 'Ada' })
        .where(filter.eq('id', 1))
        .expectedVersion(7)
        .build();
    case 'delete':
      return del('users', filter.eq('id', 1));
    case 'upsert':
      return upsert('users', { id: 1 }, { name: 'Ada' });
    case 'create_table':
      return createTable('users', { if_not_exists: true });
    case 'drop_index':
      return dropIndex(signer, 'db', 'main', 'users', 'idx', {
        if_exists: true,
      });
    case 'rename_table':
      return renameTable('users', 'people', { repo: 'main' });
    case 'get_schema':
      return getTableSchema('users');
    case 'remove_schema_rule':
      return removeSchemaRule('users', ['name']);
    case 'add_schema_rule':
      return addSchemaRule(
        'users',
        field(['email']).string().required().maxLen(255).build(),
      );
    case 'set_schema':
      return setTableSchema(
        'users',
        [field(['email']).string().required().build()],
        { expectedVersion: 3 },
      );
    case 'create_validator':
      return createValidator('validate', { source: 'source' });
    case 'bind_validator':
      return bindValidator('validate', 'users', ['insert', 'update'], 1600, {
        db: 'app',
      });
    case 'unbind_validator':
      return unbindValidator('validate', { db: 'app', table: 'users' });
    case 'drop_validator':
      return dropValidator('validate', { hmac: 'aa', if_exists: true });
    case 'rename_validator':
      return renameValidator('validate', 'validate2');
    case 'list_validators':
      return listValidators('users', { db: 'app' });
    case 'call':
      return call('proc', [1]);
    case 'subscribe':
      return subscribe(
        { store: 'main', table: 'users', on: ['put'], deliver: 'keys' },
        { initial: true, fromVersion: 7 },
      );
    case 'subscribe_default_ts':
      return subscribe({ store: 'main', table: 'users' });
    case 'batch_ts':
      return Batch.create(1)
        .add('first', insert('users', { name: 'Ada' }))
        .add('second', call('proc', [1]), {
          returnResult: false,
          after: ['first'],
          when: filter.valueEq(1, 1),
        })
        .limits({ max_queries: 20 })
        .build();
    case 'sub_batch_ts':
      return Batch.create()
        .subBatch('nested', Batch.create(), { bind: { x: 1 } })
        .build().queries.nested;
    case 'for_each_ts':
      return Batch.create()
        .forEach('loop', [1, 2], 'row', Batch.create())
        .build().queries.loop;
    default:
      throw new Error(`missing TS builder: ${name}`);
  }
}

describe('first-party wire operation matrix', () => {
  it('pins the case inventory and producer identifiers', () => {
    expect(cases.map(c => c.name)).toEqual([
      'read_complex',
      'read_page',
      'read_default_ts',
      'insert',
      'update',
      'delete',
      'upsert',
      'create_table',
      'drop_index',
      'rename_table',
      'get_schema',
      'remove_schema_rule',
      'add_schema_rule',
      'set_schema',
      'create_validator',
      'bind_validator',
      'unbind_validator',
      'drop_validator',
      'rename_validator',
      'list_validators',
      'call',
      'subscribe',
      'subscribe_default_ts',
      'batch_rust',
      'batch_ts',
      'sub_batch_rust',
      'for_each_rust',
      'sub_batch_ts',
      'for_each_ts',
    ]);
    expect(
      cases.every(c => c.builder.includes('ts:') || c.builder.includes('rust:')),
    ).toBe(true);
  });
  for (const c of cases.filter(c => c.builder.includes('ts:'))) {
    it(`TS builder emits ${c.name}`, () => {
      expect(build(c.name)).toEqual(c.wire);
    });
  }
});
