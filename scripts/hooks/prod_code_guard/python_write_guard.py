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
import re

from .shell import _is_source_code_path, git_repo_root
from .shell_python import iter_inline_python_snippets

_MAX_PATHS = 16
_SHELL_VAR_RE = re.compile(r"\$[A-Za-z_({]")
_PATH_CTORS = frozenset(
    {
        "Path",
        "PurePath",
        "PosixPath",
        "PurePosixPath",
        "pathlib.Path",
        "pathlib.PurePath",
        "pathlib.PosixPath",
        "pathlib.PurePosixPath",
    }
)
_PASSTHROUGH_FUNCS = frozenset(
    {
        "str",
        "os.fspath",
        "os.path.abspath",
        "os.path.expanduser",
        "os.path.normpath",
        "os.path.realpath",
        "posixpath.abspath",
        "posixpath.expanduser",
        "posixpath.normpath",
    }
)
_OPEN_FUNCS = frozenset({"open", "builtins.open", "io.open", "codecs.open"})
_DST_WRITE_FUNCS = frozenset(
    {
        "os.replace",
        "os.rename",
        "shutil.move",
        "shutil.copy",
        "shutil.copy2",
        "shutil.copyfile",
    }
)

Val = tuple[str, ...] | None
Env = dict[str, Val]


def _combine_paths(parts: list[Val], joiner) -> Val:
    acc: tuple[str, ...] = ("",)
    for idx, part in enumerate(parts):
        if not part:
            return None
        acc = part if idx == 0 else tuple(joiner(a, b) for a in acc for b in part)
        if not acc or len(acc) > _MAX_PATHS:
            return None
    return acc


def _is_write_mode(mode_val: Val) -> bool:
    return bool(mode_val) and any(
        any(c in m for c in ("w", "a", "x", "+")) for m in mode_val
    )


def _target_names(node: ast.AST) -> list[str]:
    if isinstance(node, ast.Name):
        return [node.id]
    if isinstance(node, (ast.Tuple, ast.List)):
        return [n for elt in node.elts for n in _target_names(elt)]
    return _target_names(node.value) if isinstance(node, ast.Starred) else []


def _get_arg(call: ast.Call, pos: int, kw_name: str) -> ast.expr | None:
    if any(isinstance(a, ast.Starred) for a in call.args):
        return None
    if 0 <= pos < len(call.args):
        return call.args[pos]
    for kw in call.keywords:
        if kw.arg is None:
            return None
        if kw.arg == kw_name:
            return kw.value
    return None


def _const_int(node: ast.AST) -> int | None:
    if (
        isinstance(node, ast.Constant)
        and isinstance(node.value, int)
        and not isinstance(node.value, bool)
    ):
        return node.value
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
        v = _const_int(node.operand)
        return -v if v is not None else None
    return None


def _resolve_repo_source_target(
    path_str: str, cmd_cwd: str | None, initial_cwd: str
) -> str | None:
    if not path_str or path_str in ("-", "/dev/stdout", "/dev/stderr", "/dev/null"):
        return None
    if any(part in path_str for part in ("://", "\n", "\r", "\x00")):
        return None
    expanded = os.path.expanduser(path_str)
    was_abs = os.path.isabs(expanded)
    if not was_abs and cmd_cwd is None:
        return None
    resolved = os.path.normpath(
        expanded if was_abs else os.path.join(cmd_cwd, expanded)
    )
    if not _is_source_code_path(resolved, cmd_cwd or initial_cwd):
        return None
    repo_root = git_repo_root(resolved)
    if repo_root is None:
        return None
    rel_repo = os.path.relpath(resolved, repo_root)
    if rel_repo == ".git" or rel_repo.startswith(".git" + os.sep):
        return None
    rel_init = os.path.relpath(resolved, initial_cwd)
    if not rel_init.startswith("..") and git_repo_root(initial_cwd) == repo_root:
        return rel_init
    return resolved if was_abs else rel_repo


class _SnippetAnalyzer:
    def __init__(
        self,
        argv: list[tuple[str, bool]],
        cmd_cwd: str | None,
        initial_cwd: str,
        shell_expanded: bool,
    ):
        self.argv, self.cmd_cwd, self.initial_cwd, self.shell_expanded = (
            argv,
            cmd_cwd,
            initial_cwd,
            shell_expanded,
        )
        self.imports: dict[str, str] = {}
        self.helpers: dict[str, ast.FunctionDef] = {}

    def qualname(self, node: ast.AST) -> str | None:
        if isinstance(node, ast.Name):
            return self.imports.get(node.id, node.id)
        if isinstance(node, ast.Attribute):
            parent = self.qualname(node.value)
            return f"{parent}.{node.attr}" if parent else None
        return None

    def eval_expr(self, node: ast.AST | None, env: Env, depth: int = 0) -> Val:
        if node is None:
            return None
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return (
                None
                if (self.shell_expanded and _SHELL_VAR_RE.search(node.value))
                else (node.value,)
            )
        if isinstance(node, ast.Name):
            return env.get(node.id)
        if isinstance(node, ast.JoinedStr):
            parts = [
                self.eval_expr(
                    v.value if isinstance(v, ast.FormattedValue) else v, env, depth
                )
                for v in node.values
            ]
            return _combine_paths(parts, lambda a, b: a + b) if parts else ("",)
        if isinstance(node, ast.BinOp):
            left, right = (
                self.eval_expr(node.left, env, depth),
                self.eval_expr(node.right, env, depth),
            )
            if isinstance(node.op, ast.Add):
                return _combine_paths([left, right], lambda a, b: a + b)
            return (
                _combine_paths([left, right], os.path.join)
                if isinstance(node.op, ast.Div)
                else None
            )
        if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
            if not node.elts or any(isinstance(e, ast.Starred) for e in node.elts):
                return None
            vals: list[str] = []
            for elt in node.elts:
                sub = self.eval_expr(elt, env, depth)
                if not sub or len(vals) + len(sub) > _MAX_PATHS:
                    return None
                vals.extend(sub)
            return tuple(vals)
        if isinstance(node, ast.Subscript) and self.qualname(node.value) == "sys.argv":
            idx = _const_int(node.slice)
            if idx is not None and -len(self.argv) <= idx < len(self.argv):
                val, exp = self.argv[idx]
                return None if exp else (val,)
            return None
        if isinstance(node, ast.Call):
            qname = self.qualname(node.func)
            no_star = not any(isinstance(a, ast.Starred) for a in node.args)
            if qname in _PATH_CTORS and not node.keywords and no_star:
                return (
                    (".",)
                    if not node.args
                    else _combine_paths(
                        [self.eval_expr(a, env, depth) for a in node.args], os.path.join
                    )
                )
            if (
                qname in ("os.path.join", "posixpath.join")
                and node.args
                and not node.keywords
                and no_star
            ):
                return _combine_paths(
                    [self.eval_expr(a, env, depth) for a in node.args], os.path.join
                )
            if (
                qname in _PASSTHROUGH_FUNCS
                and len(node.args) == 1
                and not node.keywords
            ):
                return self.eval_expr(node.args[0], env, depth)
            if (
                isinstance(node.func, ast.Attribute)
                and node.func.attr in ("expanduser", "resolve", "absolute")
                and not node.args
                and not node.keywords
            ):
                return self.eval_expr(node.func.value, env, depth)
            if (
                depth == 0
                and isinstance(node.func, ast.Name)
                and node.func.id in self.helpers
            ):
                fn_def = self.helpers[node.func.id]
                local_env = self.bind_helper(fn_def, node, env, depth)
                if (
                    local_env is not None
                    and len(fn_def.body) == 1
                    and isinstance(fn_def.body[0], ast.Return)
                ):
                    return self.eval_expr(fn_def.body[0].value, local_env, depth + 1)
        return None

    def bind_helper(
        self, fn: ast.FunctionDef, call: ast.Call, env: Env, depth: int
    ) -> Env | None:
        if any(isinstance(a, ast.Starred) for a in call.args) or any(
            k.arg is None for k in call.keywords
        ):
            return None
        if fn.args.posonlyargs or fn.args.vararg or fn.args.kwarg:
            return None
        local = dict(env)
        pos_params = [a.arg for a in fn.args.args]
        default_offset = len(pos_params) - len(fn.args.defaults)
        for i, name in enumerate(pos_params):
            if i < len(call.args):
                local[name] = self.eval_expr(call.args[i], env, depth)
            elif i >= default_offset:
                local[name] = self.eval_expr(
                    fn.args.defaults[i - default_offset], env, depth
                )
            else:
                local[name] = None
        for kw_param, kw_def in zip(fn.args.kwonlyargs, fn.args.kw_defaults):
            local[kw_param.arg] = (
                self.eval_expr(kw_def, env, depth) if kw_def is not None else None
            )
        for kw in call.keywords:
            if kw.arg in local:
                local[kw.arg] = self.eval_expr(kw.value, env, depth)
        return local

    def call_write_targets(self, call: ast.Call, env: Env, depth: int) -> Val:
        qname = self.qualname(call.func)
        if qname in _OPEN_FUNCS:
            f_arg, m_arg = _get_arg(call, 0, "file"), _get_arg(call, 1, "mode")
            if (
                f_arg is not None
                and m_arg is not None
                and _is_write_mode(self.eval_expr(m_arg, env, depth))
            ):
                return self.eval_expr(f_arg, env, depth)
            return None
        if qname in _DST_WRITE_FUNCS:
            dst = _get_arg(call, 1, "dst")
            return self.eval_expr(dst, env, depth) if dst is not None else None
        if qname in ("fileinput.input", "fileinput.FileInput"):
            inp, f_arg = _get_arg(call, -1, "inplace"), _get_arg(call, 0, "files")
            if (
                isinstance(inp, ast.Constant)
                and inp.value is True
                and f_arg is not None
            ):
                return self.eval_expr(f_arg, env, depth)
            return None
        if isinstance(call.func, ast.Attribute):
            attr = call.func.attr
            if attr in ("write_text", "write_bytes"):
                return self.eval_expr(call.func.value, env, depth)
            if attr == "open":
                m_arg = _get_arg(call, 0, "mode")
                if m_arg is not None and _is_write_mode(
                    self.eval_expr(m_arg, env, depth)
                ):
                    return self.eval_expr(call.func.value, env, depth)
            if (
                attr in ("replace", "rename")
                and len(call.args) + len(call.keywords) == 1
            ):
                dst = _get_arg(call, 0, "target")
                return self.eval_expr(dst, env, depth) if dst is not None else None
        return None

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
