#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Regenerate architecture diagrams in docs/img in the ringfire visual style.
Dependency-free (hand-written SVG) so the figures are reproducible anywhere."""
import os, textwrap
from collections.abc import Sequence

OUT = os.path.join(os.path.dirname(__file__), "..", "docs", "img")
ASSETS_OUT = os.path.join(os.path.dirname(__file__), "..", "assets")
FONT = "font-family='JetBrains Mono, SFMono-Regular, Menlo, monospace'"
INK, MUTED, GRID, PANEL = "#1f2328", "#6a737d", "#d0d7de", "#f6f8fa"
RING, RING_FILL = "#c0392b", "#fdecea"      # Red/Coral: Salsa DB, Primary Storage
NET, NET_FILL = "#1d4ed8", "#e8efff"        # Blue: Network, 10GbE, Protocol Bridges
OK, OK_FILL = "#2e7d32", "#e8f5e9"          # Green: Shadow Validation, Passing Tests
PURPLE, PURPLE_FILL = "#6f42c1", "#f3e8ff"  # Purple: AI Agents, Language Servers
AMBER, AMBER_FILL = "#b45309", "#fef3c7"    # Amber: Telemetry, Gossip Router

def esc(t):
    return str(t).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

def para(x, y, text, width=95, color=None, size=11, lh=16):
    col = color or MUTED
    return "".join(f"<text x='{x}' y='{y + i * lh}' fill='{col}' font-size='{size}'>{esc(line)}</text>"
                   for i, line in enumerate(textwrap.wrap(text, width)))

def head(W, H, title, sub=None):
    s = [f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {W} {H}' width='{W}' height='{H}' {FONT} font-size='12'>",
         "<defs>",
         f"<marker id='a' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{INK}'/></marker>",
         f"<marker id='n' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{NET}'/></marker>",
         f"<marker id='r' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{RING}'/></marker>",
         f"<marker id='g' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{OK}'/></marker>",
         "</defs>",
         f"<rect width='{W}' height='{H}' fill='white'/>",
         f"<text x='16' y='24' font-size='15' font-weight='bold' fill='{INK}'>{esc(title)}</text>"]
    if sub:
        s.append(f"<text x='16' y='42' fill='{MUTED}'>{esc(sub)}</text>")
    return s

def box(x, y, w, h, text, sub=None, fill="white", stroke=INK, bold=True, size=11):
    t = f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='6' fill='{fill}' stroke='{stroke}' stroke-width='1.5'/>"
    sub_lines = [line.strip() for line in sub.split('\n')] if sub else []
    if not sub_lines:
        t += f"<text x='{x + w / 2}' y='{y + h / 2 + 4}' text-anchor='middle' fill='{INK}' font-size='{size}' font-weight='{'bold' if bold else 'normal'}'>{esc(text)}</text>"
    elif len(sub_lines) == 1:
        t += f"<text x='{x + w / 2}' y='{y + h / 2 - 2}' text-anchor='middle' fill='{INK}' font-size='{size}' font-weight='{'bold' if bold else 'normal'}'>{esc(text)}</text>"
        t += f"<text x='{x + w / 2}' y='{y + h / 2 + 12}' text-anchor='middle' fill='{MUTED}' font-size='9.5'>{esc(sub_lines[0])}</text>"
    else:
        start_y = y + (h - (12 + len(sub_lines) * 12)) / 2 + 10
        t += f"<text x='{x + w / 2}' y='{start_y}' text-anchor='middle' fill='{INK}' font-size='{size}' font-weight='{'bold' if bold else 'normal'}'>{esc(text)}</text>"
        for i, s_line in enumerate(sub_lines):
            line_y = start_y + 13 + i * 12
            t += f"<text x='{x + w / 2}' y='{line_y}' text-anchor='middle' fill='{MUTED}' font-size='9'>{esc(s_line)}</text>"
    return t

def panel(x, y, w, h, title, fill=PANEL):
    return (f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='10' fill='{fill}' stroke='{GRID}'/>"
            f"<text x='{x + 12}' y='{y + 20}' fill='{INK}' font-weight='bold' font-size='13'>{esc(title)}</text>")

def arrow(x1, y1, x2, y2, label=None, color=INK, marker="a", dash=None, above=True, size=11):
    d = f" stroke-dasharray='{dash}'" if dash else ""
    t = f"<line x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}' stroke='{color}' stroke-width='1.5' marker-end='url(#{marker})'{d}/>"
    if label:
        ly = min(y1, y2) - 7 if above else max(y1, y2) + 15
        t += f"<text x='{(x1 + x2) / 2}' y='{ly}' text-anchor='middle' fill='{color}' font-size='{size}'>{esc(label)}</text>"
    return t

def write(name: str, s: Sequence[str]) -> None:
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(ASSETS_OUT, exist_ok=True)
    content = "\n".join([*s, "</svg>"])
    for d in [OUT, ASSETS_OUT]:
        p = os.path.join(d, name)
        with open(p, "w", encoding="utf-8") as f:
            f.write(content)

# ==============================================================================
# 1. Architecture Overview (Ringfire style)
# ==============================================================================
W, H = 960, 480
s = head(W, H, "Architecture: developer workstation to 9-node cluster, isolated worktree overlays",
         "Mirroring over mounting · Base64 delta sync · In-memory shadow validation · Gossip routing")

# Left: Workstation Panel
s.append(panel(16, 56, 310, 260, "developer workstation (0% CPU load)"))
s.append(box(28, 86, 134, 40, "AI agents", "Claude, Codex, AGY", fill=PURPLE_FILL, stroke=PURPLE))
s.append(box(174, 86, 138, 40, "editors & CLI", "Zed, VSCode, Neovim", fill="white", stroke=INK))

s.append(arrow(95, 126, 95, 146))
s.append(arrow(243, 126, 243, 146))

s.append(box(28, 146, 134, 42, "prod-code mcp", "67 tools · hot reload", fill=NET_FILL, stroke=NET))
s.append(box(174, 146, 138, 42, "prod-code lsp", "stdio LSP bridge", fill=NET_FILL, stroke=NET))

s.append(arrow(95, 188, 160, 208))
s.append(arrow(243, 188, 180, 208))

s.append(box(28, 208, 284, 48, "fast mirror & sync engine", "Base64 diffs · watermarks · stdlib cache", fill="white", stroke=INK))
s.append(f"<text x='170' y='276' text-anchor='middle' fill='{MUTED}' font-size='10'>0.76s cold sync (10.8 MB) · sub-ms edits</text>")

# Center: Network Band
s.append(f"<rect x='340' y='66' width='160' height='240' rx='10' fill='{NET_FILL}' stroke='{NET}' stroke-dasharray='4 3'/>")
s.append(f"<text x='420' y='88' text-anchor='middle' fill='{NET}' font-weight='bold'>10 GbE LAN mesh</text>")
s.append(arrow(312, 232, 514, 232, "Base64 delta", NET, "n"))
s.append(arrow(514, 166, 312, 166, "AST responses", NET, "n"))
s.append(f"<text x='420' y='125' text-anchor='middle' fill='{NET}' font-size='10'>RPC: :9400</text>")
s.append(f"<text x='420' y='140' text-anchor='middle' fill='{NET}' font-size='10'>Gossip: :9401 UDP</text>")
s.append(f"<text x='420' y='198' text-anchor='middle' fill='{MUTED}' font-size='10'>rendezvous hashing</text>")
s.append(f"<text x='420' y='212' text-anchor='middle' fill='{MUTED}' font-size='10'>failover on dead node</text>")
s.append(f"<text x='420' y='265' text-anchor='middle' fill='{NET}' font-weight='bold' font-size='11'>&lt; 1 ms RTT</text>")
s.append(f"<text x='420' y='280' text-anchor='middle' fill='{MUTED}' font-size='10'>direct socket buffers</text>")

# Right: Cluster Node Panel
s.append(panel(514, 56, 430, 260, "cluster gateway node (1 of 9 · 355 cpus total)"))
s.append(box(526, 86, 120, 38, "gossip router", "UDP broadcast", fill=AMBER_FILL, stroke=AMBER))
s.append(arrow(646, 105, 676, 105))
s.append(box(676, 86, 134, 38, "worktree mirror", "<repo>--wt-<hash>", fill="white", stroke=INK))
s.append(arrow(810, 105, 836, 105))
s.append(box(836, 86, 94, 38, "metrics", "Prometheus", fill=AMBER_FILL, stroke=AMBER))

s.append(arrow(743, 124, 600, 150))
s.append(arrow(743, 124, 743, 150))
s.append(arrow(743, 124, 870, 150))

s.append(box(526, 150, 140, 52, "primary Salsa DB", "rust-analyzer in-proc\nshared base engine", fill=RING_FILL, stroke=RING))
s.append(box(676, 150, 134, 52, "shadow engine", "in-memory /dev/shm\nisolated RAM-disk", fill=OK_FILL, stroke=OK))
s.append(box(820, 150, 110, 52, "remote exec", "cargo / go / pytest", fill="white", stroke=INK))

s.append(f"<text x='596' y='218' text-anchor='middle' fill='{MUTED}' font-size='10'>answers all queries</text>")
s.append(f"<text x='743' y='218' text-anchor='middle' fill='{OK}' font-size='10'>0 cache poison</text>")
s.append(f"<text x='875' y='218' text-anchor='middle' fill='{MUTED}' font-size='10'>warm target/</text>")

s.append(box(526, 234, 404, 38, "fleet topology: 128c Altra + 192c Zen4/5 + 35c M-series", "Swift PM routed to macOS · polyglot on Linux · 30m evict · 7d prune", fill="white", stroke=GRID, bold=False, size=10))

# Bottom Stat Callouts & Paragraph
y = 345
for x, val, label in [
    (28, "0.76 s", "cold sync (10.8 MB)"),
    (240, "< 1 ms", "LAN round trip latency"),
    (460, "355 cpus", "across 9 cluster nodes"),
    (680, "0% CPU", "developer laptop offload"),
]:
    s.append(f"<text x='{x}' y='{y}' fill='{INK}' font-weight='bold' font-size='14'>{esc(val)}</text>")
    s.append(f"<text x='{x}' y='{y + 16}' fill='{MUTED}' font-size='11'>{esc(label)}</text>")

s.append(para(28, 388, "Workspaces synchronize over 10 GbE LAN using incremental Base64 diffs rather than slow network mounts (NFS/SSHFS). Each git worktree attaches an isolated session overlay (<repo>--wt-<hash>) to a shared base compiler database (SharedWorkspace::with_base) with zero cross-session cache contamination. Edits are pre-checked on a dedicated RAM-disk shadow engine before disk writes. The cluster gossip mesh auto-routes requests to the quietest capable node, steering Apple Swift workloads to macOS Apple Silicon nodes and polyglot builds to high-throughput 128-core Linux nodes.", 125))

write("architecture-overview.svg", s)

# ==============================================================================
# 2. Dual-Engine Shadow Validation Pipeline
# ==============================================================================
W, H = 960, 420
s = head(W, H, "Dual-engine shadow validation: speculative edits tested without cache poisoning",
         "Dry-run proposals evaluated in isolated RAM-disk workspace · Primary Salsa DB stays clean and warm")

# Panels
s.append(panel(16, 56, 260, 200, "speculative edit proposals"))
s.append(panel(336, 56, 370, 200, "gateway workspace engines"))
s.append(panel(766, 56, 178, 200, "outcome & writeback"))

# Left boxes
s.append(box(30, 90, 232, 38, "multi-file proposal", "code_validate_edits", fill="white", stroke=INK))
s.append(box(30, 136, 232, 42, "shadow-run hypotheses", "3+ candidate bug fixes in RAM", fill=PURPLE_FILL, stroke=PURPLE))
s.append(box(30, 186, 232, 38, "unlinked file check", "fails validation on unlinked", fill="white", stroke=GRID, size=10))

# Middle boxes
s.append(box(350, 86, 342, 60, "primary Salsa DB (rust-analyzer)", "serves all references, callers, definitions, slices\nWARM: unaffected by speculative proposals", fill=RING_FILL, stroke=RING))
s.append(f"<text x='521' y='160' text-anchor='middle' fill='{RING}' font-weight='bold' font-size='10'>ISOLATED: ZERO CRATE RE-INFER DELAY (saves 21s)</text>")
s.append(box(350, 172, 342, 66, "shadow engine (in-memory /dev/shm)", "applies proposed overlay in RAM disk · runs diagnostics\nchecks JSON, Markdown, SVG · no disk writes", fill=OK_FILL, stroke=OK))

# Right boxes
s.append(box(780, 90, 150, 48, "clean query", "instant sub-ms RTT", fill="white", stroke=INK))
s.append(box(780, 150, 150, 48, "winner selected", "highest pass rate", fill=OK_FILL, stroke=OK))
s.append(box(780, 206, 150, 36, "write local diff", "files applied safely", fill="white", stroke=INK))

# Inter-panel arrows (drawn on top of panels)
s.append(arrow(276, 110, 336, 110, "proposal", NET, "n", size=10))
s.append(arrow(276, 156, 336, 156, "hypotheses", NET, "n", size=10))
s.append(arrow(706, 116, 766, 116, "query", INK, "a", size=10))
s.append(arrow(706, 205, 766, 205, "winner", OK, "g", size=10))

# Stat callouts
y = 280
for x, val, label, sublabel in [
    (28, "21 seconds", "saved per query", "no crate re-infer"),
    (260, "0.4 seconds", "per candidate fix", "in RAM disk"),
    (500, "3+ hypotheses", "evaluated in RAM", "concurrently"),
    (740, "Refusal gate", "blocks regressions", "force can override"),
]:
    s.append(f"<text x='{x}' y='{y}' fill='{INK}' font-weight='bold' font-size='14'>{esc(val)}</text>")
    s.append(f"<text x='{x}' y='{y + 16}' fill='{MUTED}' font-size='11'>{esc(label)}</text>")
    s.append(f"<text x='{x}' y='{y + 30}' fill='{MUTED}' font-size='10'>{esc(sublabel)}</text>")

s.append(para(28, 340, "Validating proposed file edits directly inside a shared language server invalidates warm compiler caches, triggering expensive crate re-inference (over 20s in large repositories). prod-code isolates speculative edits in dedicated RAM-disk shadow engines. Edits that introduce diagnostics or syntax errors are refused in memory without touching disk or primary state, while callers retain the option to force-apply.", 125))

write("shadow-validation-pipeline.svg", s)

# ==============================================================================
# 3. Cluster Topology & Language Routing
# ==============================================================================
W, H = 960, 460
s = head(W, H, "Production cluster: 9 nodes, 355 cores, language-aware routing",
         "Decentralized UDP gossip (:9401) · Automatic node placement · Specialized Darwin routing")

# Panels
s.append(panel(16, 56, 250, 250, "client dispatch"))
s.append(panel(310, 56, 634, 250, "9-node cluster fleet (355 CPU cores total)"))

# Left boxes
s.append(box(30, 90, 222, 44, "client request", "code_check / test / def / slice", fill="white", stroke=INK))
s.append(box(30, 146, 222, 54, "rendezvous placement", "highest-random-weight ordering\nremembers placement locally", fill=PURPLE_FILL, stroke=PURPLE))
s.append(box(30, 212, 222, 44, "nested path router", "e.g. path: 'swift/Package.swift'\nroutes to Swift-capable node", fill=NET_FILL, stroke=NET))

# Center boxes
s.append(box(324, 86, 290, 50, "128c Linux aarch64 (Ampere Altra)", "Heavy Polyglot: Rust, Go, C++, Python\nload: 0.05/cpu · 10GbE LAN", fill=RING_FILL, stroke=RING))
s.append(box(624, 86, 306, 50, "192c Linux x86_64 (Zen 5 & Zen 4)", "6 Nodes × 32 Cores · NVMe storage & RAM\nHigh-throughput compilation & parallel tests", fill=NET_FILL, stroke=NET))
s.append(box(324, 146, 290, 50, "35c macOS Apple Silicon (M-series)", "Mac Studio (24c) + MacBook Pro (11c)\nDedicated: Swift, sourcekit-lsp, xcodebuild", fill=PURPLE_FILL, stroke=PURPLE))
s.append(box(624, 146, 306, 50, "decentralized UDP gossip (:9401)", "heartbeats every 5s · load, memory, status\nzero master SPOF · instant failover", fill=AMBER_FILL, stroke=AMBER))
s.append(box(324, 206, 606, 44, "observability: Prometheus (:9401/metrics) · Pushgateway (:9091) · VictoriaMetrics & Grafana", "Events logged to events-YYYY-MM-DD.jsonl · p50/p95 latency audit via `prod-code metrics`", fill="white", stroke=GRID, bold=False, size=10))

s.append(f"<text x='627' y='278' text-anchor='middle' fill='{MUTED}' font-size='10'>Sequential cluster binary rollout via scripts/cluster-rollout.py</text>")

# Arrow drawn on top of panels
s.append(arrow(266, 173, 310, 173, "RPC", NET, "n", size=10))

y = 335
for x, val, label in [
    (28, "128 cores", "Ampere Altra aarch64"),
    (250, "192 cores", "AMD Zen 4/5 x86_64"),
    (480, "35 cores", "Apple Silicon Darwin"),
    (710, "5 seconds", "decentralized gossip interval"),
]:
    s.append(f"<text x='{x}' y='{y}' fill='{INK}' font-weight='bold' font-size='14'>{esc(val)}</text>")
    s.append(f"<text x='{x}' y='{y + 16}' fill='{MUTED}' font-size='11'>{esc(label)}</text>")

s.append(para(28, 380, "Gateways broadcast UDP status packets every 5 seconds. Workspaces are assigned using rendezvous ordering (highest-random-weight) with stickiness so warm compilation caches stay resident. If a request targets a nested SwiftPM package inside a Rust repo, the placement engine automatically reroutes that specific execution to an Apple Silicon macOS node without disrupting primary workspace placement.", 125))

write("cluster-topology.svg", s)

print("Generated docs/img and assets architecture figures successfully in ringfire style.")
