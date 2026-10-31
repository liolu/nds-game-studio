#!/usr/bin/env python3
"""Compile un jeu DS decompile en programme natif Windows (32 bits) lie au runtime nitro-pc.

  python native_build.py <dossier du projet de jeu> [--version eu] [--out dossier] [--lift f1,f2] [-j 4]

C du jeu (src/ sauf src/data) -> gcc -m32. Le reste (assembleur, fonctions non matchees, fichiers que gcc refuse)
-> arm2c.py. Les donnees ne sont pas compilees : elles viennent de la rom de l'utilisateur, chargee aux adresses DS,
et chaque symbole de donnees devient un symbole absolu. Rien de la rom n'entre dans l'exe.
"""
import argparse
import hashlib
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import arm2c
import recipe

HERE = Path(__file__).resolve().parent
RUNTIME = HERE.parent / "nitro-pc"
WINLIBS = r"C:\Users\thomr\AppData\Local\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin"
# La boite a outils du launcher donne son compilateur ; sinon celui installe sur ce PC
TOOLCHAIN_BIN = HERE.parent.parent / "mingw64" / "bin"
for bin_dir in (str(TOOLCHAIN_BIN), WINLIBS):
    if os.path.isdir(bin_dir):
        os.environ["PATH"] = bin_dir + os.pathsep + os.environ["PATH"]
        break
IDLE = 0x4000 | 0x08000000  # priorite basse, le PC reste utilisable ; pas de console quand le launcher nous lance

GAME_FLAGS = ["-m32", "-std=gnu89", "-O1", "-w", "-fwrapv", "-fno-strict-aliasing", "-fno-delete-null-pointer-checks",
              "-fno-common", "-fno-builtin", "-fno-stack-protector", "-mno-stack-arg-probe", "-mno-ms-bitfields",
              "-msse2", "-mfpmath=sse", "-mno-fp-ret-in-387", "-fno-omit-frame-pointer", "-fno-ident", "-g1"]
RUNTIME_FLAGS = ["-m32", "-std=gnu11", "-O2", "-Wall", "-Wno-unused", "-fno-builtin", "-fno-stack-protector",
                 "-mno-stack-arg-probe", "-msse2", "-mfpmath=sse", "-mno-fp-ret-in-387", "-fno-strict-aliasing",
                 "-fno-ident", "-g1"]
KR_DECL = re.compile(r"^[A-Za-z_][\w \t*]*\b\w+\s*\(\s*\)\s*;", re.M)
LIFT_FLAGS = ["-m32", "-std=gnu89", "-O1", "-w", "-fwrapv", "-fno-strict-aliasing", "-fno-builtin",
              "-fno-stack-protector", "-mno-stack-arg-probe", "-msse2", "-g1"]
SYM = re.compile(r"^(\S+) kind:(\w+)(?:\(([^)]*)\))? addr:0x([0-9a-f]+)")


def run(cmd, **kw):
    return subprocess.run([str(c) for c in cmd], capture_output=True, text=True, errors="replace", creationflags=IDLE, **kw)


class Game:
    def __init__(self, root, version):
        self.root = Path(root)
        self.version = version
        cfg = self.root / "config" / version / "arm9"
        self.functions = {}  # addr -> (nom, taille, thumb)
        self.data = {}  # nom -> addr
        self.relocs = {}
        self.units = []  # (fichier source, [(start, end)])
        modules = [cfg] + [p for p in (cfg / "itcm", cfg / "dtcm") if p.exists()]
        modules += sorted(p for p in (cfg / "overlays").glob("*") if p.is_dir()) if (cfg / "overlays").exists() else []
        for m in modules:
            for line in (m / "symbols.txt").read_text(errors="replace").splitlines():
                s = SYM.match(line)
                if not s:
                    continue
                name, kind, detail, addr = s.group(1), s.group(2), s.group(3) or "", int(s.group(4), 16)
                if kind == "function":
                    size = re.search(r"size=0x([0-9a-f]+)", detail)
                    if addr not in self.functions:
                        self.functions[addr] = (name, int(size.group(1), 16) if size else 0, "thumb" in detail)
                elif kind in ("data", "bss"):
                    self.data.setdefault(name, addr)
            for line in (m / "relocs.txt").read_text(errors="replace").splitlines():
                r = re.match(r"from:0x([0-9a-f]+) kind:(\w+) to:0x([0-9a-f]+)", line)
                if r:
                    self.relocs[int(r.group(1), 16)] = (r.group(2), int(r.group(3), 16))
            self.read_delinks(m / "delinks.txt")
            lcf = m / "lcf_symbols.txt"
            if lcf.exists():
                for name, value in re.findall(r"^(\w+) = (0x[0-9a-fA-F]+);", lcf.read_text(), re.M):
                    self.data[name] = int(value, 16)
        self.image = {}
        ext = self.root / "extract" / version / "arm9"
        for name in ("arm9", "itcm", "dtcm"):
            meta = (ext / (name + ".yaml")).read_text()
            base = int(re.search(r"base_address: (\d+)", meta).group(1))
            self.image[name] = (base, (ext / (name + ".bin")).read_bytes())
        meta = (ext / "arm9.yaml").read_text()
        self.entry = int(re.search(r"entry_function: (\d+)", meta).group(1))
        self.build_info = int(re.search(r"build_info: (\d+)", meta).group(1))

    def read_delinks(self, path):
        current = None
        for line in path.read_text(errors="replace").splitlines():
            if line.endswith(":") and not line.startswith(" "):
                current = [line[:-1], []]
                self.units.append(current)
            elif current and re.match(r"\s+\.(text|init|itcm)\b", line):
                m = re.search(r"start:0x([0-9a-f]+) end:0x([0-9a-f]+)", line)
                if m:
                    current[1].append((int(m.group(1), 16), int(m.group(2), 16)))
            elif not current:
                m = re.match(r"\s+\.ctor\s+start:0x([0-9a-f]+) end:0x([0-9a-f]+)", line)
                if m:
                    self.data["ARM9_CTOR_START"] = int(m.group(1), 16)
                    self.data["ARM9_CTOR_END"] = int(m.group(2), 16)

    def memory(self, addr, n):
        for base, raw in self.image.values():
            if base <= addr < base + len(raw):
                return raw[addr - base : addr - base + n]
        return None


def has_asm(text):
    return re.search(r"^\s*(asm|__asm)\b|\basm\s+\w", text, re.M) is not None


def obj_for(out, src):
    return out / "obj" / (hashlib.sha1(str(src).encode()).hexdigest()[:10] + "_" + src.stem + ".o")


def compile_one(args):
    src, obj, flags = args
    if obj.exists() and obj.stat().st_mtime >= src.stat().st_mtime:
        return src, True, ""
    p = run(["gcc", *flags, "-c", src, "-o", obj])
    return src, p.returncode == 0, p.stderr


def nm_defined(objs, jobs):
    def one(o):
        res = run(["nm", "--defined-only", "-g", o]).stdout
        return o, [(l.split()[1], l.split()[2]) for l in res.splitlines() if len(l.split()) == 3]
    with ThreadPoolExecutor(jobs) as e:
        return dict(e.map(one, objs))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("project", nargs="?", help="dossier du projet de jeu ; sans projet : --recipe et --rom")
    ap.add_argument("--recipe", help="recette du jeu (native/games/<code>.json.gz)")
    ap.add_argument("--rom", help="rom du joueur, avec --recipe")
    ap.add_argument("--version", default=None)
    ap.add_argument("--out", default=None)
    ap.add_argument("--lift", default="", help="fonctions a traduire depuis l'ARM meme si elles ont du C (pour trouver un bug)")
    ap.add_argument("-j", type=int, default=4)
    ap.add_argument("--with-c", action="store_true", help="compiler aussi le C decompile du jeu (registres par exceptions, plus lent)")
    ap.add_argument("--instrument", action="store_true", help="trace les entrees de fonctions (dernier appels au plantage)")
    a = ap.parse_args()
    if a.recipe:
        game = recipe.RecipeGame(a.recipe, a.rom)
        root, code = Path("."), game.code
    else:
        root = Path(a.project).resolve()
        version = a.version or next(p.name for p in (root / "config").iterdir() if (p / "arm9").exists())
        code = re.search(r"gamecode: (\w+)", (root / "extract" / version / "header.yaml").read_text()).group(1)
    out = Path(a.out) if a.out else HERE.parent / "build" / (code + ("-c" if a.with_c else "") + ("-debug" if a.instrument else ""))
    if a.instrument:
        skip = "-finstrument-functions-exclude-function-list=LDRW,ROR,LSL,LSR,ASR,CLZ,SAT32"
        GAME_FLAGS.extend(["-finstrument-functions", skip])
        LIFT_FLAGS.extend(["-finstrument-functions", skip, "-DNDS_INSTRUMENT"])
        RUNTIME_FLAGS.append("-DNDS_INSTRUMENT")
    (out / "obj").mkdir(parents=True, exist_ok=True)
    (out / "lift").mkdir(exist_ok=True)
    print("jeu", code, "->", out, flush=True)
    if not a.recipe:
        game = Game(root, version)
    force = set(filter(None, a.lift.split(",")))

    # 1. runtime : ce qu'il definit remplace le jeu
    rt_srcs = sorted(RUNTIME.glob("*.c"))
    rt_flags = RUNTIME_FLAGS + ["-I", RUNTIME]
    with ThreadPoolExecutor(a.j) as e:
        res = list(e.map(compile_one, [(s, obj_for(out, s), rt_flags) for s in rt_srcs]))
    fails = [r for r in res if not r[1]]
    for r in fails:
        print(r[2][:3000])
    if fails:
        sys.exit("runtime : erreur de compilation")
    rt_objs = [obj_for(out, s) for s in rt_srcs]
    rt_defs = {s[1:] for syms in nm_defined(rt_objs, a.j).values() for t, s in syms if t in "TDBR"}

    # 2. C du jeu
    by_addr = sorted(game.functions)
    names = {addr: game.functions[addr][0] for addr in by_addr}
    srcs = []
    file_funcs = {}
    kr_files = 0
    for unit, ranges in game.units:
        path = root / unit
        if not unit.startswith("src/") or unit.startswith("src/data/") or not path.exists():
            continue
        funcs = [f for f in by_addr if any(s <= f < e for s, e in ranges)]
        file_funcs[path] = funcs
        text = path.read_text(errors="replace")
        if not a.with_c or has_asm(text):
            continue
        # declaration sans prototype : le C matche compte sur les registres r0-r3 deja en place (f() sans arguments),
        # ce que le x86 ne garde pas ; la traduction de l'ARM reste exacte
        if KR_DECL.search(text):
            kr_files += 1
            continue
        if any(names[f] in rt_defs or names[f] in force for f in funcs):
            continue
        srcs.append(path)
    game_flags = GAME_FLAGS + ["-I", root / "include", "-I", root / "include" / "nitro"]
    with ThreadPoolExecutor(a.j) as e:
        res = list(e.map(compile_one, [(s, obj_for(out, s), game_flags) for s in srcs]))
    good = [r[0] for r in res if r[1]]
    bad = [r for r in res if not r[1]]
    # un fichier qui garde ses propres variables : les vraies sont dans la rom, aux adresses DS, et d'autres fonctions
    # ou l'ARM7 peuvent y acceder par leur adresse. On le traduit pour qu'il utilise celles de la rom.
    def own_data(src):
        res = run(["nm", "--defined-only", obj_for(out, src)]).stdout
        for line in res.splitlines():
            f = line.split()
            if len(f) == 3 and f[1] in "dDbBC" and not f[2].startswith("."):
                return True
        return False
    with ThreadPoolExecutor(a.j) as e:
        flags = list(e.map(own_data, good))
    data_files = sum(flags)
    good = [g for g, o in zip(good, flags) if not o]
    print("C compile :", len(good), " refuse :", len(bad), " sans prototypes (traduits) :", kr_files, " avec variables (traduits) :", data_files)

    # 3. fonctions a traduire : tout ce que le C ne couvre pas
    covered = {f for s in good for f in file_funcs[s]}
    to_lift = [f for f in by_addr if f not in covered and names[f] not in rt_defs]
    used = {}
    symbols = {}
    for f in by_addr:
        n = names[f]
        symbols[f] = n
    code_ptr = {frm for frm, (kind, to) in game.relocs.items() if kind == "load" and (to & ~1) in symbols}
    lifter = arm2c.Lifter(game.memory, symbols, None, code_ptr)
    lifter.debug_pc = a.instrument
    lifter.sizes = {addr: game.functions[addr][1] for addr in by_addr}
    # plusieurs passes : les fonctions qui renvoient des drapeaux se decouvrent chez leurs appelants
    for _ in range(4):
        before = len(lifter.flag_funcs) + len(lifter.gosub_funcs)
        lifter.unhandled = []
        done = set()
        queue = [(f, names[f], game.functions[f][2]) for f in to_lift]
        protos = set()
        bodies = []
        while queue:
            addr, name, thumb = queue.pop()
            if addr in done:
                continue
            done.add(addr)
            bodies.append(lifter.lift(name, addr, thumb))
            for sub_addr, sub_name in list(lifter.new_subs.items()):
                if sub_addr not in done:
                    queue.append((sub_addr, sub_name, thumb))
        if len(lifter.flag_funcs) + len(lifter.gosub_funcs) == before:
            break
    print("fonctions a retour par drapeaux :", " ".join(sorted(lifter.flag_funcs))[:600])
    all_calls = set(re.findall(r"CALLF\((\w+)\)|\(u32\)&(\w+)", "\n".join(bodies)))
    for pair in all_calls:
        n = pair[0] or pair[1]
        if n and not n.startswith("nds_"):
            protos.add(n)
    lift_c = arm2c.PRELUDE + "\n".join("extern u64 %s();" % p for p in sorted(protos)) + "\n\n" + "\n".join(bodies)
    lift_path = out / "lift" / "lifted.c"
    if not lift_path.exists() or lift_path.read_text() != lift_c:
        lift_path.write_text(lift_c)
    print("fonctions traduites depuis l'ARM :", len(done), " instructions non gerees :", len(lifter.unhandled))
    (out / "unhandled.txt").write_text("\n".join(lifter.unhandled))
    # un seul gros fichier serait lent et fragile : gcc -O1 tient bien 2 Mo de C simple
    lobj = out / "obj" / "lifted.o"
    if not lobj.exists() or lobj.stat().st_mtime < lift_path.stat().st_mtime:
        p = run(["gcc", *LIFT_FLAGS, "-c", lift_path, "-o", lobj])
        if p.returncode:
            print(p.stderr[:5000])
            sys.exit("lifted.c : erreur")

    # 4. tables : pointeurs de fonctions dans les donnees, adresse ARM -> x86, symboles absolus
    game_objs = [obj_for(out, s) for s in good]
    defs = nm_defined(game_objs + [lobj], a.j)
    defined = {s[1:]: t for syms in defs.values() for t, s in syms}
    defined.update({s: "T" for s in rt_defs})
    fn_names = {n for n in names.values() if n in defined}
    table = sorted((addr, n) for addr, n in list(names.items()) + [(k, v) for k, v in lifter.new_subs.items()] if n in defined or n in lifter.new_subs.values())
    fix = sorted((frm, to & ~1) for frm, (kind, to) in game.relocs.items() if kind == "load" and (to & ~1) in dict(table))
    gen = ["typedef unsigned int u32;"]
    tname = dict(table)
    for _, n in table:
        gen.append("extern char %s[];" % n)
    gen.append("const struct { u32 arm; void *x86; } nds_fn_table[] = {")
    gen += ["  {0x%08x, %s}," % (addr, n) for addr, n in table]
    gen.append("}; const unsigned nds_fn_count = %d;" % len(table))
    gen.append("const struct { u32 at; void *x86; } nds_fixups[] = {")
    gen += ["  {0x%08x, %s}," % (frm, tname[to]) for frm, to in fix]
    gen.append("}; const unsigned nds_fixup_count = %d;" % len(fix))
    gen.append("extern char %s[]; void *const nds_entry_fn = %s;" % (names[game.entry], names[game.entry]))
    gen.append("const u32 nds_build_info_offset = 0x%x;" % game.build_info)
    gen.append("const int nds_has_c = %d;" % (1 if good else 0))
    gen_path = out / "lift" / "tables.c"
    gen_path.write_text("\n".join(gen) + "\n")
    gobj = out / "obj" / "tables.o"
    p = run(["gcc", "-m32", "-c", "-w", "-std=gnu89", gen_path, "-o", gobj])
    if p.returncode:
        print(p.stderr[:3000])
        sys.exit("tables.c : erreur")

    # 5. donnees : symboles absolus ; une copie definie par le C devient faible
    data_defs = {}
    for o, syms in defs.items():
        for t, s in syms:
            if t in "DBRC" and s[1:] in game.data:
                data_defs.setdefault(o, []).append(s)
    for o, syms in data_defs.items():
        run(["objcopy", *sum((["-W", s] for s in syms), []), o])
    print("donnees definies par le C et rendues faibles :", sum(len(v) for v in data_defs.values()))
    abs_lines = []
    for n, addr in sorted(game.data.items()):
        if n in rt_defs:
            continue
        abs_lines += [".globl _%s" % n, ".set _%s, 0x%08x" % (n, addr)]
    abs_path = out / "lift" / "dsabs.s"
    abs_path.write_text("\n".join(abs_lines) + "\n")
    aobj = out / "obj" / "dsabs.o"
    p = run(["as", "--32", abs_path, "-o", aobj])
    if p.returncode:
        print(p.stderr[:3000])
        sys.exit("dsabs : erreur")

    # 6. edition de liens, avec bouchons pour les symboles introuvables
    k32 = out / "libkernel32.a"
    run(["dlltool", "-m", "i386", "--as-flags=--32", "-k", "-d", RUNTIME / "kernel32.def", "-l", k32])
    w32 = out / "libwinmm.a"
    run(["dlltool", "-m", "i386", "--as-flags=--32", "-k", "-d", RUNTIME / "winmm.def", "-l", w32])
    exe = out / (code + ".exe")
    objs = rt_objs + game_objs + [lobj, gobj, aobj]
    rsp = out / "link.rsp"
    def link(extra):
        rsp.write_text("\n".join('"%s"' % str(o).replace("\\", "/") for o in objs + extra))
        return run(["ld", "-m", "i386pe", "-e", "_nds_start", "--stack", "0x40000","--image-base", "0x10000000",
                    "--subsystem", "console", "--disable-dynamicbase", "--disable-reloc-section",
                    "@" + str(rsp), k32, w32, "-o", exe])
    p = link([])
    missing = sorted(set(re.findall(r"undefined reference to `([^']+)'", p.stderr)))
    if missing:
        stub = out / "lift" / "missing.c"
        lines = ["void nds_missing(const char *);"]
        for m in missing:
            if m.endswith(("@4", "@8", "@12", "@16", "@20", "@24", "@28")):
                continue
            lines.append('unsigned long long %s(void) { nds_missing("%s"); return 0; }' % (m, m))
        stub.write_text("\n".join(lines) + "\n")
        sobj = out / "obj" / "missing.o"
        run(["gcc", "-m32", "-w", "-std=gnu89", "-O1", "-fno-builtin", "-c", stub, "-o", sobj])
        print("symboles sans definition (bouchons) :", len(missing), " ".join(missing[:60]))
        p = link([sobj])
    if p.returncode:
        print(p.stderr[:6000])
        sys.exit("edition de liens : erreur")
    print("OK", exe, exe.stat().st_size // 1024, "Ko")


if __name__ == "__main__":
    main()
