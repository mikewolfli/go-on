"use strict";
/**
 * Configuration Manager for Go-On
 *
 * Handles all configuration management including:
 * - Reading/writing TOML configuration
 * - Syncing with Go-On application
 * - Managing language settings
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.configManager = void 0;
const fs = require("fs/promises");
const path = require("path");
const os = require("os");
class ConfigManager {
    constructor() {
        this.config = null;
        this.configPath = '';
    }
    static getInstance() {
        if (!ConfigManager.instance) {
            ConfigManager.instance = new ConfigManager();
        }
        return ConfigManager.instance;
    }
    /**
     * Initialize configuration from file or create default
     */
    async initialize(configPath) {
        if (!configPath) {
            configPath = await this.getDefaultConfigPath();
        }
        this.configPath = configPath;
        try {
            await fs.access(configPath);
            await this.loadFromFile(configPath);
        }
        catch {
            // File doesn't exist, create default
            this.createDefaultConfig();
        }
    }
    /**
     * Get default configuration path
     */
    async getDefaultConfigPath() {
        const homeDir = os.homedir();
        const configDir = path.join(homeDir, '.go-on');
        try {
            await fs.mkdir(configDir, { recursive: true });
        }
        catch (error) {
            console.error('Failed to create config directory:', error);
            // Return fallback path if directory creation fails
            return path.join(homeDir, 'config.toml');
        }
        return path.join(configDir, 'config.toml');
    }
    /**
     * Load configuration from TOML file
     */
    async loadFromFile(filePath) {
        try {
            const content = await fs.readFile(filePath, 'utf-8');
            this.config = this.parseTOML(content);
        }
        catch (error) {
            console.error('Failed to load config:', error);
            this.createDefaultConfig();
        }
    }
    /**
     * Parse TOML content (simplified parser)
     */
    parseTOML(content) {
        // This is a simplified TOML parser
        // For production, consider using a proper TOML library
        const config = {
            agents: {},
            phases: {},
        };
        let currentSection = '';
        const lines = content.split('\n');
        for (const line of lines) {
            const trimmed = line.trim();
            // Skip comments and empty lines
            if (!trimmed || trimmed.startsWith('#'))
                continue;
            // Section header
            if (trimmed.startsWith('[')) {
                currentSection = trimmed.slice(1, -1);
                const parts = currentSection.split('.');
                if (parts[0] === 'agents') {
                    config.agents[parts[1]] = {};
                }
                else if (parts[0] === 'phases') {
                    config.phases[parts[1]] = {};
                }
                else {
                    if (!config[parts[0]]) {
                        config[parts[0]] = {};
                    }
                }
                continue;
            }
            // Key-value pair
            const match = trimmed.match(/^([^=]+)=(.+)$/);
            if (match) {
                const key = match[1].trim();
                const rawValue = match[2].trim();
                let value = rawValue;
                // Parse value type
                if (rawValue.startsWith('"') && rawValue.endsWith('"')) {
                    value = rawValue.slice(1, -1);
                }
                else if (rawValue === 'true') {
                    value = true;
                }
                else if (rawValue === 'false') {
                    value = false;
                }
                else if (!isNaN(Number(rawValue))) {
                    value = Number(rawValue);
                }
                else if (rawValue.startsWith('[')) {
                    // Simple array parsing
                    value = JSON.parse(rawValue.replace(/'/g, '"'));
                }
                const parts = currentSection.split('.');
                if (parts[0] === 'agents' && parts[1]) {
                    config.agents[parts[1]][key] = value;
                }
                else if (parts[0] === 'phases' && parts[1]) {
                    config.phases[parts[1]][key] = value;
                }
                else if (parts[0]) {
                    if (!config[parts[0]]) {
                        config[parts[0]] = {};
                    }
                    const section = config[parts[0]];
                    if (parts.length > 1) {
                        if (!section[parts[1]] || typeof section[parts[1]] !== 'object') {
                            section[parts[1]] = {};
                        }
                        section[parts[1]][key] = value;
                    }
                    else {
                        section[key] = value;
                    }
                }
                else {
                    config[key] = value;
                }
            }
        }
        return config;
    }
    /**
     * Create default configuration
     */
    createDefaultConfig() {
        this.config = {
            default_phase: 'coding',
            cache: {
                enabled: true,
                path: 'acp_cache.sqlite3',
                default_ttl_seconds: 3600,
                max_entries: 5000,
            },
            vector: {
                enabled: true,
                auto_mode: true,
                path: 'acp_vector.sqlite3',
                dimensions: 192,
                min_query_chars: 80,
                top_k: 2,
                min_similarity: 0.82,
                max_snippet_chars: 800,
                max_entries: 10000,
                summary_enabled: true,
                summary_trigger_messages: 8,
                summary_max_chars: 1200,
            },
            autotune: {
                enabled: true,
                evaluate_interval: 20,
                min_query_chars_step: 20,
                min_query_chars_min: 40,
                min_query_chars_max: 300,
                max_top_k: 4,
                low_precision_threshold: 0.35,
                high_precision_threshold: 0.75,
                state_path: 'acp_autotune_state.json',
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
                    type: 'copilot',
                    url: 'http://127.0.0.1:8080',
                    api_key_env: 'GITHUB_COPILOT_TOKEN',
                    region: 'us',
                },
                deepseek: {
                    type: 'deepseek',
                    api_key_env: 'DEEPSEEK_API_KEY',
                    model: 'deepseek-chat',
                    region: 'cn',
                },
            },
            flow: {
                name: 'Explicit Software Development Flow',
                phases: ['planning', 'coding', 'review', 'delivery'],
            },
            phases: {
                planning: {
                    description: 'Planning phase',
                    agents: ['deepseek', 'copilot'],
                    fallback: true,
                    principles: [
                        'Break the task into explicit milestones before editing code',
                        'Identify risky files, compatibility constraints, and rollback points early',
                    ],
                },
                coding: {
                    description: 'Coding phase',
                    agents: ['copilot', 'deepseek'],
                    fallback: true,
                    principles: [
                        'Use meaningful variable names',
                        'Each function should be no more than 50 lines',
                    ],
                },
                review: {
                    description: 'Review phase',
                    agents: ['deepseek'],
                    fallback: true,
                    principles: [
                        'Only return APPROVE when the implementation is safe',
                    ],
                },
                delivery: {
                    description: 'Delivery phase',
                    agents: ['copilot'],
                    fallback: false,
                    principles: [
                        'Deploy the approved changes',
                    ],
                },
            },
        };
    }
    /**
     * Get loaded configuration
     */
    getConfig() {
        if (!this.config) {
            this.createDefaultConfig();
        }
        if (!this.config) {
            throw new Error('Configuration could not be loaded or created.');
        }
        return this.config;
    }
    /**
     * Get configuration value by path (e.g., "cache.enabled")
     */
    getConfigValue(path, defaultValue) {
        if (!this.config) {
            return defaultValue;
        }
        const parts = path.split('.');
        let value = this.config;
        for (const part of parts) {
            if (value && typeof value === 'object' && part in value) {
                value = value[part];
            }
            else {
                return defaultValue;
            }
        }
        return value;
    }
    /**
     * Set configuration value
     */
    setConfigValue(path, value) {
        if (!this.config) {
            this.createDefaultConfig();
        }
        const parts = path.split('.');
        const lastPart = parts.pop();
        let current = this.config;
        for (const part of parts) {
            if (!(part in current) || typeof current[part] !== 'object' || current[part] === null) {
                current[part] = {};
            }
            current = current[part];
        }
        current[lastPart] = value;
    }
    /**
     * Save configuration to file
     */
    async saveToFile() {
        if (!this.config || !this.configPath) {
            throw new Error('Configuration not initialized');
        }
        try {
            const content = this.toTOML(this.config);
            await fs.writeFile(this.configPath, content, 'utf-8');
        }
        catch (error) {
            throw new Error(`Failed to save configuration: ${error}`);
        }
    }
    /**
     * Convert configuration to TOML format
     */
    toTOML(config) {
        let result = '';
        // Root level properties
        if (config.default_phase) {
            result += `default_phase = "${config.default_phase}"\n\n`;
        }
        // Cache section
        if (config.cache) {
            result += '[cache]\n';
            result += `enabled = ${config.cache.enabled}\n`;
            result += `path = "${config.cache.path}"\n`;
            result += `default_ttl_seconds = ${config.cache.default_ttl_seconds}\n`;
            result += `max_entries = ${config.cache.max_entries}\n\n`;
        }
        // Vector section
        if (config.vector) {
            result += '[vector]\n';
            result += `enabled = ${config.vector.enabled}\n`;
            result += `auto_mode = ${config.vector.auto_mode}\n`;
            result += `path = "${config.vector.path}"\n`;
            result += `dimensions = ${config.vector.dimensions}\n`;
            result += `min_query_chars = ${config.vector.min_query_chars}\n`;
            result += `top_k = ${config.vector.top_k}\n`;
            result += `min_similarity = ${config.vector.min_similarity}\n`;
            result += `max_snippet_chars = ${config.vector.max_snippet_chars}\n`;
            result += `max_entries = ${config.vector.max_entries}\n`;
            result += `summary_enabled = ${config.vector.summary_enabled}\n`;
            result += `summary_trigger_messages = ${config.vector.summary_trigger_messages}\n`;
            result += `summary_max_chars = ${config.vector.summary_max_chars}\n\n`;
        }
        // Agents section
        if (config.agents) {
            for (const [agentName, agentConfig] of Object.entries(config.agents)) {
                result += `[agents.${agentName}]\n`;
                for (const [key, value] of Object.entries(agentConfig)) {
                    if (typeof value === 'string') {
                        result += `${key} = "${value}"\n`;
                    }
                    else {
                        result += `${key} = ${JSON.stringify(value)}\n`;
                    }
                }
                result += '\n';
            }
        }
        // Autotune section
        if (config.autotune) {
            result += '[autotune]\n';
            result += `enabled = ${config.autotune.enabled}\n`;
            result += `evaluate_interval = ${config.autotune.evaluate_interval}\n`;
            result += `min_query_chars_step = ${config.autotune.min_query_chars_step}\n`;
            result += `min_query_chars_min = ${config.autotune.min_query_chars_min}\n`;
            result += `min_query_chars_max = ${config.autotune.min_query_chars_max}\n`;
            result += `max_top_k = ${config.autotune.max_top_k}\n`;
            result += `low_precision_threshold = ${config.autotune.low_precision_threshold}\n`;
            result += `high_precision_threshold = ${config.autotune.high_precision_threshold}\n`;
            result += `state_path = "${config.autotune.state_path}"\n`;
            result += `cooldown_windows = ${config.autotune.cooldown_windows}\n`;
            result += `min_vector_searches = ${config.autotune.min_vector_searches}\n`;
            result += `summary_trigger_min = ${config.autotune.summary_trigger_min}\n`;
            result += `summary_trigger_max = ${config.autotune.summary_trigger_max}\n\n`;
        }
        // Runtime section
        if (config.runtime) {
            result += '[runtime]\n';
            result += `maintenance_interval_seconds = ${config.runtime.maintenance_interval_seconds}\n`;
            result += `health_interval_seconds = ${config.runtime.health_interval_seconds}\n`;
            result += `shutdown_drain_seconds = ${config.runtime.shutdown_drain_seconds}\n\n`;
        }
        // Flow section
        if (config.flow) {
            result += '[flow]\n';
            result += `name = "${config.flow.name}"\n`;
            result += `phases = ${JSON.stringify(config.flow.phases)}\n\n`;
        }
        // Phases section
        if (config.phases) {
            for (const [phaseName, phaseConfig] of Object.entries(config.phases)) {
                result += `[phases.${phaseName}]\n`;
                for (const [key, value] of Object.entries(phaseConfig)) {
                    if (Array.isArray(value)) {
                        result += `${key} = ${JSON.stringify(value)}\n`;
                    }
                    else if (typeof value === 'string') {
                        result += `${key} = "${value}"\n`;
                    }
                    else {
                        result += `${key} = ${JSON.stringify(value)}\n`;
                    }
                }
                result += '\n';
            }
        }
        return result;
    }
}
exports.configManager = ConfigManager.getInstance();
//# sourceMappingURL=configManager.js.map