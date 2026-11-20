/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import { fileURLToPath } from 'url';
import { homedir } from 'os';
import { isAbsolute, join, resolve } from 'path';

const fileCommands = new Set(['outline', 'symbols', 'diagnostics']);
const commands = new Set(['def', 'refs', 'callers', 'hover', ...fileCommands]);
const validPosition = n => Number.isSafeInteger(n) && n >= 1;
const unicodeSpaces = /[  -   　]/g;

// The same spelling rules as Pi's own read/edit/write paths (normalizePath with
// normalizeUnicodeSpaces and stripAtPrefix), so a tool names the file Pi touched.
export function normalizeFile(file) {
  let path = file.replace(unicodeSpaces, ' ');
  if (path.startsWith('@')) path = path.slice(1);
  if (path === '~') return homedir();
  if (path.startsWith('~/')) return join(homedir(), path.slice(2));
  return /^file:\/\//.test(path) ? fileURLToPath(path) : path;
}

// Pi's resolveToCwd: the absolute file an edit or write on `file` wrote.
export function resolveToCwd(file, cwd) {
  const path = normalizeFile(file);
  return isAbsolute(path) ? resolve(path) : resolve(cwd, path);
}

function locationFile(output) {
  const locations = output.trim().split('\n');
  const match = locations.length === 1 && /^📍 Definition: (file:\/\/.*):\d+:\d+$/.exec(locations[0]);
  if (!match) throw new Error('No unique definition file; pass a file instead.');
  return fileURLToPath(match[1]);
}

// File-scoped commands have no --symbol switch. Resolve a single definition,
// not a fuzzy workspace-symbol match, before issuing the actual request.
/**
 * @param {string} command
 * @param {{symbol?: string, file?: string, line?: number, col?: number}} target
 * @param {(args: string[]) => Promise<string>} run
 */
export async function query(command, { symbol, file, line, col }, run) {
  if (!commands.has(command)) throw new Error('Unsupported prod-code query.');
  const fileScoped = fileCommands.has(command);
  // `path` is a resolved file name and is passed on verbatim from here on:
  // normalizing it again would turn a real NBSP in a file name into a space.
  let path;
  if (symbol !== undefined) {
    if (typeof symbol !== 'string' || !symbol.trim() || [file, line, col].some(x => x !== undefined)) {
      throw new Error('Pass a nonempty symbol alone, or a file target.');
    }
    if (command === 'symbols') return run(['symbols', '--', symbol]);
    if (!fileScoped) return run([command, `--symbol=${symbol}`]);
    path = locationFile(await run(['def', '--no-body', `--symbol=${symbol}`]));
  } else {
    if (typeof file !== 'string' || !normalizeFile(file).trim()) {
      throw new Error('Pass symbol or file.');
    }
    if ((!fileScoped || line !== undefined || col !== undefined) && (!validPosition(line) || !validPosition(col))) {
      throw new Error('Pass both line and col as positive 1-based integers.');
    }
    path = normalizeFile(file);
  }
  if (command === 'diagnostics') return diagnose(path, run);
  if (fileScoped) return run([command, '--', path]);
  return run([command, '--', path, String(line), String(col)]);
}

// Diagnostics for an already resolved file name, passed to the CLI verbatim.
export async function diagnose(path, run) {
  return formatDiagnostics(await run(['diagnostics', '--json', '--', path]));
}

export function compact(text) {
  const full = text.trim();
  const head = full.split('\n', 100).join('\n').slice(0, 8192);
  return head === full ? full : head + '\n[truncated; narrow the query]';
}

const source = /\.(?:go|rs|ts|tsx|js|py|swift|c|cc|cpp|h)$/i;

export function formatDiagnostics(output) {
  let report;
  try { report = JSON.parse(output); } catch { /* Invalid or missing CLI report. */ }
  if (!report || typeof report.file !== 'string' || !Array.isArray(report.items) ||
      !Number.isSafeInteger(report.errors) || report.errors < 0 ||
      !Number.isSafeInteger(report.warnings) || report.warnings < 0) {
    throw new Error('diagnostics unavailable');
  }
  const findings = [];
  for (const item of report.items) {
    if (!item || typeof item.severity !== 'string') throw new Error('diagnostics unavailable');
    if (item.severity !== 'error' && item.severity !== 'warning') continue;
    if (typeof item.message !== 'string' || !validPosition(item.line) || !validPosition(item.col)) {
      throw new Error('diagnostics unavailable');
    }
    findings.push(item);
  }
  return [`${report.file}: ${report.errors} error(s), ${report.warnings} warning(s)`,
    ...findings.map(d => `${d.line}:${d.col} ${d.severity}: ${d.message}`)].join('\n');
}

// Return a partial tool_result patch; never replace edit details or its status.
export async function appendDiagnostics(event, cwd, check) {
  if (event.isError || (event.toolName !== 'edit' && event.toolName !== 'write')) return;
  if (typeof event.input.path !== 'string') return;
  // Diagnose the file Pi wrote, not the raw spelling: a non-breaking space or a
  // file:// URL in the input names a different path until it is resolved.
  let file;
  try { file = resolveToCwd(event.input.path, cwd); } catch { return; }
  // Only source files have analyzer diagnostics; anything else keeps its result untouched.
  if (!source.test(file)) return;
  let text = 'diagnostics unavailable';
  try { text = await check(file); } catch { /* Preserve the successful edit. */ }
  return { content: [...event.content, { type: 'text', text }] };
}

// The tool_result hook: diagnostics of the file Pi wrote, its resolved name
// passed to the CLI verbatim.
export function diagnosticsPatch(event, cwd, run) {
  return appendDiagnostics(event, cwd, async file => compact(await diagnose(file, run)));
}
