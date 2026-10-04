/** Native counters remain strings on the wire and BigInts during comparison. */
export function unsignedDecimal(value: unknown): value is string {
  return typeof value === 'string' && /^(?:0|[1-9][0-9]*)$/.test(value)
    && value.length <= 20 && BigInt(value).toString() === value && BigInt(value) <= 18446744073709551615n;
}
export function signedDecimal(value: unknown): value is string {
  if (typeof value !== 'string' || !/^(?:0|-?[1-9][0-9]*)$/.test(value) || value.length > 20) return false;
  const integer = BigInt(value);
  return integer.toString() === value && integer >= -9223372036854775808n && integer <= 9223372036854775807n;
}
export function compareDecimal(left: string, right: string): number {
  const a = BigInt(left), b = BigInt(right);
  return a < b ? -1 : a > b ? 1 : 0;
}
