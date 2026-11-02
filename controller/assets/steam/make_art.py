#!/usr/bin/env python3
"""Steam Library artwork for the Non-Steam Game entry: a little TurtleBot-style robot.

Writes SVGs and renders PNGs (Inkscape) next to this script (both are committed, so deploying
needs no Inkscape; rerun this after changing the art):
  icon.png 256x256, capsule_portrait.png 600x900, capsule_landscape.png 920x430,
  hero.png 1920x620, logo.png 1280x400 (transparent).
scripts/deploy_deck.sh copies them to the Deck, and scripts/steam_art.py installs them into
Steam's grid folder for the "Steam Deck Remote Joy" shortcut.
"""
import pathlib
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE

TEAL = "#3fd6cc"
AMBER = "#ffb547"
FONT = "Noto Sans, DejaVu Sans, sans-serif"


def defs() -> str:
    return f"""
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#0d1726"/><stop offset="1" stop-color="#143049"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="{TEAL}" stop-opacity="0.28"/><stop offset="1" stop-color="{TEAL}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="baseSide" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#1f232a"/><stop offset="0.45" stop-color="#3a414b"/><stop offset="1" stop-color="#1c2026"/>
    </linearGradient>
    <linearGradient id="plateSide" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#8f99a5"/><stop offset="0.45" stop-color="#cfd6de"/><stop offset="1" stop-color="#848e9a"/>
    </linearGradient>
    <linearGradient id="lidarSide" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#14171c"/><stop offset="0.45" stop-color="#2f353e"/><stop offset="1" stop-color="#121418"/>
    </linearGradient>
    <linearGradient id="deckBody" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#2a2f38"/><stop offset="1" stop-color="#171a20"/>
    </linearGradient>
  </defs>"""


def disc(cy: float, rx: float, ry: float, h: float, side: str, top: str, extra: str = "") -> str:
    """A cylinder seen from slightly above: side band of height h, then the top ellipse."""
    return (
        f'<path d="M{-rx} {cy} L{-rx} {cy + h} A{rx} {ry} 0 0 0 {rx} {cy + h} L{rx} {cy} Z" fill="{side}"/>'
        f'<ellipse cx="0" cy="{cy}" rx="{rx}" ry="{ry}" fill="{top}"/>{extra}'
    )


def rods(y_bottom: float, y_top: float, xs, dy: float, color: str = "#a3acb7") -> str:
    out = []
    for x in xs:
        for y in (y_bottom - dy, y_bottom + dy):  # back pair, then front pair
            out.append(f'<rect x="{x - 4}" y="{y - (y_bottom - y_top)}" width="8" height="{y_bottom - y_top}" rx="3" fill="{color}"/>')
    return "".join(out)


def robot() -> str:
    """Ground contact at y=0, centred on x=0, about 320 wide and 300 tall."""
    p = []
    p.append('<ellipse cx="0" cy="30" rx="178" ry="36" fill="#000" opacity="0.38"/>')
    # wheels peeking out under the sides of the base
    for x in (-136, 136):
        p.append(f'<rect x="{x - 15}" y="-22" width="30" height="54" rx="13" fill="#0e1014"/>')
    # base: dark puck with a teal light strip and two friendly eyes
    p.append(disc(-74, 150, 50, 52, "url(#baseSide)", "#4a525e"))
    p.append(f'<path d="M-120 4 A150 50 0 0 0 120 4" fill="none" stroke="{TEAL}" stroke-width="5" stroke-linecap="round" opacity="0.9"/>')
    for x in (-30, 30):
        p.append(f'<ellipse cx="{x}" cy="-1" rx="10" ry="12" fill="{TEAL}"/>')
        p.append(f'<ellipse cx="{x - 3}" cy="-5" rx="3" ry="4" fill="#e9fffd"/>')
    # standoffs, first plate (with a little board), standoffs, second plate
    p.append(rods(-74, -150, (-78, 78), 22))
    p.append(disc(-150, 122, 40, 11, "url(#plateSide)", "#e8ecf1",
                  '<path d="M-38 -158 L22 -170 L60 -152 L0 -140 Z" fill="#1f6f62"/>'
                  f'<circle cx="34" cy="-156" r="4" fill="{AMBER}"/>'))
    p.append(rods(-150, -222, (-60, 60), 18))
    p.append(disc(-222, 104, 34, 10, "url(#plateSide)", "#e8ecf1"))
    # lidar puck with a glowing ring and a faint scan halo
    p.append(f'<ellipse cx="0" cy="-246" rx="150" ry="48" fill="none" stroke="{TEAL}" stroke-width="3" stroke-dasharray="10 12" opacity="0.35"/>')
    p.append(disc(-262, 42, 14, 26, "url(#lidarSide)", "#2c323b"))
    p.append(f'<path d="M-42 -248 A42 14 0 0 0 42 -248" fill="none" stroke="{TEAL}" stroke-width="5" stroke-linecap="round"/>')
    p.append('<ellipse cx="-10" cy="-266" rx="14" ry="4" fill="#59616d"/>')
    return "<g>" + "".join(p) + "</g>"


def handheld() -> str:
    """A generic handheld controller, centred on 0,0, about 250 x 104."""
    return f"""<g>
    <rect x="-125" y="-52" width="250" height="104" rx="44" fill="url(#deckBody)" stroke="#3a414c" stroke-width="2"/>
    <rect x="-56" y="-34" width="112" height="68" rx="8" fill="#0b1220"/>
    <rect x="-50" y="-28" width="100" height="56" rx="5" fill="#16314a"/>
    <path d="M-30 14 L-8 -6 L8 6 L30 -14" fill="none" stroke="{TEAL}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
    <circle cx="-92" cy="-18" r="15" fill="#11141a"/><circle cx="-92" cy="-18" r="10" fill="#2b313a"/>
    <circle cx="92" cy="-18" r="15" fill="#11141a"/><circle cx="92" cy="-18" r="10" fill="#2b313a"/>
    <g fill="#5b636f"><rect x="-100" y="10" width="7" height="22" rx="2"/><rect x="-107" y="17" width="22" height="7" rx="2"/></g>
    <g><circle cx="86" cy="14" r="5" fill="#5b636f"/><circle cx="98" cy="22" r="5" fill="{AMBER}"/><circle cx="86" cy="30" r="5" fill="#5b636f"/><circle cx="74" cy="22" r="5" fill="#5b636f"/></g>
  </g>"""


def signal(angle_deg: float = 0) -> str:
    """Three arcs pointing up (angle 0), rotated clockwise by angle_deg."""
    arcs = "".join(
        f'<path d="M{-r * 0.6:.1f} {-r * 0.8:.1f} A{r} {r} 0 0 1 {r * 0.6:.1f} {-r * 0.8:.1f}" fill="none" stroke="{AMBER}" '
        f'stroke-width="7" stroke-linecap="round" opacity="{o}"/>'
        for r, o in ((22, 1), (42, 0.75), (62, 0.5))
    )
    return f'<g transform="rotate({angle_deg})">{arcs}</g>'


def floor(w: int, h: int, horizon: float) -> str:
    lines = []
    for i in range(-12, 13):
        lines.append(f'<line x1="{w / 2 + i * 40}" y1="{horizon}" x2="{w / 2 + i * 260}" y2="{h}" stroke="{TEAL}" stroke-opacity="0.07" stroke-width="2"/>')
    y = horizon
    step = 14
    while y < h:
        lines.append(f'<line x1="0" y1="{y:.1f}" x2="{w}" y2="{y:.1f}" stroke="{TEAL}" stroke-opacity="0.07" stroke-width="2"/>')
        step *= 1.35
        y += step
    return "".join(lines)


def title(x: float, y: float, size: float, anchor: str = "middle", sub: bool = True) -> str:
    t = (
        f'<text x="{x}" y="{y}" font-family="{FONT}" font-weight="800" font-size="{size}" fill="#f2f6fa" '
        f'text-anchor="{anchor}" letter-spacing="{size * 0.01:.1f}">Remote <tspan fill="{TEAL}">Joy</tspan></text>'
    )
    if sub:
        t += (
            f'<text x="{x}" y="{y + size * 0.5:.0f}" font-family="{FONT}" font-weight="600" font-size="{size * 0.24:.0f}" '
            f'fill="#9fb3c6" text-anchor="{anchor}" letter-spacing="{size * 0.03:.1f}">DRIVE A ROS 2 ROBOT</text>'
        )
    return t


def svg(w: int, h: int, body: str, background: bool = True) -> str:
    bg = f'<rect width="{w}" height="{h}" fill="url(#bg)"/>' if background else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{defs()}{bg}{body}</svg>'


def glow(cx: float, cy: float, r: float) -> str:
    return f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#glow)"/>'


def place(group: str, x: float, y: float, s: float) -> str:
    return f'<g transform="translate({x} {y}) scale({s})">{group}</g>'


ART = {
    # Rounded tile (transparent outside it), whole robot inside with a margin.
    "icon": (256, 256, lambda: '<rect x="8" y="8" width="240" height="240" rx="52" fill="url(#bg)"/>'
             '<clipPath id="tile"><rect x="8" y="8" width="240" height="240" rx="52"/></clipPath>'
             '<g clip-path="url(#tile)">' + glow(128, 132, 120) + place(robot(), 128, 192, 0.55) + '</g>'),
    "capsule_portrait": (600, 900, lambda: floor(600, 900, 560) + glow(300, 560, 300)
                         + place(handheld(), 300, 790, 0.9) + place(signal(0), 300, 744, 1.0)
                         + place(robot(), 300, 610, 1.3) + title(300, 150, 92)),
    "capsule_landscape": (920, 430, lambda: floor(920, 430, 300) + glow(215, 250, 210)
                          + place(robot(), 215, 375, 0.9) + title(660, 215, 76)),
    "hero": (1920, 620, lambda: floor(1920, 620, 430) + glow(1330, 380, 340)
             + place(handheld(), 1765, 455, 1.1) + place(signal(-90), 1590, 455, 1.2)
             + place(robot(), 1290, 560, 1.45)),
    "logo": (1280, 400, lambda: title(640, 220, 170, sub=False)),
}


def main() -> None:
    for name, (w, h, body) in ART.items():
        svg_path = OUT / f"{name}.svg"
        svg_path.write_text(svg(w, h, body(), background=name not in ("logo", "icon")))
        subprocess.run(["inkscape", str(svg_path), "--export-type=png", f"--export-filename={OUT / name}.png",
                        f"--export-width={w}", f"--export-height={h}"], check=True, capture_output=True)
        print(f"{name}.png {w}x{h}")


if __name__ == "__main__":
    main()
