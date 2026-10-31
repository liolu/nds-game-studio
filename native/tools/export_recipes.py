#!/usr/bin/env python3
"""Ecrit native/games/<code>.json.gz et index.json depuis les projets de jeux.

  python export_recipes.py <dossier des projets> <dossier des roms>

Les jeux essayes jusqu'en jeu sont dans PLAYABLE ; les autres restent proposes comme experimentaux.
"""
import json
import re
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import native_build
import recipe

GAMES = HERE.parent / "games"
PLAYABLE = {"AJTP", "AR5E", "AL7E"}
# A augmenter quand le runtime, le traducteur ou les recettes changent : le launcher retelecharge et regenere.
TOOLCHAIN = 1


def main():
    projects, rom_dir = Path(sys.argv[1]), Path(sys.argv[2])
    GAMES.mkdir(exist_ok=True)
    roms = {}
    for rom in sorted(rom_dir.glob("*.nds")):
        with open(rom, "rb") as f:
            roms.setdefault(f.read(16)[12:16].decode("ascii", "replace"), rom)
    index = {}
    for project in sorted(projects.iterdir()):
        cfg = next(iter(sorted(project.glob("config/*/arm9/config.yaml"))), None)
        if cfg is None:
            continue
        version = cfg.parts[-3]
        header = project / "extract" / version / "header.yaml"
        if not header.exists():
            continue
        code = re.search(r"gamecode: (\w+)", header.read_text(errors="replace")).group(1)
        if code in index:
            continue
        rom = next(iter(sorted(project.glob("*.nds"))), None) or roms.get(code)
        if rom is None:
            print(code, project.name, "pas de rom : pas de recette")
            continue
        try:
            game = native_build.Game(project, version)
            recipe.export(game, code, recipe.rom_sha1(rom), GAMES / (code + ".json.gz"))
        except Exception as e:
            print(code, project.name, "echec :", e)
            continue
        index[code] = {"status": "ok" if code in PLAYABLE else "experimental"}
        print(code, project.name, (GAMES / (code + ".json.gz")).stat().st_size // 1024, "Ko")
    (GAMES / "index.json").write_text(json.dumps({"toolchain": TOOLCHAIN, "games": index}, indent=1, sort_keys=True) + "\n", encoding="utf8", newline="\n")


if __name__ == "__main__":
    main()
