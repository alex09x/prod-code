/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { Type } from 'typebox';
import { appendDiagnostics, compact, query } from './cli.mjs';

const exec = promisify(execFile);

function cli(cwd: string, signal?: AbortSignal) {
  return async (args: string[]): Promise<string> => {
    const pending = exec('prod-code', args, {
      cwd,
      signal,
      encoding: 'utf8',
      shell: false,
      timeout: 20_000,
      killSignal: 'SIGKILL',
      maxBuffer: 256 * 1024,
    });
    // No batch input. Some queries wait for EOF when stdin is a pipe.
    pending.child.stdin?.end();
    try {
      return (await pending).stdout;
    } catch (cause) {
      const err = cause as { code?: string | number; stdout?: string; killed?: boolean };
      // Exit 1 means findings only when stdout contains a diagnostics report.
      // query() validates it before exposing any of it to the model.
      if (args[0] === 'diagnostics' && err.code === 1 && err.stdout?.trim()) return err.stdout;
      if (err.code === 'ENOENT' || err.code === 'EACCES') {
        throw new Error('prod-code unavailable: install the CLI and check that Pi can execute it on PATH.');
      }
      if (err.code === 'ABORT_ERR') throw new Error('prod-code query cancelled.');
      if (err.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER') {
        throw new Error('prod-code output too large; narrow the query.');
      }
      if (err.killed) throw new Error('prod-code query timed out.');
      // Raw process errors can include environment or connection details.
      throw new Error(`prod-code ${args[0]} failed; check the target and run prod-code status.`);
    }
  };
}

const target = Type.Object({
  symbol: Type.Optional(Type.String({
    minLength: 1, description: 'Symbol name, e.g. pkg.Func or Type::method',
  })),
  file: Type.Optional(Type.String({
    minLength: 1, description: 'Path relative to the session directory, or absolute',
  })),
  line: Type.Optional(Type.Integer({
    minimum: 1,
    description: '1-based line',
  })),
  col: Type.Optional(Type.Integer({
    minimum: 1,
    description: '1-based column',
  })),
}, { additionalProperties: false });

const tools = [
  ['definition', 'def', 'Show the symbol definition.'],
  ['references', 'refs', 'Find usages of the symbol.'],
  ['callers', 'callers', 'Find functions calling the symbol.'],
  ['hover', 'hover', 'Inspect symbol type and documentation.'],
  ['outline', 'outline', 'Outline declarations in the target file; file alone also works.'],
  ['symbols', 'symbols', 'Search workspace declarations by name, or list declarations in a file; file alone also works.'],
  ['diagnostics', 'diagnostics', 'Show analyzer errors and warnings for the target file; file alone also works.'],
] as const;

export default function prodCode(pi: ExtensionAPI) {
  for (const [name, command, description] of tools) {
    pi.registerTool({
      name: `code_${name}`,
      label: `prod-code ${command}`,
      description: `${description} Pass symbol OR file + line + col. Output limited to 100 lines / 8192 characters.`,
      promptSnippet: description,
      parameters: target,
      async execute(_id, params, signal, _update, ctx) {
        const text = compact(await query(command, params, cli(ctx.cwd, signal))) || 'No results.';
        return { content: [{ type: 'text', text }], details: undefined };
      },
    });
  }

  pi.on('tool_result', async (event, ctx) => {
    const patch = await appendDiagnostics(event, ctx.cwd, async (file: string) => {
      const report = await query('diagnostics', { file }, cli(ctx.cwd, ctx.signal));
      return compact(report);
    });
    if (patch) return { ...patch, structuredContent: event.structuredContent };
  });
}
