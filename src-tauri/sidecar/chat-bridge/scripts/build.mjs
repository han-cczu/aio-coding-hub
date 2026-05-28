// Build script for chat-bridge. Bundles src/index.ts into dist/chat-bridge.js
// and inlines the resolved @anthropic-ai/claude-agent-sdk version so the
// sidecar can advertise it in its `ready` event without doing a runtime
// package.json lookup (the SDK's `exports` field blocks ./package.json).

import { build } from 'esbuild';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

const own = readJson(resolve(root, 'package.json'));
const sdk = readJson(resolve(root, 'node_modules/@anthropic-ai/claude-agent-sdk/package.json'));

await build({
  entryPoints: [resolve(root, 'src/index.ts')],
  bundle: true,
  platform: 'node',
  target: 'node18',
  format: 'esm',
  outfile: resolve(root, 'dist/chat-bridge.js'),
  // esbuild ESM banner: shim `require` for any CJS deps in the bundle.
  banner: {
    js: "import { createRequire as __cr } from 'node:module'; const require = __cr(import.meta.url);",
  },
  define: {
    __SIDECAR_VERSION__: JSON.stringify(own.version),
    __SDK_VERSION__: JSON.stringify(sdk.version),
  },
  logLevel: 'info',
});
