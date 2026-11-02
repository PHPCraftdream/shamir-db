import { describe, expect, it } from 'vitest';
import { decodeMessage } from '../protocol.js';

const decode = (wire: number[]) => decodeMessage(Uint8Array.from(wire));
const invalid = (wire: number[]) => expect(() => decode(wire)).toThrow();

describe('login MessagePack strict parsing', () => {
  it('accepts non-shortest unsigned integers but rejects every signed integer marker', () => {
    expect(decode([0x81, 0xa1, 0x78, 0xcc, 0x01])).toEqual({ x: 1 });
    for (const marker of [0xd0, 0xd1, 0xd2, 0xd3]) {
      const width = marker === 0xd0 ? 1 : marker === 0xd1 ? 2 : marker === 0xd2 ? 4 : 8;
      invalid([0x81, 0xa1, 0x78, marker, ...Array(width).fill(0)]);
    }
    invalid([0x81, 0xa1, 0x78, 0xd0, 0x01]);
  });
  it('rejects duplicate keys and trailing bytes', () => {
    invalid([0x82, 0xa1, 0x78, 0x01, 0xa1, 0x78, 0x02]);
    invalid([0x81, 0xa1, 0x78, 0x01, 0x00]);
  });
  it('decodes unknown fields for core protocol validation', () => {
    expect(decode([0x81, 0xa1, 0x79, 0x01])).toEqual({ y: 1 });
  });
  it('rejects malformed UTF-8 and truncated values', () => {
    invalid([0x81, 0xa1, 0x78, 0xa1, 0xff]);
    invalid([0x81, 0xa1, 0x78, 0xcc]);
  });
  it('decodes unsigned uint32 and uint64 markers without signed coercion', () => {
    expect(decode([0x81, 0xa1, 0x78, 0xce, 0xff, 0xff, 0xff, 0xff]).x).toBe(0xffff_ffff);
    expect(decode([0x81, 0xa1, 0x78, 0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).x).toBe(0xffff_ffff_ffff_ffffn);
    expect(decode([0x81, 0xa1, 0x78, 0xcf, 0, 0, 0, 0, 0, 0, 0, 1]).x).toBe(1n);
  });
  it('rejects non-string map keys, unsupported markers and non-map top-level values', () => {
    invalid([0x81, 0x01, 0x02]);
    invalid([0x81, 0xa1, 0x78, 0xc7, 0x00, 0x01, 0x01]);
    invalid([0x90]);
    invalid([0xc4, 0x00]);
  });
  it('rejects excessive nesting', () => {
    invalid([0x81, 0xa1, 0x78, ...Array.from({ length: 34 }, () => 0x91), 0x00]);
  });
});
