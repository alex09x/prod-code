/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import assert from 'assert';
import { homedir } from 'os';
import { join } from 'path';
import { pathToFileURL } from 'url';
import {
  query, compact, appendDiagnostics, diagnosticsPatch, formatDiagnostics, resolveToCwd,
} from './cli.mjs';

const report = JSON.stringify({ file: 'a b.go', errors: 1, warnings: 1, items: [
  { line: 2, col: 3, severity: 'error', message: 'undefined: name' },
  { line: 5, col: 7, severity: 'warning', message: 'unused variable' },
  { line: 5, col: 7, severity: 'hint', message: 'not a finding' },
] });
const formatted = 'a b.go: 1 error(s), 1 warning(s)\n2:3 error: undefined: name\n5:7 warning: unused variable';
const position = { file: '@a b.go', line: 2, col: 3 };
const symbol = 'Type::method';

async function argumentsFor(command, target) {
  const calls = [];
  await query(command, target, async args => {
    calls.push(args);
    if (args[1] === '--no-body') return '📍 Definition: file:///src/a%20b.go:2:3\n';
    return args[0] === 'diagnostics' ? report : 'ok';
  });
  return calls;
}

async function argumentChecks() {
  for (const command of ['def', 'refs', 'callers', 'hover']) {
    assert.deepStrictEqual(await argumentsFor(command, position), [[command, '--', 'a b.go', '2', '3']]);
    assert.deepStrictEqual(await argumentsFor(command, { symbol }), [[command, '--symbol=Type::method']]);
  }
  for (const command of ['outline', 'symbols', 'diagnostics']) {
    const prefix = command === 'diagnostics' ? ['diagnostics', '--json', '--'] : [command, '--'];
    assert.deepStrictEqual(await argumentsFor(command, position), [[...prefix, 'a b.go']]);
    assert.deepStrictEqual(await argumentsFor(command, { file: '-r' }), [[...prefix, '-r']]);
    const expected = command === 'symbols' ? [['symbols', '--', symbol]] :
      [['def', '--no-body', '--symbol=Type::method'], [...prefix, '/src/a b.go']];
    assert.deepStrictEqual(await argumentsFor(command, { symbol }), expected);
  }
  for (const target of [{}, { symbol: ' ' }, { symbol, line: 4 }, { symbol, file: 'x' },
    { file: '' }, { file: 'x', col: 1 }, { file: 'x', line: -2, col: 1 }, { ...position, line: 1.5 }]) {
    await assert.rejects(query('refs', target, assert.fail), /Pass/);
  }
  await assert.rejects(query('def', { file: 'x.rs' }, assert.fail), /line and col/);
  await assert.rejects(query('outline', { symbol }, async () => 'ambiguous'), /unique definition/);
  assert.deepStrictEqual(await argumentsFor('symbols', { symbol: '--remote; echo nope' }),
    [['symbols', '--', '--remote; echo nope']]);
  assert.ok(compact('x'.repeat(20_000)).endsWith('[truncated; narrow the query]'));
}

async function diagnosticsChecks() {
  assert.strictEqual(formatDiagnostics(report), formatted);
  const clean = { file: 'a.go', errors: 0, warnings: 0, items: [] };
  assert.strictEqual(formatDiagnostics(JSON.stringify(clean)), 'a.go: 0 error(s), 0 warning(s)');
  for (const item of [null, { severity: 'error' }, { severity: 'warning', message: 'lost location' }]) {
    assert.throws(() => formatDiagnostics(JSON.stringify({ ...clean, items: [item] })),
      /diagnostics unavailable/);
  }
  for (const invalid of ['', '{}', 'unavailable']) {
    assert.throws(() => formatDiagnostics(invalid), /diagnostics unavailable/);
  }
  const original = { type: 'text', text: 'original result' };
  const base = { content: [original], input: { path: 'scratch.go' }, isError: false };
  for (const toolName of ['edit', 'write']) {
    const event = { ...base, toolName };
    for (const ext of ['go', 'rs', 'ts', 'tsx', 'js', 'py', 'swift', 'c', 'cc', 'cpp', 'h']) {
      const path = `scratch.${ext}`;
      const patch = await appendDiagnostics({ ...event, input: { path } }, '/w', async file => {
        assert.strictEqual(file, `/w/${path}`);
        return formatted;
      });
      assert.deepStrictEqual(patch, { content: [original, { type: 'text', text: formatted }] });
    }
    assert.strictEqual(await appendDiagnostics({ ...event, isError: true }, '/w', assert.fail), undefined);
    const failed = await appendDiagnostics(event, '/w', async () => { throw new Error('private backend details'); });
    const unavailable = { content: [original, { type: 'text', text: 'diagnostics unavailable' }] };
    assert.deepStrictEqual(failed, unavailable);
    let called = false;
    const skipped = await appendDiagnostics({ ...event, input: { path: 'notes.md' } }, '/w', () => { called = true; });
    assert.strictEqual(skipped, undefined);
    assert.strictEqual(called, false);
    assert.deepStrictEqual(event.content, [original]);
  }
  assert.strictEqual(await appendDiagnostics({ ...base, toolName: 'read' }, '/w', assert.fail), undefined);
}

// Spellings that Pi's edit and write resolve to a different file than the raw input names.
const spellings = [
  ['src/health check.rs', '/w/src/health check.rs'],
  ['src/a b.rs', '/w/src/a b.rs'],
  [pathToFileURL('/tmp/health check.rs').href, '/tmp/health check.rs'],
  ['@src/x.rs', '/w/src/x.rs'],
  ['@@x.rs', '/w/@x.rs'],
  ['~/x.rs', join(homedir(), 'x.rs')],
  ['./src/../y.rs', '/w/y.rs'],
  // Pi replaces Unicode spaces before decoding, so an encoded NBSP stays a real NBSP.
  ['file:///w/src/health%C2%A0check.rs', '/w/src/health check.rs'],
];

// The argv the CLI finally receives, through the production hook and query.
async function argvChecks() {
  const clean = JSON.stringify({ file: 'x', errors: 0, warnings: 0, items: [] });
  for (const [input, file] of spellings) {
    const calls = [];
    const event = { toolName: 'edit', isError: false, content: [], input: { path: input } };
    await diagnosticsPatch(event, '/w', async args => { calls.push(args); return clean; });
    assert.deepStrictEqual(calls, [['diagnostics', '--json', '--', file]], input);
  }
  // A definition file decoded from a URI is a resolved name: passed on verbatim.
  for (const command of ['outline', 'diagnostics']) {
    const calls = [];
    await query(command, { symbol }, async args => {
      calls.push(args);
      return args[1] === '--no-body' ? '📍 Definition: file:///w/a%C2%A0b.rs:1:1\n' : clean;
    });
    const prefix = command === 'diagnostics' ? ['diagnostics', '--json', '--'] : [command, '--'];
    assert.deepStrictEqual(calls[1], [...prefix, '/w/a b.rs'], command);
  }
}

async function pathChecks() {
  for (const [input, file] of spellings) {
    assert.strictEqual(resolveToCwd(input, '/w'), file);
    const event = { toolName: 'write', isError: false, content: [], input: { path: input } };
    const seen = [];
    await appendDiagnostics(event, '/w', async f => { seen.push(f); return 'ok'; });
    assert.deepStrictEqual(seen, [file], input);
  }
  // A file:// URL naming a .md file is not a source file once decoded.
  const md = { toolName: 'edit', isError: false, content: [], input: { path: 'file:///tmp/a.md' } };
  assert.strictEqual(await appendDiagnostics(md, '/w', assert.fail), undefined);
  // With PI_CODING_AGENT_DIR set, compare against the resolver Pi's edit and write use.
  const pi = process.env.PI_CODING_AGENT_DIR;
  if (!pi) return 'SKIP Pi resolver comparison (PI_CODING_AGENT_DIR unset)';
  const utils = pathToFileURL(join(pi, 'dist/core/tools/path-utils.js')).href;
  const { resolveToCwd: piResolve } = await import(utils);
  for (const [input] of spellings) {
    assert.strictEqual(resolveToCwd(input, '/w'), piResolve(input, '/w'), input);
  }
  return `PASS paths match Pi's resolveToCwd (${spellings.length} spellings)`;
}

async function main() {
  await argumentChecks();
  console.log('PASS CLI arguments and target validation');
  await diagnosticsChecks();
  console.log('PASS diagnostics formatting and tool-result append');
  const piComparison = await pathChecks();
  console.log('PASS diagnostics target follows Pi path resolution');
  await argvChecks();
  console.log('PASS resolved file names reach the CLI verbatim');
  console.log(piComparison);
}

main().catch(error => { console.error(error); process.exitCode = 1; });
