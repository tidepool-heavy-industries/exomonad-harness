import { expect, it } from 'vitest';
import { compareDecimal, nextCounter, signedDecimal, unsignedDecimal } from './decimal';

it('compares and advances native counters above the JSON safe integer limit', () => {
  expect(unsignedDecimal('18446744073709551615')).toBe(true);
  expect(nextCounter('9007199254740992')).toBe('9007199254740993');
  expect(compareDecimal('9007199254740992', '9007199254740993')).toBe(-1);
  expect(compareDecimal('9', '10')).toBe(-1);
  expect(() => nextCounter('18446744073709551615')).toThrow();
});
it('validates canonical decimal spellings and both signed native bounds using bigint', () => {
  expect(signedDecimal('-9223372036854775808')).toBe(true);
  expect(signedDecimal('9223372036854775807')).toBe(true);
  for (const value of [1, '01', '-0', '+1', '1.0', '1e3', ' 1', '18446744073709551616']) {
    expect(unsignedDecimal(value)).toBe(false);
    expect(signedDecimal(value)).toBe(false);
  }
  expect(signedDecimal('9223372036854775808')).toBe(false);
  expect(signedDecimal('-9223372036854775809')).toBe(false);
});
