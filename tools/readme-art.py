"""Generates the Parsley README artwork: animated cover + how-it-works strip, light and dark."""
import os, re
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

# Point GEIST_FONTS at the dist/fonts folder of the `geist` npm package.
F = os.environ.get("GEIST_FONTS", "node_modules/geist/dist/fonts/")
ROOT = Path(__file__).resolve().parent.parent
FONTS = {
    "pixel": TTFont(F + "geist-pixel/GeistPixel-Square.woff2"),
    "medium": TTFont(F + "geist-sans/Geist-Medium.ttf"),
    "semibold": TTFont(F + "geist-sans/Geist-SemiBold.ttf"),
    "regular": TTFont(F + "geist-sans/Geist-Regular.ttf"),
    "mono": TTFont(F + "geist-mono/GeistMono-Medium.ttf"),
}
OUT = str(ROOT / "docs")
os.makedirs(OUT, exist_ok=True)

def text_path(s, font, size, x, y, fill, spacing=0.0, anchor="start", opacity=None, extra=""):
    f = FONTS[font]
    gs = f.getGlyphSet(); cmap = f.getBestCmap(); upm = f["head"].unitsPerEm; hm = f["hmtx"]
    sc = size / upm
    width = sum(hm[cmap.get(ord(c), cmap[32])][0] * sc + spacing for c in s) - spacing
    if anchor == "end": x -= width
    if anchor == "middle": x -= width / 2
    pen = SVGPathPen(gs)
    cx = x
    for c in s:
        g = cmap.get(ord(c), cmap[32])
        tp = TransformPen(pen, (sc, 0, 0, -sc, cx, y))
        gs[g].draw(tp)
        cx += hm[g][0] * sc + spacing
    op = f' opacity="{opacity}"' if opacity is not None else ""
    return f'<path d="{pen.getCommands()}" fill="{fill}"{op}{extra}/>', width

# ---- pixel sprig (logo) ----
PIX = """
.....GG..GG.....
....GGGGGGGG....
...GGGGggGGGG...
....GGGggGGG....
.GG...GGggG..GG.
GGGG...GSG..GGGG
GGgGG...S..GGgGG
.GGgGG..S.GGgGG.
..GGGgS.S.gGGG..
...GGGSSSSGG....
.......SS.......
.......S........
......SS........
......S.........
.....SS.........
................
""".strip("\n").split("\n")
PC = {"G": "#38a04a", "g": "#8fd47f", "S": "#2b7d39"}
def pix(x, y, cell):
    r = []
    for j, row in enumerate(PIX):
        for i, ch in enumerate(row):
            if ch in PC:
                r.append(f'<rect x="{x+i*cell}" y="{y+j*cell}" width="{cell}" height="{cell}" fill="{PC[ch]}"/>')
    return f'<g shape-rendering="crispEdges">{"".join(r)}</g>'

BUNCH = (ROOT / "ui" / "bunch.svg").read_text(encoding="utf-8")
BUNCH_INNER = re.search(r"<svg[^>]*>([\s\S]*)</svg>", BUNCH).group(1)

THEMES = {
    "dark": dict(bg="#09080c", ink="#f5f5f5", grid="#ffffff", g1=.05, g2=.09, muted="#f5f5f5", mo=.72, card="#141318", line="#ffffff", lo=.14, shadow="#000000", so=.55),
    "light": dict(bg="#f5f5f5", ink="#121212", grid="#121212", g1=.05, g2=.09, muted="#121212", mo=.72, card="#ffffff", line="#121212", lo=.14, shadow="#121212", so=.18),
}
LVL = {"crit": "#e5484d", "err": "#f07a3a", "warn": "#e6b422"}

def grid(w, h, t, step=32):
    lines = []
    for x in range(0, w + 1, step):
        o = t["g2"] if x % (step * 4) == 0 else t["g1"]
        lines.append(f'<line x1="{x}" y1="0" x2="{x}" y2="{h}" stroke="{t["grid"]}" stroke-opacity="{o}"/>')
    for y in range(0, h + 1, step):
        o = t["g2"] if y % (step * 4) == 0 else t["g1"]
        lines.append(f'<line x1="0" y1="{y}" x2="{w}" y2="{y}" stroke="{t["grid"]}" stroke-opacity="{o}"/>')
    return f'<g shape-rendering="crispEdges">{"".join(lines)}</g>'

HOP_CSS = """
@keyframes hop{0%,100%{transform:translateY(0) scale(1,1)}12%{transform:translateY(0) scale(1.08,.9)}45%{transform:translateY(-38px) scale(.94,1.07)}72%{transform:translateY(0) scale(1.1,.88)}86%{transform:translateY(0) scale(.97,1.03)}}
@keyframes shad{0%,100%,72%{transform:scaleX(1);opacity:1}45%{transform:scaleX(.58);opacity:.45}}
.hop{animation:hop 1.2s cubic-bezier(.16,1,.3,1) infinite;transform-box:fill-box;transform-origin:50% 100%}
.shad{animation:shad 1.2s cubic-bezier(.16,1,.3,1) infinite;transform-box:fill-box;transform-origin:50% 50%}
@media (prefers-reduced-motion: reduce){*{animation:none!important}}
"""

def cover(name, t):
    W, H = 1280, 520
    parts = [f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>', grid(W, H, t)]
    # eyebrow
    p, _ = text_path("OPEN SOURCE  ·  WINDOWS  ·  MIT", "mono", 13, 96, 92, t["muted"], spacing=1.6, opacity=t["mo"])
    parts.append(f'<rect x="64" y="82" width="20" height="3" fill="#e0792f"/>')
    parts.append(p)
    # logo + wordmark
    parts.append(pix(64, 128, 8))
    wm, wmw = text_path("Parsley", "pixel", 150, 212, 246, t["ink"])
    parts.append(wm)
    # tagline
    a, _ = text_path("See what Windows recorded", "medium", 34, 66, 318, t["ink"], spacing=-0.6)
    b, _ = text_path("around a crash or restart.", "medium", 34, 66, 360, t["ink"], spacing=-0.6, opacity=.62)
    parts += [a, b]
    # rewind strip
    x0, x1, y = 66, 740, 452
    strip = [f'<line x1="{x0}" y1="{y}" x2="{x1}" y2="{y}" stroke="{t["line"]}" stroke-opacity=".35"/>']
    for i in range(0, 31):
        xx = x0 + (x1 - x0) * i / 30
        hh = 10 if i % 5 == 0 else 5
        strip.append(f'<line x1="{xx:.1f}" y1="{y}" x2="{xx:.1f}" y2="{y-hh}" stroke="{t["line"]}" stroke-opacity=".3"/>')
    evs = [(0.237, "warn"), (0.461, "err"), (0.501, "crit"), (0.514, "err"), (0.563, "err"), (0.82, "warn")]
    for f_, lv in evs:
        xx = x0 + (x1 - x0) * f_
        strip.append(f'<rect x="{xx-5:.1f}" y="{y-28}" width="10" height="10" fill="{LVL[lv]}"/>')
    rx = x0 + (x1 - x0) * 0.501
    for lab, f_ in (("1:44", 0), ("2:14 pm", .501), ("2:44", 1)):
        tp, _ = text_path(lab, "mono", 12, x0 + (x1 - x0) * f_, y + 24, t["muted"], anchor="middle" if f_ == .501 else ("start" if f_ == 0 else "end"), opacity=.6)
        strip.append(tp)
    # playhead travels back from right edge to the restart, holds, label appears
    lab, lw = text_path("Unexpected restart · Kernel-Power 41", "medium", 13, 0, 0, t["bg"])
    strip.append(f'''<g class="ph"><line x1="0" y1="{y-50}" x2="0" y2="{y+6}" stroke="{t["ink"]}" stroke-width="2"/>
<g class="lab"><rect x="8" y="{y-64}" width="{lw+20:.0f}" height="24" rx="4" fill="{t["ink"]}"/><g transform="translate(18 {y-47})">{lab}</g></g></g>''')
    css = HOP_CSS + f"""
@keyframes rewind{{0%{{transform:translateX({x1}px)}}8%{{transform:translateX({x1}px)}}42%,92%{{transform:translateX({rx:.1f}px)}}100%{{transform:translateX({x1}px)}}}}
@keyframes lab{{0%,42%{{opacity:0}}50%,86%{{opacity:1}}92%,100%{{opacity:0}}}}
.ph{{animation:rewind 6s cubic-bezier(.16,1,.3,1) infinite}}
.lab{{animation:lab 6s linear infinite}}
"""
    parts.append("".join(strip))
    # stage for the bunch
    sx, sy, sw, sh = 864, 64, 352, 392
    parts.append(f'<rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="14" fill="{t["card"]}" stroke="{t["line"]}" stroke-opacity="{t["lo"]}"/>')
    sub = grid(sw, sh, t, 16).replace("<g ", '<g opacity=".7" ', 1)
    parts.append(f'<g transform="translate({sx} {sy})">{sub}</g>')
    tag, _ = text_path("FICTIONAL EXAMPLE READY", "mono", 11, sx + 22, sy + 34, t["muted"], spacing=1.4, opacity=.6)
    parts.append(tag)
    parts.append(f'<ellipse class="shad" cx="{sx+sw/2}" cy="{sy+sh-58}" rx="62" ry="7" fill="{t["shadow"]}" fill-opacity="{t["so"]}"/>')
    parts.append(f'<g class="hop"><g transform="translate({sx+sw/2-128} {sy+sh-58-252}) scale(4.27) translate(-3 -1)">{BUNCH_INNER}</g></g>')
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="Parsley. See what Windows recorded around a crash or restart."><style>{css}</style>{"".join(parts)}</svg>'
    open(f"{OUT}/{name}", "w").write(svg)

def steps(name, t):
    W, H = 1280, 400
    parts = [f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>', grid(W, H, t)]
    pw, gap, x0 = 368, 40, 64
    data = [("01", "Pick a moment", ["Latest 24 hours, a time you", "choose, or an exported .evtx log."]),
            ("02", "Follow the evidence", ["Each event is explained and", "links back to its Windows record."]),
            ("03", "Compare a normal day", ["Same length, same clock time.", "Differences are counts, not causes."])]
    css = """
@keyframes blink{0%,49%{opacity:1}50%,100%{opacity:0}}
@keyframes pick{0%,30%{opacity:0;transform:translateY(4px)}40%,90%{opacity:1;transform:none}100%{opacity:0}}
@keyframes grow{0%,10%{transform:scaleY(0)}45%,90%{transform:scaleY(1)}100%{transform:scaleY(0)}}
.blink{animation:blink 1s steps(1) infinite}
.pick{animation:pick 4s cubic-bezier(.16,1,.3,1) infinite}
.grow{animation:grow 4s cubic-bezier(.16,1,.3,1) infinite;transform-box:fill-box;transform-origin:50% 100%}
@media (prefers-reduced-motion: reduce){*{animation:none!important}}
"""
    for i, (n, title, lines) in enumerate(data):
        x = x0 + i * (pw + gap)
        y = 48
        parts.append(f'<rect x="{x}" y="{y}" width="{pw}" height="304" rx="12" fill="{t["card"]}" stroke="{t["line"]}" stroke-opacity="{t["lo"]}"/>')
        num, _ = text_path(n, "pixel", 34, x + 24, y + 52, "#38a04a")
        parts.append(num)
        # illustration area y+72..y+170
        ix, iy = x + 24, y + 78
        if i == 0:
            parts.append(f'<rect x="{ix}" y="{iy}" width="{pw-48}" height="52" rx="8" fill="none" stroke="{t["line"]}" stroke-opacity=".3"/>')
            tp, tw = text_path("2026-11-21   2:14 pm", "mono", 18, ix + 16, iy + 33, t["ink"])
            parts.append(tp)
            parts.append(f'<rect class="blink" x="{ix+18+tw:.0f}" y="{iy+15}" width="9" height="22" fill="#38a04a"/>')
            for k, lab in enumerate(["± 10 min", "± 30 min", "± 2 h"]):
                bx = ix + k * 106
                sel = k == 1
                parts.append(f'<rect x="{bx}" y="{iy+66}" width="96" height="30" rx="6" fill="{t["ink"] if sel else "none"}" stroke="{t["line"]}" stroke-opacity="{1 if sel else .3}"/>')
                lp, _ = text_path(lab, "medium", 13, bx + 48, iy + 86, t["bg"] if sel else t["ink"], anchor="middle")
                parts.append(lp)
        elif i == 1:
            ly = iy + 30
            parts.append(f'<line x1="{ix}" y1="{ly}" x2="{ix+pw-48}" y2="{ly}" stroke="{t["line"]}" stroke-opacity=".35"/>')
            for f_, lv in [(.12, "warn"), (.4, "err"), (.52, "crit"), (.6, "err"), (.85, "warn")]:
                ex = ix + (pw - 48) * f_
                ring = f'<rect x="{ex-9:.0f}" y="{ly-19}" width="18" height="18" fill="none" stroke="{t["ink"]}" stroke-width="2"/>' if lv == "crit" else ""
                parts.append(f'<rect x="{ex-5:.0f}" y="{ly-15}" width="10" height="10" fill="{LVL[lv]}"/>{ring}')
            cx = ix + (pw - 48) * .52
            parts.append(f'<g class="pick"><line x1="{cx:.0f}" y1="{ly+4}" x2="{cx:.0f}" y2="{ly+20}" stroke="{t["ink"]}" stroke-opacity=".5"/>'
                         f'<rect x="{ix}" y="{ly+20}" width="{pw-48}" height="48" rx="8" fill="{t["ink"]}"/>')
            a, _ = text_path("Kernel-Power 41 · restart", "semibold", 14, ix + 14, ly + 41, t["bg"])
            b, _ = text_path("System · record 118 231", "mono", 12, ix + 14, ly + 59, t["bg"], opacity=.7)
            parts += [a, b, "</g>"]
        else:
            base = iy + 98
            bars = [("Critical", 1, 0), ("Errors", 3, 0), ("Warnings", 5, 2)]
            mx = 5
            for k, (lab, a_, b_) in enumerate(bars):
                bx = ix + 8 + k * 110
                ha = max(4, 86 * a_ / mx) if a_ else 2
                hb = max(4, 86 * b_ / mx) if b_ else 2
                parts.append(f'<rect class="grow" x="{bx}" y="{base-ha:.0f}" width="34" height="{ha:.0f}" fill="{t["ink"]}"/>')
                parts.append(f'<rect class="grow" x="{bx+38}" y="{base-hb:.0f}" width="34" height="{hb:.0f}" fill="{t["ink"]}" fill-opacity=".28"/>')
                lp, _ = text_path(lab, "mono", 11, bx + 36, base + 18, t["muted"], anchor="middle", opacity=.65)
                parts.append(lp)
            lg1, _ = text_path("Incident", "medium", 12, ix + 18, iy + 6, t["ink"])
            lg2, _ = text_path("Day before", "medium", 12, ix + 112, iy + 6, t["ink"], opacity=.6)
            parts += [f'<rect x="{ix}" y="{iy-4}" width="10" height="10" fill="{t["ink"]}"/>', lg1,
                      f'<rect x="{ix+94}" y="{iy-4}" width="10" height="10" fill="{t["ink"]}" fill-opacity=".28"/>', lg2]
        tt, _ = text_path(title, "semibold", 26, x + 24, y + 238, t["ink"], spacing=-0.5)
        parts.append(tt)
        for k, ln in enumerate(lines):
            lp, _ = text_path(ln, "regular", 15, x + 24, y + 266 + k * 21, t["ink"], opacity=.68)
            parts.append(lp)
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="How Rewind works: pick a moment, follow the evidence, compare with a normal day."><style>{css}</style>{"".join(parts)}</svg>'
    open(f"{OUT}/{name}", "w").write(svg)

for k, t in THEMES.items():
    cover(f"cover-{k}.svg", t)
    steps(f"rewind-steps-{k}.svg", t)
print("ok")
