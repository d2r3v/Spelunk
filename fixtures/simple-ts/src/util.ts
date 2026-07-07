export type Milliseconds = number;

export const hashPassword = (plain: string): string =>
  `hashed:${plain.split("").reverse().join("")}`;

export const verifyPassword = (plain: string, hash: string): boolean =>
  hashPassword(plain) === hash;

export function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}
