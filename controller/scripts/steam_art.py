#!/usr/bin/env python3
"""Install Steam Library artwork for the "Steam Deck Remote Joy" Non-Steam Game (runs on the Deck).

  steam_art.py ART_DIR [--name "Steam Deck Remote Joy"] [--restart-steam]

ART_DIR holds controller/assets/steam/*.png. For each Steam user with that shortcut, copies them
into userdata/<user>/config/grid/ under the shortcut's appid:
  capsule_portrait -> <appid>p.png, capsule_landscape -> <appid>.png, hero -> <appid>_hero.png,
  logo -> <appid>_logo.png, icon -> <appid>_icon.png
and points the shortcut's icon at <appid>_icon.png. Steam keeps shortcuts.vdf in memory and
rewrites it, so the icon is only set while Steam isn't running; the grid images work either way
(Steam shows them after a restart, or when the Library refreshes). --restart-steam shuts Steam down
and writes the icon as soon as it exits; in Game Mode, the session starts Steam again by itself,
and that Steam loads the new icon and artwork. Standard library only.
"""
import argparse
import pathlib
import re
import shutil
import struct
import subprocess
import time

GRID_NAMES = {
    "capsule_portrait.png": "{appid}p.png",
    "capsule_landscape.png": "{appid}.png",
    "hero.png": "{appid}_hero.png",
    "logo.png": "{appid}_logo.png",
    "icon.png": "{appid}_icon.png",
}


def shortcuts(vdf: bytes):
    """(appid, name, entry_start, entry_end) for each shortcut in a binary shortcuts.vdf."""
    starts = [m.start() for m in re.finditer(rb"\x02appid\x00", vdf, re.I)]
    for i, s in enumerate(starts):
        end = starts[i + 1] if i + 1 < len(starts) else len(vdf)
        appid = struct.unpack("<I", vdf[s + 7 : s + 11])[0]
        name = re.search(rb"\x01appname\x00([^\x00]*)\x00", vdf[s:end], re.I)
        yield appid, (name.group(1).decode(errors="replace") if name else ""), s, end


def set_icon(vdf: bytes, start: int, end: int, icon: str) -> bytes:
    """Replace the icon string of the entry in [start, end). Binary VDF strings aren't length
    prefixed, so changing the length in place is fine."""
    entry = vdf[start:end]
    new, n = re.subn(rb"(\x01icon\x00)[^\x00]*(\x00)", lambda m: m.group(1) + icon.encode() + m.group(2), entry,
                     count=1, flags=re.I)
    if n == 0:
        raise SystemExit("shortcut has no icon field")
    return vdf[:start] + new + vdf[end:]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("art_dir", type=pathlib.Path)
    ap.add_argument("--name", default="Steam Deck Remote Joy")
    ap.add_argument("--restart-steam", action="store_true", help="shut Steam down to set the icon")
    args = ap.parse_args()
    steam_running = subprocess.run(["pgrep", "-x", "steam"], capture_output=True).returncode == 0
    if steam_running and args.restart_steam:
        subprocess.Popen(["steam", "-shutdown"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 30
        # Game Mode restarts Steam within moments, so poll fast and write as soon as it's gone.
        while subprocess.run(["pgrep", "-x", "steam"], capture_output=True).returncode == 0:
            if time.monotonic() > deadline:
                raise SystemExit("Steam didn't exit within 30 s")
            time.sleep(0.05)
        steam_running = False
    found = False
    for vdf_path in pathlib.Path.home().glob(".local/share/Steam/userdata/*/config/shortcuts.vdf"):
        vdf = vdf_path.read_bytes()
        for appid, name, start, end in shortcuts(vdf):
            if name != args.name:
                continue
            found = True
            grid = vdf_path.parent / "grid"
            grid.mkdir(exist_ok=True)
            for src, dst in GRID_NAMES.items():
                shutil.copyfile(args.art_dir / src, grid / dst.format(appid=appid))
            icon = str(grid / f"{appid}_icon.png")
            note = "grid images installed"
            if steam_running:
                note += "; icon not set (close Steam and run again)"
            else:
                vdf = set_icon(vdf, start, end, icon)
                vdf_path.write_bytes(vdf)
                note += ", icon set"
            print(f"{vdf_path.parent.parent.name}: {name!r} appid {appid}: {note}")
    if not found:
        print(f"no Non-Steam Game named {args.name!r} (add it first: ADD_TO_STEAM=1 scripts/deploy_deck.sh)")


if __name__ == "__main__":
    main()
