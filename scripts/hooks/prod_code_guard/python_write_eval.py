# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Path expression evaluation and repository source-target resolution for Python write AST checks."""

import ast
import os
import re

from .shell import _is_source_code_path, git_repo_root

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


class _SnippetEvalBase:
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
