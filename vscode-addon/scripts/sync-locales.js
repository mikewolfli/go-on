const fs = require('fs');
const path = require('path');

const sourceDir = path.resolve(__dirname, '..', 'src', 'locales');
const targetDir = path.resolve(__dirname, '..', 'out', 'locales');

if (!fs.existsSync(sourceDir)) {
  console.error(`Source locales directory not found: ${sourceDir}`);
  process.exit(1);
}

fs.mkdirSync(targetDir, { recursive: true });
fs.cpSync(sourceDir, targetDir, { recursive: true });
