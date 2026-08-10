declare module "vitest" {
  export function describe(name: string, fn: () => void): void;
  export function it(name: string, fn: () => Promise<void> | void): void;
  export function expect(value: unknown): Matcher;
  export function beforeEach(fn: () => void): void;

  export interface Matcher {
    toBe(v: unknown): void;
    toEqual(v: unknown): void;
    toThrow(): void;
    toBeInstanceOf(c: unknown): void;
    toBeGreaterThanOrEqual(v: number): void;
    toBeUndefined(): void;
    toHaveLength(n: number): void;
    rejects: { toThrow(): Promise<void> };
    resolves: { toBe(v: unknown): Promise<void>; toEqual(v: unknown): Promise<void> };
  }

  // `vi.fn()` returns a callable mock assignable to any function type.
  export interface Mock<T extends (...args: any[]) => any = (...args: any[]) => any>
    extends T {
    mockResolvedValue(v: unknown): Mock<T>;
    mockRejectedValue(v: unknown): Mock<T>;
    // Each call is a tuple of its arguments; arguments are loosely typed so
    // tests can read e.g. `calls[0][1].body` (RequestInit in the fetch mock).
    mock: { calls: any[][] };
  }

  export const vi: {
    fn<T extends (...args: any[]) => any>(impl?: T): Mock<T>;
    clearAllMocks(): void;
  };
}
