/**
 * Configuration Manager for Go-On
 *
 * Handles all configuration management including:
 * - Reading/writing TOML configuration
 * - Syncing with Go-On application
 * - Managing language settings
 */

import * as fs from "fs/promises";
import * as path from "path";
import * as os from "os";
import * as vscode from "vscode";
import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import { Logger } from "./logger";

const log = Logger.forModule("configManager");

export interface CacheConfig {
  enabled: boolean;
  path: string;
  default_ttl_seconds: number;
  max_entries: number;
  // Mirrors backend CacheConfig::persist_enabled — wire the durable cache as
  // the token cache's L3 layer. Defaults to true.
  persist_enabled: boolean;
}

export interface VectorConfig {
  enabled: boolean;
  auto_mode: boolean;
  path: string;
  dimensions: number;
  min_query_chars: number;
  top_k: number;
  min_similarity: number;
  max_snippet_chars: number;
  max_entries: number;
  summary_enabled: boolean;
  summary_trigger_messages: number;
  summary_max_chars: number;
}

export interface AutotuneConfig {
  enabled: boolean;
  evaluate_interval: number;
  min_query_chars_step: number;
  min_query_chars_min: number;
  min_query_chars_max: number;
  max_top_k: number;
  low_precision_threshold: number;
  high_precision_threshold: number;
  state_path: string;
  cooldown_windows: number;
  min_vector_searches: number;
  summary_trigger_min: number;
  summary_trigger_max: number;
}

export interface RuntimeConfig {
  maintenance_interval_seconds: number;
  health_interval_seconds: number;
  shutdown_drain_seconds: number;
}

export interface AgentConfig {
  type: string;
  url?: string;
  api_key_env?: string;
  secret_key_env?: string;
  model?: string;
  supports_system?: boolean;
  [key: string]: unknown;
  region?: string;
}

export interface PhaseConfig {
  description: string;
  agents: string[];
  fallback: boolean;
  principles: string[];
  options?: {
    [key: string]: unknown;
  };
}

export interface FlowConfig {
  name: string;
  phases: string[];
}

export interface GoOnConfig {
  default_phase: string;
  cache: CacheConfig;
  vector: VectorConfig;
  autotune: AutotuneConfig;
  runtime: RuntimeConfig;
  agents: {
    [key: string]: AgentConfig;
  };
  flow: FlowConfig;
  phases: {
    [key: string]: PhaseConfig;
  };
  [key: string]: unknown;
}

/**
 * Type guard that verifies an unknown value conforms to the `GoOnConfig` interface.
 * Checks runtime shape: required top-level keys and sub-object with `default_phase`.
 * Does not deeply validate every field — just confirms the shape is plausible.
 */
export function isGoOnConfig(obj: unknown): obj is GoOnConfig {
  if (typeof obj !== "object" || obj === null) return false;
  const record = obj as Record<string, unknown>;
  // Must have a string default_phase
  if (typeof record.default_phase !== "string") return false;
  // Must have a cache sub-object
  if (typeof record.cache !== "object" || record.cache === null) return false;
  // Must have a runtime sub-object
  if (typeof record.runtime !== "object" || record.runtime === null)
    return false;
  // Must have an agents sub-object
  if (typeof record.agents !== "object" || record.agents === null) return false;
  // Must have a flow sub-object
  if (typeof record.flow !== "object" || record.flow === null) return false;
  return true;
}

/**
 * Normalize known boolean fields in the parsed config that may arrive as
 * strings from TOML (e.g. `"true"` / `"false"` instead of `true` / `false`).
 */
function normalizeBooleans(config: Record<string, unknown>): void {
  const BOOLEAN_PATHS: Record<string, string[]> = {
    cache: ["enabled"],
    vector: ["enabled", "auto_mode", "summary_enabled"],
    autotune: ["enabled"],
    agents: ["supports_system"],
  };

  for (const [section, fields] of Object.entries(BOOLEAN_PATHS)) {
    const obj = config[section];
    if (!obj || typeof obj !== "object") continue;
    const record = obj as Record<string, unknown>;
    for (const field of fields) {
      const val = record[field];
      if (typeof val === "string") {
        const trimmed = val.trim().toLowerCase();
        if (trimmed === "true") record[field] = true;
        else if (trimmed === "false") record[field] = false;
      }
    }
  }
}

class ConfigManager {
  private config: GoOnConfig | null = null;
  private configPath: string = "";
  private static instance: ConfigManager;

  private constructor() {}

  static getInstance(): ConfigManager {
    if (!ConfigManager.instance) {
      ConfigManager.instance = new ConfigManager();
    }
    return ConfigManager.instance;
  }

  /**
   * Initialize configuration from file or create default
   */
  async initialize(configPath?: string): Promise<void> {
    if (!configPath) {
      configPath = await this.getDefaultConfigPath();
    }

    this.configPath = configPath;

    try {
      await fs.access(configPath);
      await this.loadFromFile(configPath);
    } catch (err) {
      log.warn("Config file not found, creating default:", err);
      this.createDefaultConfig();
    }
  }

  /**
   * Get default configuration path
   */
  private async getDefaultConfigPath(): Promise<string> {
    const homeDir = os.homedir();
    const configDir = path.join(homeDir, ".go-on");

    try {
      await fs.mkdir(configDir, { recursive: true });
    } catch (err) {
      log.warn("mkdir failed:", err);
      return path.join(homeDir, "config.toml");
    }

    return path.join(configDir, "config.toml");
  }

  /**
   * Load configuration from TOML file
   */
  private async loadFromFile(filePath: string): Promise<void> {
    try {
      const content = await fs.readFile(filePath, "utf-8");
      try {
        this.config = this.parseTOML(content);
      } catch (e) {
        const message = `Failed to parse TOML config: ${e}. Using defaults.`;
        log.warn(message);
        void vscode.window.showErrorMessage(`Go-On: ${message}`);
        this.createDefaultConfig();
      }
    } catch (err) {
      log.warn("loadFromFile failed:", err);
      this.createDefaultConfig();
    }
  }

  /**
   * Parse TOML content using the smol-toml library.
   * Handles inline tables, arrays of tables, multi-line strings, etc.
   */
  private parseTOML(content: string): GoOnConfig {
    const parsed = parseToml(content) as Record<string, unknown>;

    // Ensure sub-objects exist for expected sections
    const config: Record<string, unknown> = {
      agents: (parsed.agents as Record<string, unknown>) || {},
      phases: (parsed.phases as Record<string, unknown>) || {},
      ...parsed,
    };

    // Check that the parsed config has actual content
    const agents = config.agents as Record<string, unknown>;
    const phases = config.phases as Record<string, unknown>;
    const sectionKeys = Object.keys(config).filter(
      (k) => k !== "agents" && k !== "phases",
    );
    if (
      sectionKeys.length === 0 &&
      Object.keys(agents).length === 0 &&
      Object.keys(phases).length === 0
    ) {
      log.warn("TOML file appears empty or malformed, using defaults");
    }

    // Normalize known string-to-boolean fields from TOML (which may parse as strings)
    normalizeBooleans(config);

    // Use runtime type guard instead of raw `as unknown as GoOnConfig` cast.
    // This provides type safety: if the parsed TOML is missing a required field,
    // the guard catches it and falls back to defaults.
    if (isGoOnConfig(config)) {
      return config;
    }
    log.warn(
      "parsed TOML does not conform to GoOnConfig shape, using defaults",
    );
    this.createDefaultConfig();
    return this.config as GoOnConfig;
  }

  /**
   * Create default configuration
   *
   * Defaults mirror the single backend config `config/config.toml`
   * (source of truth). Keep field values in sync when it changes.
   */
  private createDefaultConfig(): void {
    // Values mirror the authoritative backend template config/config.toml.
    // Keep in sync when that file changes.
    this.config = {
      default_phase: "think",
      cache: {
        enabled: true,
        path: "acp_cache.sqlite3",
        default_ttl_seconds: 1800,
        max_entries: 2000,
        // Mirrors backend CacheConfig::persist_enabled (config/config.toml).
        persist_enabled: true,
      },
      vector: {
        enabled: true,
        auto_mode: true,
        path: "acp_vector.sqlite3",
        dimensions: 128,
        min_query_chars: 120,
        top_k: 2,
        min_similarity: 0.82,
        max_snippet_chars: 600,
        max_entries: 3000,
        summary_enabled: true,
        summary_trigger_messages: 4,
        summary_max_chars: 800,
      },
      autotune: {
        // Matches backend default (core/config/autotune.rs default_autotune_config).
        enabled: false,
        evaluate_interval: 20,
        min_query_chars_step: 20,
        min_query_chars_min: 40,
        min_query_chars_max: 300,
        max_top_k: 4,
        low_precision_threshold: 0.35,
        high_precision_threshold: 0.75,
        state_path: "acp_autotune_state.json",
        cooldown_windows: 2,
        min_vector_searches: 5,
        summary_trigger_min: 3,
        summary_trigger_max: 20,
      },
      runtime: {
        maintenance_interval_seconds: 60,
        health_interval_seconds: 120,
        shutdown_drain_seconds: 30,
      },
      agents: {
        copilot: {
          type: "copilot",
          url: "https://api.githubcopilot.com",
          api_key_env: "keyring://go-on/copilot_api_key",
          region: "us",
        },
        deepseek: {
          type: "deepseek",
          api_key_env: "keyring://go-on/deepseek_api_key",
          model: "deepseek-v4-flash",
          region: "cn",
        },
      },
      flow: {
        name: "Universal Adaptive",
        phases: ["think", "act", "check", "done"],
      },
      phases: {
        think: {
          description: "Think — analyze, plan, gather context",
          agents: ["deepseek", "copilot"],
          fallback: true,
          principles: [
            "Break the task into explicit milestones before editing code",
            "Identify risky files, compatibility constraints, and rollback points early",
          ],
        },
        act: {
          description: "Act — execute, generate, implement",
          agents: ["copilot", "deepseek"],
          fallback: true,
          principles: [
            "Use meaningful variable names",
            "Each function should be no more than 50 lines",
          ],
        },
        check: {
          description: "Check — verify output",
          agents: ["deepseek"],
          fallback: true,
          principles: ["Only return APPROVE when the implementation is safe"],
        },
        done: {
          description: "Done — finalize",
          agents: ["copilot"],
          fallback: false,
          principles: ["Deploy the approved changes"],
        },
      },
    };
  }

  /**
   * Get loaded configuration
   */
  getConfig(): GoOnConfig {
    if (!this.config) {
      this.createDefaultConfig();
    }
    if (!this.config) {
      throw new Error("Configuration could not be loaded or created.");
    }
    return this.config;
  }

  /**
   * Get configuration value by path (e.g., "cache.enabled")
   */
  getConfigValue(path: string, defaultValue?: unknown): unknown {
    if (!this.config) {
      return defaultValue;
    }

    const parts = path.split(".");
    let value: unknown = this.config;

    for (const part of parts) {
      if (
        value &&
        typeof value === "object" &&
        part in (value as Record<string, unknown>)
      ) {
        value = (value as Record<string, unknown>)[part];
      } else {
        return defaultValue;
      }
    }

    return value;
  }

  /**
   * Set configuration value
   */
  setConfigValue(path: string, value: unknown): void {
    if (!this.config) {
      this.createDefaultConfig();
    }

    const parts = path.split(".");
    const lastPart = parts.pop()!;
    let current = this.config as Record<string, unknown>;

    for (const part of parts) {
      if (
        !(part in current) ||
        typeof current[part] !== "object" ||
        current[part] === null
      ) {
        current[part] = {};
      }
      current = current[part] as Record<string, unknown>;
    }

    current[lastPart] = value;
  }

  /**
   * Save configuration to file
   */
  async saveToFile(): Promise<void> {
    if (!this.config || !this.configPath) {
      throw new Error("Configuration not initialized");
    }

    try {
      const content = this.toTOML(this.config);
      const tmpPath = this.configPath + ".tmp";
      await fs.writeFile(tmpPath, content, "utf-8");
      await fs.rename(tmpPath, this.configPath);
    } catch (error) {
      throw new Error(`Failed to save configuration: ${error}`);
    }
  }

  /**
   * Convert configuration to TOML format using the smol-toml library.
   * Properly handles inline tables, arrays, multi-line strings, etc.
   */
  private toTOML(config: GoOnConfig): string {
    return stringifyToml(config as Record<string, unknown>);
  }
}

export const configManager = ConfigManager.getInstance();
