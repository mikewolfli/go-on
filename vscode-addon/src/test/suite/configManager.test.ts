import * as assert from "node:assert";
import { configManager } from "../../configManager";

suite("configManager", () => {
  suite("ConfigManager", () => {
    test("getConfig() returns a default config when not initialized", () => {
      const config = configManager.getConfig();
      assert.ok(config, "config should be defined");
      assert.strictEqual(config.default_phase, "coding");
    });

    test("default config has all required sections", () => {
      const config = configManager.getConfig();
      assert.ok(config.cache, "cache section should exist");
      assert.ok(config.vector, "vector section should exist");
      assert.ok(config.autotune, "autotune section should exist");
      assert.ok(config.runtime, "runtime section should exist");
      assert.ok(config.agents, "agents section should exist");
      assert.ok(config.flow, "flow section should exist");
      assert.ok(config.phases, "phases section should exist");
    });

    test("default cache config has expected values", () => {
      const config = configManager.getConfig();
      assert.strictEqual(config.cache.enabled, true);
      assert.strictEqual(config.cache.default_ttl_seconds, 3600);
      assert.strictEqual(config.cache.max_entries, 5000);
      assert.ok(config.cache.path.length > 0);
    });

    test("default vector config has expected values", () => {
      const config = configManager.getConfig();
      assert.strictEqual(config.vector.enabled, true);
      assert.strictEqual(config.vector.auto_mode, true);
      assert.strictEqual(config.vector.dimensions, 192);
      assert.strictEqual(config.vector.top_k, 2);
      assert.strictEqual(config.vector.min_similarity, 0.82);
      assert.strictEqual(config.vector.max_entries, 10000);
    });

    test("default autotune config has expected values", () => {
      const config = configManager.getConfig();
      assert.strictEqual(config.autotune.enabled, true);
      assert.strictEqual(config.autotune.evaluate_interval, 20);
      assert.strictEqual(config.autotune.max_top_k, 4);
    });

    test("default runtime config has expected values", () => {
      const config = configManager.getConfig();
      assert.strictEqual(config.runtime.maintenance_interval_seconds, 60);
      assert.strictEqual(config.runtime.health_interval_seconds, 120);
      assert.strictEqual(config.runtime.shutdown_drain_seconds, 30);
    });

    test("default agents are copilot and deepseek", () => {
      const config = configManager.getConfig();
      assert.ok(config.agents.copilot, "copilot agent should exist");
      assert.strictEqual(config.agents.copilot.type, "copilot");
      assert.ok(config.agents.deepseek, "deepseek agent should exist");
      assert.strictEqual(config.agents.deepseek.type, "deepseek");
    });

    test("default flow has four phases", () => {
      const config = configManager.getConfig();
      assert.strictEqual(
        config.flow.name,
        "Explicit Software Development Flow",
      );
      assert.deepStrictEqual(config.flow.phases, [
        "planning",
        "coding",
        "review",
        "delivery",
      ]);
    });

    test("default phases contain planning, coding, review, delivery", () => {
      const config = configManager.getConfig();
      assert.ok(config.phases.planning, "planning phase should exist");
      assert.ok(config.phases.coding, "coding phase should exist");
      assert.ok(config.phases.review, "review phase should exist");
      assert.ok(config.phases.delivery, "delivery phase should exist");
      assert.strictEqual(config.phases.planning.fallback, true);
      assert.strictEqual(config.phases.delivery.fallback, false);
    });
  });

  suite("getConfigValue", () => {
    test("returns value for a top-level key", () => {
      const val = configManager.getConfigValue("default_phase");
      assert.strictEqual(val, "coding");
    });

    test("returns value for a nested key (dot notation)", () => {
      const val = configManager.getConfigValue("cache.enabled");
      assert.strictEqual(val, true);
    });

    test("returns value for deeply nested key", () => {
      const val = configManager.getConfigValue("agents.copilot.type");
      assert.strictEqual(val, "copilot");
    });

    test("returns default value for missing key", () => {
      const val = configManager.getConfigValue("nonexistent.key", "fallback");
      assert.strictEqual(val, "fallback");
    });

    test("returns undefined as default when key is missing and no default given", () => {
      const val = configManager.getConfigValue("nonexistent.key");
      assert.strictEqual(val, undefined);
    });

    test("returns default for completely missing path", () => {
      const val = configManager.getConfigValue("cache.doesNotExist", 42);
      assert.strictEqual(val, 42);
    });
  });

  suite("setConfigValue", () => {
    test("sets a top-level value", () => {
      configManager.setConfigValue("default_phase", "review");
      const val = configManager.getConfigValue("default_phase");
      assert.strictEqual(val, "review");
      // Reset
      configManager.setConfigValue("default_phase", "coding");
    });

    test("sets a nested value", () => {
      configManager.setConfigValue("cache.max_entries", 1000);
      const val = configManager.getConfigValue("cache.max_entries");
      assert.strictEqual(val, 1000);
      // Reset
      configManager.setConfigValue("cache.max_entries", 5000);
    });

    test("creates intermediate objects for a deeply nested path", () => {
      configManager.setConfigValue("experimental.featureA.enabled", true);
      const val = configManager.getConfigValue("experimental.featureA.enabled");
      assert.strictEqual(val, true);
    });

    test("overwrites existing value", () => {
      configManager.setConfigValue("cache.enabled", false);
      const val = configManager.getConfigValue("cache.enabled");
      assert.strictEqual(val, false);
      configManager.setConfigValue("cache.enabled", true);
    });

    test("sets a string value", () => {
      configManager.setConfigValue("flow.name", "Custom Flow");
      const val = configManager.getConfigValue("flow.name");
      assert.strictEqual(val, "Custom Flow");
      configManager.setConfigValue(
        "flow.name",
        "Explicit Software Development Flow",
      );
    });
  });

  suite("settings parsing", () => {
    test("getConfigValue returns boolean for boolean fields", () => {
      const val = configManager.getConfigValue("vector.summary_enabled");
      assert.strictEqual(typeof val, "boolean");
    });

    test("getConfigValue returns number for numeric fields", () => {
      const val = configManager.getConfigValue("vector.dimensions");
      assert.strictEqual(typeof val, "number");
      assert.strictEqual(val, 192);
    });

    test("getConfigValue returns string for string fields", () => {
      const val = configManager.getConfigValue("cache.path");
      assert.strictEqual(typeof val, "string");
    });

    test("flow.phases is an array of strings", () => {
      const val = configManager.getConfigValue("flow.phases");
      assert.ok(Array.isArray(val));
    });

    test("agents is a record with agent config objects", () => {
      const agents = configManager.getConfigValue("agents") as Record<
        string,
        unknown
      >;
      assert.ok(agents);
      assert.ok(typeof agents === "object");
      assert.ok("copilot" in agents);
    });
  });

  suite("path resolution", () => {
    test("getConfigValue returns agent.url for copilot", () => {
      const url = configManager.getConfigValue("agents.copilot.url");
      assert.strictEqual(url, "http://127.0.0.1:8080");
    });

    test("getConfigValue returns agent.api_key_env", () => {
      const key = configManager.getConfigValue("agents.deepseek.api_key_env");
      assert.strictEqual(key, "keyring://go-on/deepseek_api_key");
    });

    test("getConfigValue for agent.model", () => {
      const model = configManager.getConfigValue("agents.deepseek.model");
      assert.strictEqual(model, "deepseek-chat");
    });

    test("phases have principles array", () => {
      const principles = configManager.getConfigValue(
        "phases.planning.principles",
      ) as string[];
      assert.ok(Array.isArray(principles));
      assert.ok(principles.length > 0);
      assert.ok(
        principles[0].includes("Break the task into explicit milestones"),
      );
    });
  });
});
