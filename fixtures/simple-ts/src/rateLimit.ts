// Token-bucket rate limiting for incoming requests.

export interface RateLimitOptions {
  capacity: number;
  refillPerSecond: number;
}

export class TokenBucket {
  private tokens: number;
  private lastRefill: number;

  constructor(private options: RateLimitOptions) {
    this.tokens = options.capacity;
    this.lastRefill = Date.now();
  }

  tryTake(): boolean {
    this.refill();
    if (this.tokens < 1) {
      return false;
    }
    this.tokens -= 1;
    return true;
  }

  private refill(): void {
    const now = Date.now();
    const elapsedSeconds = (now - this.lastRefill) / 1000;
    this.tokens = Math.min(
      this.options.capacity,
      this.tokens + elapsedSeconds * this.options.refillPerSecond,
    );
    this.lastRefill = now;
  }
}

const buckets = new Map<string, TokenBucket>();

/**
 * Rate limiting entry point: returns false when the client identified by
 * `key` has exceeded its request budget and should receive a 429.
 */
export function rateLimit(key: string, options: RateLimitOptions): boolean {
  let bucket = buckets.get(key);
  if (!bucket) {
    bucket = new TokenBucket(options);
    buckets.set(key, bucket);
  }
  return bucket.tryTake();
}
