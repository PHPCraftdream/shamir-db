import { describe, it, expect } from 'vitest';
import { canonicalBatchOp } from '../hmac.js';

const bytes = (value: Uint8Array) => Array.from(value);

const canon = (extra: Record<string, unknown>) =>
  bytes(
    canonicalBatchOp('', {
      create_function: 'f',
      replace: false,
      secret_grants: [],
      net_grants: [],
      ...extra,
    } as never),
  );

describe('canonical create_function — lang/optimize', () => {
  it('lang changes bytes (wasm vs rust)', () => {
    expect(canon({ wasm: 'AAEC', lang: 'wasm' })).not.toEqual(
      canon({ wasm: 'AAEC', lang: 'rust' }),
    );
  });

  it('optimize changes bytes pairwise', () => {
    const auto = canon({ wasm: 'AAEC', optimize: 'auto' });
    const now = canon({ wasm: 'AAEC', optimize: 'now' });
    const never = canon({ wasm: 'AAEC', optimize: 'never' });
    expect(auto).not.toEqual(now);
    expect(auto).not.toEqual(never);
    expect(now).not.toEqual(never);
  });

  it('equal resolved values give equal bytes', () => {
    expect(canon({ wasm: 'AAEC', lang: 'wasm', optimize: 'now' })).toEqual(
      canon({ wasm: 'AAEC', lang: 'wasm', optimize: 'now' }),
    );
  });

  it('wasm defaults equal explicit wasm/auto', () => {
    expect(canon({ wasm: 'AAEC' })).toEqual(
      canon({ wasm: 'AAEC', lang: 'wasm', optimize: 'auto' }),
    );
  });

  it('source-only default lang equals explicit rust', () => {
    expect(canon({ source: 'src' })).toEqual(
      canon({ source: 'src', lang: 'rust' }),
    );
  });

  it('wasm with absent lang differs from explicit rust', () => {
    expect(canon({ wasm: 'AAEC' })).not.toEqual(
      canon({ wasm: 'AAEC', lang: 'rust' }),
    );
  });

  it('shamir differs from rust', () => {
    expect(canon({ source: 'src', lang: 'shamir' })).not.toEqual(
      canon({ source: 'src', lang: 'rust' }),
    );
  });
});
