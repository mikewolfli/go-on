#!/usr/bin/env node
/**
 * Locale consistency checker.
 * Reads en-US.json, zh-CN.json, zh-TW.json and reports missing/mismatched keys.
 */

const fs = require("fs");
const path = require("path");

const LOCALE_DIR = path.resolve(__dirname, "../src/locales");
const FILES = ["en-US.json", "zh-CN.json", "zh-TW.json"];

/**
 * Recursively collect all leaf-keys with their dot-notation paths.
 */
function collectKeys(obj, prefix = "") {
  const keys = new Set();
  for (const key of Object.keys(obj)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (obj[key] !== null && typeof obj[key] === "object" && !Array.isArray(obj[key])) {
      const childKeys = collectKeys(obj[key], fullKey);
      for (const ck of childKeys) keys.add(ck);
    } else {
      keys.add(fullKey);
    }
  }
  return keys;
}

function loadJSON(file) {
  const content = fs.readFileSync(path.join(LOCALE_DIR, file), "utf-8");
  return JSON.parse(content);
}

function main() {
  const locales = {};
  const allKeys = new Set();

  for (const file of FILES) {
    const data = loadJSON(file);
    const keys = collectKeys(data);
    locales[file] = keys;
    for (const k of keys) allKeys.add(k);
  }

  const sortedKeys = [...allKeys].sort();

  console.log("\n========================================");
  console.log("   LOCALE CONSISTENCY CHECK");
  console.log("========================================\n");

  let hasIssues = false;

  // Check each file for missing keys
  for (const file of FILES) {
    const missing = sortedKeys.filter((k) => !locales[file].has(k));
    if (missing.length > 0) {
      hasIssues = true;
      console.log(`\n❌ ${file} is MISSING ${missing.length} key(s):`);
      for (const k of missing) {
        console.log(`   - ${k}`);
      }
    } else {
      console.log(`✅ ${file} has all ${sortedKeys.length} keys.`);
    }
  }

  // Check for keys in zh-CN or zh-TW but not in en-US (orphaned keys)
  const enKeys = locales["en-US.json"];
  for (const file of ["zh-CN.json", "zh-TW.json"]) {
    const orphaned = [...locales[file]].filter((k) => !enKeys.has(k));
    if (orphaned.length > 0) {
      hasIssues = true;
      console.log(`\n⚠️  ${file} has ${orphaned.length} key(s) not in en-US.json:`);
      for (const k of orphaned) {
        console.log(`   - ${k}`);
      }
    }
  }

  // Summary statistics
  console.log("\n----------------------------------------");
  console.log("   STATISTICS");
  console.log("----------------------------------------");
  for (const file of FILES) {
    console.log(`   ${file}: ${locales[file].size} keys`);
  }
  console.log(`   Total unique keys: ${sortedKeys.length}`);

  if (!hasIssues) {
    console.log("\n🎉 All locale files are consistent! No missing or orphaned keys.\n");
  } else {
    console.log("\n⚠️  Some inconsistencies found. Review the details above.\n");
    process.exit(1);
  }
}

main();
