/**
 * Simple LRU cache with TTL support
 */
export interface CacheEntry<T> {
    value: T;
    timestamp: number;
    ttl: number;
}

export class RpcCache {
    private cache: Map<string, CacheEntry<any>> = new Map();
    private cleanupInterval: NodeJS.Timeout | null = null;

    constructor(cleanupIntervalMs: number = 60000) {
        // Cleanup expired entries every 60 seconds
        this.cleanupInterval = setInterval(() => this.cleanup(), cleanupIntervalMs);
    }

    set<T>(key: string, value: T, ttl: number = 5000) {
        this.cache.set(key, {
            value,
            timestamp: Date.now(),
            ttl,
        });
    }

    get<T>(key: string): T | null {
        const entry = this.cache.get(key);
        if (!entry) return null;

        const age = Date.now() - entry.timestamp;
        if (age > entry.ttl) {
            this.cache.delete(key);
            return null;
        }

        return entry.value as T;
    }

    has(key: string): boolean {
        return this.get(key) !== null;
    }

    delete(key: string) {
        this.cache.delete(key);
    }

    clear() {
        this.cache.clear();
    }

    private cleanup() {
        const now = Date.now();
        for (const [key, entry] of this.cache.entries()) {
            if (now - entry.timestamp > entry.ttl) {
                this.cache.delete(key);
            }
        }
    }

    destroy() {
        if (this.cleanupInterval) {
            clearInterval(this.cleanupInterval);
        }
        this.clear();
    }
}

export const rpcCache = new RpcCache();
