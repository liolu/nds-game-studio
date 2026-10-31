#!/usr/bin/env python3
"""Construit et essaie tous les projets de jeux, un a la fois, et note le resultat de chacun dans native/GAMES.md.

  python build_all.py <dossier des projets> <dossier des roms> [--secs 20] [--only a,b]

Un jeu qui ne marche pas garde son module et sa ligne : le travail n'est pas perdu. Les modules vont dans
target/release/native (jamais dans git : ils contiennent le code du jeu).
"""
import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
NATIVE = HERE.parent
INSTALL = NATIVE.parent / "target" / "release" / "native"
IDLE = 0x4000  # BELOW_NORMAL_PRIORITY_CLASS
ROM_CODE_AT = 12


def rom_index(rom_dir):
    roms = {}
    for rom in sorted(Path(rom_dir).glob("*.nds")):
        with open(rom, "rb") as f:
            roms.setdefault(f.read(16)[ROM_CODE_AT:16].decode("ascii", "replace"), rom)
    return roms


def try_game(project, roms, secs, seen):
    cfg = next(iter(sorted(project.glob("config/*/arm9/config.yaml"))), None)
    if cfg is None:
        return None
    version = cfg.parts[-3]
    header = project / "extract" / version / "header.yaml"
    if not header.exists():
        return None
    code = re.search(r"gamecode: (\w+)", header.read_text(errors="replace")).group(1)
    if code in seen:  # copies de travail d'un meme jeu : le premier projet suffit
        return None
    seen.add(code)
    overlays = len(list((cfg.parent / "overlays").glob("*"))) if (cfg.parent / "overlays").exists() else 0
    row = {"game": project.name, "code": code, "overlays": overlays, "build": "", "run": ""}
    b = subprocess.run([sys.executable, str(HERE / "native_build.py"), str(project), "-j", "2"], capture_output=True, text=True, errors="replace", creationflags=IDLE)
    exe = NATIVE / "build" / code / (code + ".exe")
    if b.returncode or not exe.exists():
        last = [l for l in (b.stdout + b.stderr).splitlines() if l.strip()][-1:] or ["?"]
        row["build"] = "echec : " + last[0][:90]
        return row
    row["build"] = "ok"
    INSTALL.mkdir(parents=True, exist_ok=True)
    shutil.copy2(exe, INSTALL / exe.name)
    rom = next(iter(sorted(project.glob("*.nds"))), None) or roms.get(code)
    if rom is None:
        row["run"] = "pas de rom"
    else:
        try:
            r = subprocess.run([str(exe), str(rom), "--mute", "--trace", "--every", "60"], capture_output=True, text=True, errors="replace", timeout=secs, creationflags=IDLE)
            log, rc = r.stderr, r.returncode
        except subprocess.TimeoutExpired as e:
            log, rc = e.stderr or "", None
            if isinstance(log, bytes):
                log = log.decode("utf8", "replace")
        frames = re.findall(r"^image (\d+) ", log, re.M)
        last_frame = frames[-1] if frames else "0"
        fatal = re.search(r"(acces memoire invalide.*|instruction non traduite.*|fonction absente.*|saut vers.*|swi .* inconnu|memoire DS.*)", log)
        vram = re.findall(r"pal=(\d+) palB=(\d+) vramBG=(\d+)", log)
        drew = bool(vram) and any(int(a) or int(b) or int(c) for a, b, c in vram)
        state = "plante : " + fatal.group(1)[:70] if fatal else ("tourne, image %s" % last_frame if rc is None else "fin %s, image %s" % (rc, last_frame))
        row["run"] = state + (", affiche" if drew else ", rien a l'ecran")
    # le module est garde ; les objets prennent trop de place
    shutil.rmtree(NATIVE / "build" / code / "obj", ignore_errors=True)
    shutil.rmtree(NATIVE / "build" / code / "lift", ignore_errors=True)
    return row


def write(rows):
    out = ["# Jeux natifs : ce que donne chaque jeu", "",
           "Relevé automatique de `native/tools/build_all.py`, sans aucune touche pressée : un essai court ne dit pas qu'un jeu est jouable.",
           "Les modules restent sur le PC (`target/release/native`), jamais dans git.", "",
           "| Jeu | Code | Overlays | Construction | Essai |", "|---|---|---|---|---|"]
    for r in sorted(rows, key=lambda r: r["game"]):
        out.append("| %s | %s | %d | %s | %s |" % (r["game"], r["code"], r["overlays"], r["build"].replace("|", "/"), r["run"].replace("|", "/")))
    (NATIVE / "GAMES.md").write_text("\n".join(out) + "\n", encoding="utf8", newline="\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("projects")
    ap.add_argument("roms")
    ap.add_argument("--secs", type=int, default=20)
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    only = set(filter(None, a.only.split(",")))
    roms = rom_index(a.roms)
    rows = []
    seen = set()
    for project in sorted(Path(a.projects).iterdir()):
        if not project.is_dir() or (only and project.name not in only):
            continue
        try:
            row = try_game(project, roms, a.secs, seen)
        except Exception as e:  # un jeu qui casse l'outil ne doit pas arreter les autres
            row = {"game": project.name, "code": "?", "overlays": 0, "build": "outil : %s" % str(e)[:80], "run": ""}
        if row:
            rows.append(row)
            print(row, flush=True)
            write(rows)


if __name__ == "__main__":
    main()
