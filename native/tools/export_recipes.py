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
TOOLCHAIN = 3


def game_state(project, version, game):
    """Etat de la decompilation : code et donnees d'apres le rapport du projet, C d'apres ses sources.

    code = part du code identique a l'original ; c = part du code ecrite en C sans assembleur."""
    state = {}
    report = project / "build" / version / "report.json"
    if report.exists():
        m = json.loads(report.read_text(encoding="utf8", errors="replace")).get("measures", {})
        state["code"] = round(float(m.get("matched_code_percent", 0) or 0), 2)
        state["data"] = round(float(m.get("matched_data_percent", 0) or 0), 2)
    total = sum(size for _, size, _ in game.functions.values()) or 1
    by_addr = sorted(game.functions)
    in_c = 0
    for unit, ranges in game.units:
        path = project / unit
        if not unit.startswith("src/") or unit.startswith("src/data/") or not path.exists():
            continue
        if native_build.has_asm(path.read_text(errors="replace")):
            continue
        in_c += sum(game.functions[f][1] for f in by_addr if any(s <= f < e for s, e in ranges))
    state["c"] = round(100.0 * in_c / total, 2)
    return state


def playability(code, runs):
    """Ce qu'a donne l'essai du jeu : jouable, plante, bloque ou demarre (native/GAMES.md)."""
    if code in PLAYABLE:
        return "playable"
    run = runs.get(code, "")
    if run.startswith("plante"):
        return "crash"
    if "affiche" in run and "rien" not in run:
        return "starts"
    return "stuck" if run else "untested"


def main():
    projects, rom_dir = Path(sys.argv[1]), Path(sys.argv[2])
    GAMES.mkdir(exist_ok=True)
    roms = {}
    for rom in sorted(rom_dir.glob("*.nds")):
        with open(rom, "rb") as f:
            roms.setdefault(f.read(16)[12:16].decode("ascii", "replace"), rom)
    runs = {}
    games_md = HERE.parent / "GAMES.md"
    if games_md.exists():
        for line in games_md.read_text(encoding="utf8").splitlines():
            cells = [c.strip() for c in line.split("|")]
            if len(cells) > 5 and len(cells[2]) == 4:
                runs[cells[2]] = cells[5]
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
        index[code] = {"status": "ok" if code in PLAYABLE else "experimental", "play": playability(code, runs), **game_state(project, version, game)}
        print(code, project.name, (GAMES / (code + ".json.gz")).stat().st_size // 1024, "Ko")
    (GAMES / "index.json").write_text(json.dumps({"toolchain": TOOLCHAIN, "games": index}, indent=1, sort_keys=True) + "\n", encoding="utf8", newline="\n")


if __name__ == "__main__":
    main()
