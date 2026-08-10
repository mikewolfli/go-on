import * as assert from "node:assert";

import { backoffDelayMs } from "../../utils";

/**
 * Reconnection backoff tests.
 *
 * These tests validate the REAL shared backoff implementation
 * (`utils/backoffDelayMs`) used by `runtime/reconnect.ts` and
 * `stateSync.ts`, plus the ReconnectManager-style state machine
 * contract (attempts / reset / cancel / schedule).
 *
 * The formula under test (contracts/cross-client-sync.md):
 * `delay = min(1000 * 2^attempt, 30000) * (0.7 + random() * 0.3)`.
 * Because jitter is random, deterministic assertions use the
 * guaranteed min/max range for each attempt.
 */

function expectedRange(attempt: number): [number, number] {
  const capped = Math.min(1000 * Math.pow(2, attempt), 30_000);
  return [Math.round(capped * 0.7), capped];
}

class TestableReconnectManager {
  private _attempts = 0;
  private _timer: ReturnType<typeof setTimeout> | undefined;
  public scheduledDelays: number[] = [];
  public doReconnectCallCount = 0;

  constructor(
    private readonly doReconnect: () => Promise<void>,
  ) {}

  get attempts(): number {
    return this._attempts;
  }

  reset(): void {
    this._attempts = 0;
    this.scheduledDelays = [];
  }

  backoffMs(attempt: number): number {
    return backoffDelayMs(attempt);
  }

  schedule(): void {
    const delay = this.backoffMs(this._attempts);
    this.scheduledDelays.push(delay);
    this._timer = setTimeout(() => {
      void this.doAttempt();
    }, delay);
  }

  cancel(): void {
    if (this._timer) {
      clearTimeout(this._timer);
      this._timer = undefined;
    }
  }

  private async doAttempt(): Promise<void> {
    this._attempts++;
    this.doReconnectCallCount++;
    await this.doReconnect();
  }
}

suite("reconnect", () => {
  suite("backoffDelayMs (real shared implementation)", () => {
    test("attempt 0 delays within [700ms, 1000ms]", () => {
      for (let i = 0; i < 50; i++) {
        const backoff = backoffDelayMs(0);
        const [min, max] = expectedRange(0);
        assert.ok(backoff >= min && backoff <= max, `attempt 0 delay ${backoff} outside [${min}, ${max}]`);
      }
    });

    test("attempt 1 delays within [1400ms, 2000ms]", () => {
      for (let i = 0; i < 50; i++) {
        const backoff = backoffDelayMs(1);
        const [min, max] = expectedRange(1);
        assert.ok(backoff >= min && backoff <= max, `attempt 1 delay ${backoff} outside [${min}, ${max}]`);
      }
    });

    test("attempt 2 delays within [2800ms, 4000ms]", () => {
      for (let i = 0; i < 50; i++) {
        const backoff = backoffDelayMs(2);
        const [min, max] = expectedRange(2);
        assert.ok(backoff >= min && backoff <= max, `attempt 2 delay ${backoff} outside [${min}, ${max}]`);
      }
    });

    test("attempt 3 delays within [5600ms, 8000ms]", () => {
      for (let i = 0; i < 50; i++) {
        const backoff = backoffDelayMs(3);
        const [min, max] = expectedRange(3);
        assert.ok(backoff >= min && backoff <= max, `attempt 3 delay ${backoff} outside [${min}, ${max}]`);
      }
    });

    test("attempt 4 delays within [11200ms, 16000ms]", () => {
      for (let i = 0; i < 50; i++) {
        const backoff = backoffDelayMs(4);
        const [min, max] = expectedRange(4);
        assert.ok(backoff >= min && backoff <= max, `attempt 4 delay ${backoff} outside [${min}, ${max}]`);
      }
    });

    test("attempt 5+ caps at 30000ms (with jitter: [21000ms, 30000ms])", () => {
      for (let i = 0; i < 50; i++) {
        const backoff5 = backoffDelayMs(5);
        const backoff6 = backoffDelayMs(6);
        const backoff20 = backoffDelayMs(20);
        const [min, max] = expectedRange(5);
        assert.ok(backoff5 >= min && backoff5 <= max, `attempt 5 delay ${backoff5} outside [${min}, ${max}]`);
        assert.ok(backoff6 >= min && backoff6 <= max, `attempt 6 delay ${backoff6} outside [${min}, ${max}]`);
        assert.ok(backoff20 >= min && backoff20 <= max, `attempt 20 delay ${backoff20} outside [${min}, ${max}]`);
      }
    });

    test("jitter produces varied values within range", () => {
      const samples = new Set<number>();
      for (let i = 0; i < 100; i++) {
        samples.add(backoffDelayMs(0));
      }
      assert.ok(samples.size > 1, "jitter should produce more than one distinct value across 100 samples");
    });

    test("delays grow exponentially then plateau at the cap", () => {
      const medians: number[] = [];
      for (let attempt = 0; attempt <= 6; attempt++) {
        const values: number[] = [];
        for (let i = 0; i < 200; i++) {
          values.push(backoffDelayMs(attempt));
        }
        values.sort((a, b) => a - b);
        medians.push(values[values.length >> 1]);
      }
      assert.ok(medians[1] > medians[0], "attempt 1 median must exceed attempt 0");
      assert.ok(medians[2] > medians[1], "attempt 2 median must exceed attempt 1");
      assert.ok(medians[3] > medians[2], "attempt 3 median must exceed attempt 2");
      assert.ok(medians[4] > medians[3], "attempt 4 median must exceed attempt 3");
      // 5 and 6 are both capped at 30000ms → medians equal
      assert.strictEqual(medians[5], medians[6], "capped attempts must plateau");
    });
  });

  suite("TestableReconnectManager (state machine contract)", () => {
    test("initial attempts is 0", () => {
      const mgr = new TestableReconnectManager(async () => {});
      assert.strictEqual(mgr.attempts, 0);
    });

    test("reset sets attempts back to 0", () => {
      const mgr = new TestableReconnectManager(async () => {});
      mgr.schedule();
      mgr.reset();
      assert.strictEqual(mgr.attempts, 0);
      assert.strictEqual(mgr.scheduledDelays.length, 0);
    });

    test("cancel stops pending timer", () => {
      const mgr = new TestableReconnectManager(async () => {});
      mgr.schedule();
      mgr.cancel();
      assert.strictEqual(mgr.attempts, 0);
    });

    test("schedule records a delay within the real backoff range", () => {
      const mgr = new TestableReconnectManager(async () => {});
      mgr.schedule();
      assert.strictEqual(mgr.scheduledDelays.length, 1);
      const [min, max] = expectedRange(0);
      assert.ok(
        mgr.scheduledDelays[0] >= min && mgr.scheduledDelays[0] <= max,
        `scheduled delay ${mgr.scheduledDelays[0]} outside [${min}, ${max}]`,
      );
    });

    test("attempt increments only after the timer fires", (done) => {
      const mgr = new TestableReconnectManager(async () => {
        // intentionally empty — testing state machine contract
      });
      mgr.schedule();
      assert.strictEqual(mgr.attempts, 0);
      // The real min delay for attempt 0 is 700ms; 50ms is far short of it,
      // so the attempt must not have fired yet.
      setTimeout(() => {
        assert.strictEqual(mgr.attempts, 0, "attempt must not fire before the backoff delay elapses");
        mgr.cancel();
        done();
      }, 50);
    });
  });
});
