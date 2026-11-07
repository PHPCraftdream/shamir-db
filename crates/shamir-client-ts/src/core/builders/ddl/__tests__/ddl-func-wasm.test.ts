import { describe, it, expect } from 'vitest';
import { ddl } from '../index.js';

describe('ddl.createFunction — lang/optimize', () => {
  it('omits lang and optimize when unset', () => {
    const op = ddl.createFunction('f', { wasm: 'AAEC' });
    expect(op).toEqual({
      create_function: 'f',
      replace: false,
      wasm: 'AAEC',
    });
    expect(op).not.toHaveProperty('lang');
    expect(op).not.toHaveProperty('optimize');
  });

  it.each(['wasm', 'rust', 'shamir'] as const)('threads lang %s', (lang) => {
    const op = ddl.createFunction('f', { source: 'src', lang });
    expect(op).toEqual({
      create_function: 'f',
      replace: false,
      source: 'src',
      lang,
    });
    expect(op).not.toHaveProperty('optimize');
  });

  it.each(['auto', 'now', 'never'] as const)('threads optimize %s', (optimize) => {
    const op = ddl.createFunction('f', { wasm: 'AAEC', optimize });
    expect(op).toEqual({
      create_function: 'f',
      replace: false,
      wasm: 'AAEC',
      optimize,
    });
    expect(op).not.toHaveProperty('lang');
  });
});

describe('ddl.describeFunction / exportFunction', () => {
  it('describeFunction wire shape', () => {
    expect(ddl.describeFunction('f')).toEqual({
      describe_function: 'f',
    });
  });

  it('exportFunction wire shape', () => {
    expect(ddl.exportFunction('f')).toEqual({
      export_function: 'f',
    });
  });
});

describe('ddl.checkFunction', () => {
  it('wasm only', () => {
    expect(ddl.checkFunction({ wasm: 'AAEC' })).toEqual({
      check_function: {
        wasm: 'AAEC',
      },
    });
  });

  it('source only', () => {
    expect(ddl.checkFunction({ source: 'src' })).toEqual({
      check_function: {
        source: 'src',
      },
    });
  });

  it('with lang and optimize', () => {
    expect(
      ddl.checkFunction({ source: 'src', lang: 'shamir', optimize: 'now' }),
    ).toEqual({
      check_function: {
        lang: 'shamir',
        source: 'src',
        optimize: 'now',
      },
    });
  });

  it('omits unset fields', () => {
    const op = ddl.checkFunction({ wasm: 'AAEC' });
    expect(op.check_function).not.toHaveProperty('lang');
    expect(op.check_function).not.toHaveProperty('source');
    expect(op.check_function).not.toHaveProperty('optimize');
  });

  it('throws TypeError when neither wasm nor source', () => {
    expect(() => ddl.checkFunction({})).toThrow(TypeError);
    expect(() => ddl.checkFunction({ lang: 'wasm' })).toThrow(TypeError);
  });

  it('throws TypeError when both wasm and source', () => {
    expect(() => ddl.checkFunction({ wasm: 'AAEC', source: 'src' })).toThrow(
      TypeError,
    );
  });
});

describe('ddl.alterFunction', () => {
  it.each(['auto', 'now', 'never'] as const)('optimize %s', (optimize) => {
    expect(ddl.alterFunction('f', optimize)).toEqual({
      alter_function: 'f',
      optimize,
    });
  });
});

describe('function type-level checks', () => {
  it('rejects invalid lang/optimize at compile time', () => {
    const _typeChecks = () => {
      // @ts-expect-error 'fast' is not a FunctionOptimize
      ddl.alterFunction('f', 'fast');
      // @ts-expect-error 'cobol' is not a FunctionLang
      ddl.createFunction('f', { wasm: 'x', lang: 'cobol' });
      // @ts-expect-error 'turbo' is not a FunctionOptimize
      ddl.checkFunction({ wasm: 'x', optimize: 'turbo' });
    };
    void _typeChecks;
    expect(true).toBe(true);
  });
});
