#!/usr/bin/env python3
"""Le runtime existe en une copie par depot de jeu (ROADMAP-MODS.md, etape 3 bis) : montre l'ecart entre celui du studio et
celui d'un jeu, et le reporte si on le demande. Rien n'est jamais impose : chaque jeu choisit quand il suit.

  python native/tools/runtime_sync.py <dossier du jeu> <code>                 # l'ecart, fichier par fichier
  python native/tools/runtime_sync.py <dossier du jeu> <code> --apply         # copie ce qui differe (rien n'est efface)
  python native/tools/runtime_sync.py <dossier du jeu> <code> --apply --bump  # et passe native/VERSION a la suivante

Le dossier du jeu est son depot, ou son dossier dans un depot qui porte plusieurs jeux (liolu/All-nds-decomp-games :
games/<L>/<jeu>). Ensuite : commit, puis l'etiquette, runtime-v<VERSION> dans le depot d'un seul jeu ou
<jeu>/runtime-v<VERSION> dans celui de plusieurs jeux (<jeu> = nom du dossier) ; la CI du depot publie l'archive. Puis la
version va dans native/games/index.json du studio ("runtime": {"repo": ..., "version": n}, plus "tag": "<jeu>/runtime-v<n>"
pour un depot de plusieurs jeux ; le script affiche l'entree) : pousse sur experimental, seul index.json est publie
(native-index.yml, apres avoir verifie que la release existe), sans nouveau launcher ni boite a outils.
Le jeu passe par son verrou build/pipeline.lock s'il en a un : le Studio de decompilation y travaille.
"""
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
NATIVE = HERE.parent
REPO = NATIVE.parent
TOOLS = ["arm2c.py", "native_build.py", "recipe.py", "runtime_package.py"]
TESTS = ["arm2c_test.py", "arm2c_cases.s", "harness.c", "runtime_check.py"]


def wanted(code, own_repo=True):
    """(chemin dans le dossier du jeu, contenu voulu). Dans un depot de plusieurs jeux (own_repo faux), pas de CI par jeu :
    celle du depot (.github/workflows/native-runtime.yml) publie chaque etiquette <jeu>/runtime-v<n>."""
    out = [("native/tools/" + n, (HERE / n).read_bytes()) for n in TOOLS]
    out += [("native/tests/" + n, (NATIVE / "tests" / n).read_bytes()) for n in TESTS]
    out += [("native/nitro-pc/" + f.name, f.read_bytes()) for f in sorted((NATIVE / "nitro-pc").iterdir()) if f.is_file()]
    out += [("native/sdk/" + f.name, f.read_bytes()) for f in sorted((NATIVE / "sdk").iterdir()) if f.is_file()]
    out.append(("native/games/%s.json.gz" % code, (NATIVE / "games" / (code + ".json.gz")).read_bytes()))
    patches = json.loads((NATIVE / "games" / "patches.json").read_text(encoding="utf8"))
    out.append(("native/games/patches.json", (json.dumps({code: patches[code]} if code in patches else {}, indent=1, sort_keys=True) + "\n").encode()))
    out.append(("native/LICENSE", (NATIVE / "RUNTIME-LICENSE.txt").read_bytes()))
    out.append(("native/README.md", (NATIVE / "RUNTIME-README.md").read_bytes().replace(b"@CODE@", code.encode())))
    if own_repo:
        out.append((".github/workflows/native-runtime.yml", (HERE / "runtime-ci.yml").read_bytes().replace(b"@CODE@", code.encode())))
    return out


def git_root(folder):
    """Le depot git qui contient ce dossier : lui-meme (un jeu par depot) ou un parent (plusieurs jeux par depot)."""
    return next((p for p in [folder, *folder.parents] if (p / ".git").exists()), None)


def github_repo(root):
    """proprietaire/depot d'apres le remote origin, None sinon."""
    r = subprocess.run(["git", "-C", str(root), "remote", "get-url", "origin"], capture_output=True, text=True)
    m = re.search(r"github\.com[:/]([^/\s]+/[^/\s]+?)(?:\.git)?/?$", r.stdout.strip()) if r.returncode == 0 else None
    return m.group(1) if m else None


def same(a, b):
    """Les fins de ligne ne comptent pas : git les change selon la machine."""
    return a.replace(b"\r\n", b"\n") == b.replace(b"\r\n", b"\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("game_repo")
    ap.add_argument("code")
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--bump", action="store_true", help="avec --apply : native/VERSION + 1")
    a = ap.parse_args()
    game = Path(a.game_repo).resolve()
    root = git_root(game)
    if root is None:
        sys.exit("%s n'est pas dans un depot git" % game)
    own_repo = root == game
    if not own_repo and not (game / "config").is_dir():
        sys.exit("%s n'est pas le dossier d'un jeu (pas de config/) : donner games/<L>/<jeu>" % game)
    lock = game / "build" / "pipeline.lock"
    if a.apply and lock.exists():
        sys.exit("%s existe : le Studio travaille dans ce depot, recommencer quand il a fini" % lock)
    changed = 0
    for rel, data in wanted(a.code, own_repo):
        target = game / rel
        state = "nouveau" if not target.exists() else ("identique" if same(target.read_bytes(), data) else "change")
        if state == "identique":
            continue
        changed += 1
        print("%-9s %s" % (state, rel))
        if a.apply:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
    extra = sorted(p.relative_to(game).as_posix() for p in (game / "native").rglob("*") if p.is_file()) if (game / "native").exists() else []
    known = {rel for rel, _ in wanted(a.code, own_repo)} | {"native/VERSION"}
    for rel in extra:
        if rel not in known:
            print("%-9s %s (seulement dans le jeu, garde)" % ("en plus", rel))
    version_file = game / "native" / "VERSION"
    version = int(version_file.read_text().strip()) if version_file.exists() else 0
    if a.apply and (a.bump or not version_file.exists()):
        version += 1
        version_file.write_text("%d\n" % version, encoding="ascii", newline="\n")
        print("native/VERSION :", version)
    print("%d fichier(s) %s ; runtime du jeu : version %d" % (changed, "reportes" if a.apply else "differents", version))
    if version:
        # ce qui publie cette version, puis l'entree de native/games/index.json qui l'annonce
        tag = "runtime-v%d" % version if own_repo else "%s/runtime-v%d" % (game.name, version)
        entry = {"repo": github_repo(root) or "<proprietaire/depot>", "version": version}
        if not own_repo:
            entry["tag"] = tag
        print("etiquette :", tag)
        print('index.json : "runtime": %s' % json.dumps(entry))


if __name__ == "__main__":
    main()
