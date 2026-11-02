import { describe, expect, it } from 'vitest';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const testDirectory = dirname(fileURLToPath(import.meta.url));
const sourceRoot = resolve(testDirectory, '../../../');
const roots = [
  resolve(sourceRoot, 'browser.ts'),
  resolve(sourceRoot, 'platform/connect/browser-connect.ts'),
  resolve(sourceRoot, 'platform/token/socket-browser.ts'),
  resolve(sourceRoot, 'platform/token/http-browser.ts'),
  resolve(sourceRoot, 'entry/ws/token-ws-client.ts'),
];
const relativeSpecifiers = /^\s*(?:import|export)\s+(?:[^;\n]|\n(?!\s*(?:import|export)\b))*?\bfrom\s*['"]([^'"]+)['"]|^\s*import\s*\(\s*['"]([^'"]+)['"]\s*\)/gm;
const nodeSpecifier = /^\s*(?:import|export)\s+(?:[^;\n]|\n(?!\s*(?:import|export)\b))*?\bfrom\s*['"](node:[^'"]+)['"]|^\s*import\s*\(\s*['"](node:[^'"]+)['"]\s*\)/gm;

async function sourceGraph(file: string, visited = new Set<string>(), forbidNode = true): Promise<Set<string>> {
  const normalized = resolve(file);
  if (visited.has(normalized)) return visited;
  visited.add(normalized);
  const source = await readFile(normalized, 'utf8');
  if (forbidNode) {
    for (const match of source.matchAll(nodeSpecifier)) {
      const specifier = match[1] ?? match[2];
      if (specifier) throw new Error(`runtime Node import in browser graph: ${specifier}`);
    }
  }
  for (const match of source.matchAll(relativeSpecifiers)) {
    const specifier = match[1] ?? match[2];
    if (!specifier?.startsWith('.')) continue;
    const target = resolve(dirname(normalized), specifier.replace(/\.js$/, '.ts'));
    await readFile(target, 'utf8');
    await sourceGraph(target, visited, forbidNode);
  }
  return visited;
}

describe('browser runtime source graph', () => {
  it('has no runtime Node imports in browser socket, HTTP, or WS client dependencies', async () => {
    for (const root of roots) {
      const graph = await sourceGraph(root);
      expect(graph.has(root)).toBe(true);
      if (root.endsWith('token-ws-client.ts')) {
        expect(graph.has(resolve(sourceRoot, 'entry/ws/token-lifecycle.ts'))).toBe(true);
        expect(graph.has(resolve(sourceRoot, 'platform/token/socket-browser.ts'))).toBe(true);
      }
    }
  });
  it('keeps Node connectors out of the browser graph and browser connectors out of Node', async () => {
    const browserGraph = await sourceGraph(resolve(sourceRoot, 'browser.ts'));
    const nodeGraph = await sourceGraph(resolve(sourceRoot, 'index.ts'), new Set(), false);
    for (const path of browserGraph) {
      const content = await readFile(path, 'utf8');
      expect(content).not.toMatch(nodeSpecifier);
      expect(content).not.toMatch(/^\s*(?:import|export)\s+(?:[^;\n]|\n(?!\s*(?:import|export)\b))*?\bfrom\s*['"]ws(?:\/[^'"]*)?['"]|^\s*import\s*\(\s*['"]ws(?:\/[^'"]*)?['"]\s*\)/gm);
    }
    expect(browserGraph.has(resolve(sourceRoot, 'platform/connect/browser-connect.ts'))).toBe(true);
    expect(browserGraph.has(resolve(sourceRoot, 'platform/connect/node-connect.ts'))).toBe(false);
    expect(nodeGraph.has(resolve(sourceRoot, 'platform/connect/node-connect.ts'))).toBe(true);
    expect(nodeGraph.has(resolve(sourceRoot, 'platform/connect/browser-connect.ts'))).toBe(false);
    for (const path of nodeGraph) {
      const content = await readFile(path, 'utf8');
      expect(content).not.toContain('connectTokenBrowser');
    }
    for (const path of browserGraph) {
      const content = await readFile(path, 'utf8');
      expect(content).not.toContain('connectTokenNode');
      expect(content).not.toContain('createNodeTokenPlatform');
    }
  });
});
