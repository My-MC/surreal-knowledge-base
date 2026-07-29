#!/usr/bin/env node
const { spawnSync } = require('node:child_process');
const { platform, arch } = require('node:process');
const path = require('node:path');
const fs = require('node:fs');

const PLATFORM_MAP = {
  'linux': 'linux',
  'darwin': 'darwin',
  'win32': 'win32',
};

const ARCH_MAP = {
  'x64': 'x64',
  'arm64': 'arm64',
};

const os = PLATFORM_MAP[platform];
const cpu = ARCH_MAP[arch];

if (!os || !cpu) {
  console.error(`skb-mcp: unsupported platform ${platform}-${arch}`);
  console.error('Supported: linux-x64, linux-arm64, darwin-x64, darwin-arm64, win32-x64');
  process.exit(1);
}

// Resolve the platform-specific binary from optionalDependencies
const pkgName = `@surreal-knowledge-base/${os}-${cpu}`;
let binaryPath;

try {
  const pkgDir = path.dirname(require.resolve(`${pkgName}/package.json`));
  binaryPath = path.join(pkgDir, 'bin', platform === 'win32' ? 'skb-mcp.exe' : 'skb-mcp');
} catch {
  console.error(`skb-mcp: platform package ${pkgName} is not installed.`);
  console.error(`Install with: npm install ${pkgName}`);
  process.exit(1);
}

if (!fs.existsSync(binaryPath)) {
  console.error(`skb-mcp: binary not found at ${binaryPath}`);
  process.exit(1);
}

// Execute the binary with stdio inheritance
const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  env: process.env,
});

process.exit(result.status ?? 1);
