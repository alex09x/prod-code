# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Conservative AST detector for inline Python rewrites of repository source files (#1760)."""

import ast
import os

from .python_write_eval import (
    Env,
    Val,
    _combine_paths,
    _resolve_repo_source_target,
    _SnippetEvalBase,
    _target_names,
)
from .shell_python import iter_inline_python_snippets


class _SnippetAnalyzer(_SnippetEvalBase):
    def scan_expr(self, node: ast.AST | None, env: Env, depth: int = 0) -> str | None:
        if node is None:
            return None
        if isinstance(
            node, (ast.ListComp, ast.SetComp, ast.GeneratorExp, ast.DictComp)
        ):
            comp_env = dict(env)
            for gen in node.generators:
                hit = self.scan_expr(gen.iter, comp_env, depth)
                if hit:
                    return hit
                vals = self.eval_expr(gen.iter, comp_env, depth)
                if isinstance(gen.target, ast.Name) and vals:
                    comp_env[gen.target.id] = vals
                else:
                    for name in _target_names(gen.target):
                        comp_env[name] = None
                for cond in gen.ifs:
                    hit = self.scan_expr(cond, comp_env, depth)
                    if hit:
                        return hit
            if isinstance(node, ast.DictComp):
                return self.scan_expr(node.key, comp_env, depth) or self.scan_expr(
                    node.value, comp_env, depth
                )
            return self.scan_expr(node.elt, comp_env, depth)

        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.expr):
                hit = self.scan_expr(child, env, depth)
                if hit:
                    return hit
        if isinstance(node, ast.Call):
            for p in self.call_write_targets(node, env, depth) or ():
                hit = _resolve_repo_source_target(p, self.cmd_cwd, self.initial_cwd)
                if hit:
                    return hit
            if (
                depth == 0
                and isinstance(node.func, ast.Name)
                and node.func.id in self.helpers
            ):
                fn_def = self.helpers[node.func.id]
                local_env = self.bind_helper(fn_def, node, env, depth)
                if local_env is not None:
                    return self.walk_stmts(fn_def.body, local_env, depth + 1)
        return None

    def _assign(
        self, target: ast.AST, value: ast.AST | None, val: Val, env: Env, depth: int
    ) -> None:
        if isinstance(target, ast.Name):
            env[target.id] = val
        elif (
            value is not None
            and isinstance(target, (ast.Tuple, ast.List))
            and isinstance(value, (ast.Tuple, ast.List))
            and len(target.elts) == len(value.elts)
        ):
            for lhs, rhs in zip(target.elts, value.elts):
                self._assign(lhs, rhs, self.eval_expr(rhs, env, depth), env, depth)
        else:
            for n in _target_names(target):
                env[n] = None

    def walk_stmts(self, stmts: list[ast.stmt], env: Env, depth: int = 0) -> str | None:
        for stmt in stmts:
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    self.imports[alias.asname or alias.name.split(".")[0]] = alias.name
            elif isinstance(stmt, ast.ImportFrom) and stmt.module:
                for alias in stmt.names:
                    if alias.name != "*":
                        self.imports[alias.asname or alias.name] = (
                            f"{stmt.module}.{alias.name}"
                        )
            elif isinstance(stmt, ast.FunctionDef) and depth == 0:
                self.helpers[stmt.name] = stmt
            elif isinstance(stmt, (ast.Assign, ast.AnnAssign)):
                val_node = stmt.value
                hit = self.scan_expr(val_node, env, depth)
                if hit:
                    return hit
                if val_node is not None:
                    val = self.eval_expr(val_node, env, depth)
                    targets = (
                        stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
                    )
                    for target in targets:
                        self._assign(target, val_node, val, env, depth)
            elif isinstance(stmt, ast.AugAssign):
                hit = self.scan_expr(stmt.value, env, depth)
                if hit:
                    return hit
                if isinstance(stmt.target, ast.Name):
                    rhs = self.eval_expr(stmt.value, env, depth)
                    env[stmt.target.id] = (
                        _combine_paths(
                            [env.get(stmt.target.id), rhs], lambda a, b: a + b
                        )
                        if isinstance(stmt.op, ast.Add)
                        else None
                    )
            elif isinstance(stmt, ast.For):
                hit = self.scan_expr(stmt.iter, env, depth)
                if hit:
                    return hit
                vals, loop_env = self.eval_expr(stmt.iter, env, depth), dict(env)
                self._assign(stmt.target, None, vals, loop_env, depth)
                hit = self.walk_stmts(stmt.body, loop_env, depth) or self.walk_stmts(
                    stmt.orelse, loop_env, depth
                )
                if hit:
                    return hit
                for k in set(env) | set(loop_env):
                    if env.get(k) != loop_env.get(k):
                        env[k] = None
            elif isinstance(stmt, ast.With):
                with_env = dict(env)
                for item in stmt.items:
                    hit = self.scan_expr(item.context_expr, with_env, depth)
                    if hit:
                        return hit
                    if item.optional_vars is not None:
                        self._assign(
                            item.optional_vars,
                            item.context_expr,
                            self.eval_expr(item.context_expr, with_env, depth),
                            with_env,
                            depth,
                        )
                hit = self.walk_stmts(stmt.body, with_env, depth)
                if hit:
                    return hit
                env.update(with_env)
            elif isinstance(stmt, ast.If):
                hit = self.scan_expr(stmt.test, env, depth)
                if hit:
                    return hit
                if isinstance(stmt.test, ast.Constant) and stmt.test.value is False:
                    hit = self.walk_stmts(stmt.orelse, env, depth)
                    if hit:
                        return hit
                    continue
                b_env, o_env = dict(env), dict(env)
                hit = self.walk_stmts(stmt.body, b_env, depth) or self.walk_stmts(
                    stmt.orelse, o_env, depth
                )
                if hit:
                    return hit
                if (
                    not stmt.orelse
                    and isinstance(stmt.test, ast.Compare)
                    and isinstance(stmt.test.left, ast.Name)
                    and stmt.test.left.id == "__name__"
                ):
                    env.update(b_env)
                else:
                    for k in set(b_env) | set(o_env):
                        env[k] = b_env.get(k) if b_env.get(k) == o_env.get(k) else None
            elif isinstance(stmt, ast.Try):
                t_env = dict(env)
                blocks = [
                    (stmt.body, t_env),
                    *(
                        [
                            (h.body, {**t_env, **({h.name: None} if h.name else {})})
                            for h in stmt.handlers
                        ]
                    ),
                    (stmt.orelse, t_env),
                    (stmt.finalbody, t_env),
                ]
                for body, blk_env in blocks:
                    hit = self.walk_stmts(body, blk_env, depth)
                    if hit:
                        return hit
            else:
                for child in ast.iter_child_nodes(stmt):
                    if isinstance(child, ast.expr):
                        hit = self.scan_expr(child, env, depth)
                        if hit:
                            return hit
        return None


def scripted_source_write_target(command: str, cwd: str | None = None) -> str | None:
    """Returns the repository/worktree source file an inline Python command rewrites, or None."""
    initial_cwd = os.path.normpath(os.path.abspath(cwd or os.getcwd()))
    for code, argv, cmd_cwd, shell_expanded in iter_inline_python_snippets(
        command, initial_cwd
    ):
        try:
            tree = ast.parse(code)
        except (SyntaxError, ValueError):
            continue
        hit = _SnippetAnalyzer(argv, cmd_cwd, initial_cwd, shell_expanded).walk_stmts(
            tree.body, {}
        )
        if hit:
            return hit
    return None
