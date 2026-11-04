#!/usr/bin/env python3
"""Assemble la boite a outils des jeux natifs : ce que le launcher telecharge pour generer un module chez le joueur.

  python make_toolchain.py --gcc <racine mingw64> --python <dossier python> --capstone <dossier du paquet capstone> --out x.zip
  python3 make_toolchain.py --platform macos-arm64|macos-x86_64 --python <cpython-...-install_only.tar.gz ou dossier>
                            --capstone <capstone-5.0.7-...whl ou dossier du paquet> --out x.zip

Contenu : traducteur, runtime nitro-pc, recettes, Python et Capstone ; sous Windows aussi GCC (seulement ce qui compile
en 32 bits sans bibliotheques). --common : seulement GCC, Python et Capstone, la boite commune des jeux qui ont leur
propre runtime (release runtime-v<n> du depot du jeu, ROADMAP-MODS.md etape 3 bis). Pas une instruction de jeu. GCC et binutils sont sous GPL : voir NOTICE.txt dans l'archive.
macOS (native/MACOS.md 3.8) : pas de compilateur, le SDK d'Apple ne se redistribue pas ; le joueur a les Command Line
Tools. Python est python-build-standalone (install_only), Capstone la roue de la meme plateforme.
"""
import argparse
import os
import shutil
import stat
import sys
import tarfile
import tempfile
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
NOTICE_MACOS = """Boite a outils des jeux natifs de Nds Game Studio, macOS.

Pas de compilateur ici : le jeu est compile chez vous avec clang, des Xcode Command Line Tools (xcode-select --install).
python/ : CPython (https://www.python.org), licence PSF, dans la distribution python-build-standalone
(https://github.com/astral-sh/python-build-standalone, MPL 2.0), qui embarque OpenSSL (Apache 2.0), SQLite (domaine public),
zlib, bzip2, xz, libffi, expat, mpdecimal, ncurses et libedit sous leurs licences d'origine (python/lib/python3.13/LICENSE.txt
et https://github.com/astral-sh/python-build-standalone/tree/main/docs).
python/capstone/ : Capstone (https://www.capstone-engine.org), licence BSD, texte dans licenses/capstone-LICENSE.txt.
"""
# Ce que Python n'a pas besoin d'emporter : essais, interfaces graphiques, aides, en-tetes, pip, et les copies de bin/python3
# (le launcher lance python/bin/python3 ; 18 Mo chacune). Comparaison par composants de chemin.
PYTHON_SKIP = {"__pycache__", "test", "tests", "idlelib", "tkinter", "turtledemo", "ensurepip", "pydoc_data", "include", "share",
               "site-packages", "pkgconfig"}
PYTHON_SKIP_PREFIX = ("config-", "libtcl", "libtk", "libitcl", "tcl", "tk8", "tk9", "itcl", "thread3", "idle", "pydoc3", "python3-config", "python3.13-config", "_tkinter", "_test", "_ctypes_test", "_xxtestfuzz", "xxlimited", "xxsubtype")


def keep_python(rel):
    parts = rel.split("/")
    if parts[0] == "bin":
        return rel == "bin/python3"
    return not any(p in PYTHON_SKIP or p.startswith(PYTHON_SKIP_PREFIX) for p in parts)


def add_file(z, source, name):
    """Un fichier ordinaire (liens suivis), avec ses droits : sans le bit d'execution, python3 ne se lancerait pas."""
    source = Path(source).resolve()
    info = zipfile.ZipInfo.from_file(source, name)
    info.external_attr = (stat.S_IFREG | (0o755 if os.access(source, os.X_OK) else 0o644)) << 16
    info.compress_type = zipfile.ZIP_DEFLATED
    with open(source, "rb") as f:
        z.writestr(info, f.read(), compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def unpack(source, tmp, kind):
    """Dossier tel quel, ou archive (.tar.gz, .whl/.zip) decompressee dans tmp ; renvoie le dossier."""
    source = Path(source)
    if source.is_dir():
        return source
    out = Path(tmp) / kind
    if tarfile.is_tarfile(source):
        with tarfile.open(source) as t:
            try:
                t.extractall(out, filter="data")
            except TypeError:  # Python avant le parametre filter
                t.extractall(out)
    else:
        with zipfile.ZipFile(source) as w:
            w.extractall(out)
    return out


def macos(a):
    tmp = tempfile.mkdtemp()
    try:
        py = unpack(a.python, tmp, "python")
        py = py / "python" if (py / "python" / "bin").is_dir() else py
        cap = unpack(a.capstone, tmp, "capstone")
        wheel = cap / "capstone"
        if not wheel.is_dir():  # un dossier du paquet lui-meme
            cap, wheel = cap.parent, cap
        if not (py / "bin" / "python3").exists() or not any(wheel.glob("lib/libcapstone*.dylib")):
            sys.exit("python (bin/python3) ou capstone (lib/libcapstone.dylib) introuvable : %s, %s" % (a.python, a.capstone))
        with zipfile.ZipFile(a.out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            z.writestr("NOTICE.txt", NOTICE_MACOS)
            if not a.common:
                add_sources(z)
            for f in sorted(py.rglob("*")):
                rel = f.relative_to(py).as_posix()
                if f.is_file() and keep_python(rel):
                    add_file(z, f, "python/" + rel)
            for f in sorted(wheel.rglob("*")):
                rel = f.relative_to(wheel).as_posix()
                if f.is_file() and "__pycache__" not in rel and not rel.startswith("include/"):
                    add_file(z, f, "python/capstone/" + rel)
            licence = next(iter(cap.glob("capstone-*.dist-info/LICENSE*")), None)
            if licence:
                add_file(z, licence, "licenses/capstone-LICENSE.txt")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def add_sources(z):
    """Traducteur, runtime et recettes : pareil sur toutes les plateformes."""
    for name in ["arm2c.py", "native_build.py", "recipe.py"]:
        z.write(HERE / name, "native/tools/" + name)
    for f in sorted((NATIVE / "nitro-pc").iterdir()):
        z.write(f, "native/nitro-pc/" + f.name)
    z.write(NATIVE / "games" / "patches.json", "native/games/patches.json")
    for f in sorted((NATIVE / "games").glob("*.json.gz")):
        z.write(f, "native/games/" + f.name, zipfile.ZIP_STORED)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", choices=["windows", "macos-arm64", "macos-x86_64"], default="windows")
    ap.add_argument("--gcc", help="racine de mingw64 (Windows)")
    ap.add_argument("--python", required=True)
    ap.add_argument("--capstone", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--common", action="store_true", help="sans traducteur, runtime ni recettes")
    a = ap.parse_args()
    if a.platform != "windows":
        macos(a)
        print(a.out, Path(a.out).stat().st_size // (1 << 20), "Mo")
        return
    if not a.gcc:
        ap.error("--gcc est obligatoire sous Windows")
    gcc = Path(a.gcc)
    with zipfile.ZipFile(a.out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.writestr("NOTICE.txt", NOTICE)
        if not a.common:
            add_sources(z)
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
