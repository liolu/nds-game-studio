#!/usr/bin/env python3
"""Assemble la boite a outils des jeux natifs : ce que le launcher telecharge pour generer un module chez le joueur.

  python make_toolchain.py --gcc <racine mingw64> --python <dossier python> --capstone <dossier du paquet capstone> --out x.zip

Contenu : traducteur, runtime nitro-pc, recettes, GCC (seulement ce qui compile en 32 bits sans bibliotheques) et Python.
Pas une instruction de jeu. GCC et binutils sont sous GPL : voir NOTICE.txt dans l'archive.
"""
import argparse
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
NATIVE = HERE.parent
DLLS = ["libgmp-10.dll", "libiconv-2.dll", "libisl-23.dll", "libmpc-3.dll", "libmpfr-6.dll", "zlib1.dll", "libzstd.dll", "libwinpthread-1.dll", "libintl-8.dll", "libgcc_s_seh-1.dll"]
TOOLS = ["gcc.exe", "as.exe", "ld.exe", "dlltool.exe", "nm.exe"]
NOTICE = """Boite a outils des jeux natifs de Nds Game Studio.

mingw64/ : GCC et GNU binutils (WinLibs, https://winlibs.com), licence GPL version 3 ; sources :
https://gcc.gnu.org et https://www.gnu.org/software/binutils
python/ : Python (https://www.python.org), licence PSF, et Capstone (https://www.capstone-engine.org), licence BSD.
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gcc", required=True)
    ap.add_argument("--python", required=True)
    ap.add_argument("--capstone", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    gcc = Path(a.gcc)
    with zipfile.ZipFile(a.out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.writestr("NOTICE.txt", NOTICE)
        for name in ["arm2c.py", "native_build.py", "recipe.py"]:
            z.write(HERE / name, "native/tools/" + name)
        for f in sorted((NATIVE / "nitro-pc").iterdir()):
            z.write(f, "native/nitro-pc/" + f.name)
        for f in sorted((NATIVE / "games").glob("*.json.gz")):
            z.write(f, "native/games/" + f.name, zipfile.ZIP_STORED)
        for name in TOOLS + DLLS:
            z.write(gcc / "bin" / name, "mingw64/bin/" + name)
        version = next((gcc / "libexec/gcc/x86_64-w64-mingw32").iterdir()).name
        cc1 = gcc / "libexec/gcc/x86_64-w64-mingw32" / version / "cc1.exe"
        z.write(cc1, "mingw64/libexec/gcc/x86_64-w64-mingw32/%s/cc1.exe" % version)
        for f in sorted((gcc / "x86_64-w64-mingw32/lib/ldscripts").glob("i386pe.*")):
            z.write(f, "mingw64/x86_64-w64-mingw32/lib/ldscripts/" + f.name)
        root = Path(a.python)
        for f in sorted(root.rglob("*")):
            rel = f.relative_to(root).as_posix()
            if f.is_file() and not any(part in rel for part in ("__pycache__", "/test/", "/tests/", "site-packages/", "idlelib/", "tkinter/", "turtledemo/", "ensurepip/", "pydoc_data/", "Scripts/", "include/", "tcl/", "Doc/")):
                z.write(f, "python/" + rel)
        cap = Path(a.capstone)
        for f in sorted(cap.rglob("*")):
            rel = f.relative_to(cap).as_posix()
            if f.is_file() and "__pycache__" not in rel and not rel.startswith("include/"):
                z.write(f, "python/capstone/" + rel)
    print(a.out, Path(a.out).stat().st_size // (1 << 20), "Mo")


if __name__ == "__main__":
    main()
