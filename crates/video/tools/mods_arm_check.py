"""Fait tourner le vrai decodeur Mobiclip d'un jeu (son code ARM, image et son) sous Unicorn, et compare au decodeur Rust.

Meme raison que vx_arm_check.py : aucun jeu a videos MODS ne demarre en natif, et le decodeur Rust seul ne voit pas ce que le code
du jeu fait hors de l'image, ni sa place en memoire.

    cargo run -p nds-video --release --example vdump -- x.mods r.yuv r.f32
    python -I mods_arm_check.py <jeu> <dossier extract/<region> du projet> x.mods r.yuv r.f32 [n images]

Jeux : layton et clonewars (sans son), pvz (PCM), bluedragon (Sx). Il faut `pip install unicorn pyyaml`. Le code du jeu reste dans le projet de
decompilation, jamais dans ce depot.
"""
import struct, sys
import yaml
from unicorn import *
from unicorn.arm_const import *

# Les adresses de la classe du Mobiclip SDK dans chaque jeu : elle est la meme partout, seul son chargement change.
GAMES = {
    # Professor Layton and the Spectre's Call (Europe)
    "layton": {"overlay": "ov001.bin", "open": 0x2050dc8, "read": 0x20514d4, "decode": 0x205160c, "skip": 0x2051858, "audio": None, "malloc": 0x2000e24},
    # Plants vs. Zombies (USA)
    "pvz": {"overlay": "ov001.bin", "open": 0x21fc9c8, "read": 0x21fd100, "decode": 0x21fd31c, "skip": 0x21fd568, "audio": 0x21fd5b8, "malloc": 0x21fed18},
    # Star Wars: The Clone Wars - Republic Heroes (USA), videos sans son, dans cw_hh_2.bundle
    "clonewars": {"overlay": None, "open": 0x20631a4, "read": 0x20638d0, "decode": 0x2063af4, "skip": 0x2063d5c, "audio": None, "malloc": 0x2082198},
    # Blue Dragon Plus (Europe)
    "bluedragon": {"overlay": "ov004.bin", "open": 0x216263c, "read": 0x2162d8c, "decode": 0x2162fb0, "skip": 0x2163204, "audio": 0x2163254, "malloc": 0x2161744},
}

game = GAMES[sys.argv[1]]
J = sys.argv[2].rstrip("/\\") + "/"
path = sys.argv[3]
data = open(path, "rb").read()
ref = open(sys.argv[4], "rb").read()
pcm = open(sys.argv[5], "rb").read()
pcm = struct.unpack("<%df" % (len(pcm) // 4), pcm)
limit = int(sys.argv[6]) if len(sys.argv) > 6 else 10**9

DONE, HOOKS, CTX, FILEOBJ, HEAP0, STACK, OUT = 0x2ffff00, 0x2ffe000, 0x2f00000, 0x2f01000, 0x2400000, 0x2e00000, 0x2f10000

uc = Uc(UC_ARCH_ARM, UC_MODE_ARM)
uc.ctl_set_cpu_model(UC_CPU_ARM_926)
for base, size in ((0x02000000, 0x01000000), (0x01ff8000, 0x8000), (0x04000000, 0x10000)):
    uc.mem_map(base, size)


def load(name, base):
    uc.mem_write(base, open(J + name, "rb").read())


load("arm9/arm9.bin", yaml.safe_load(open(J + "arm9/arm9.yaml"))["base_address"])
load("arm9/itcm.bin", yaml.safe_load(open(J + "arm9/itcm.yaml"))["base_address"])
dtcm = yaml.safe_load(open(J + "arm9/dtcm.yaml"))["base_address"]
if dtcm >= 0x02000000:
    load("arm9/dtcm.bin", dtcm)
if game["overlay"]:
    overlay = next(o for o in yaml.safe_load(open(J + "arm9_overlays/overlays.yaml"))["overlays"] if o["file_name"] == game["overlay"])
    load("arm9_overlays/" + game["overlay"], overlay["base_address"])

state = {"heap": HEAP0, "pos": 0, "biggest": 0, "blocks": []}


def ret(value=None):
    if value is not None:
        uc.reg_write(UC_ARM_REG_R0, value)
    uc.reg_write(UC_ARM_REG_PC, uc.reg_read(UC_ARM_REG_LR))


def h_malloc():
    size = uc.reg_read(UC_ARM_REG_R0)
    at = state["heap"]
    state["heap"] = (at + size + 63) & ~31
    state["blocks"].append((at, size))
    uc.mem_write(at, b"\xcd" * size)
    ret(at)


def h_seek():
    state["pos"] = uc.reg_read(UC_ARM_REG_R1)
    uc.mem_write(FILEOBJ + 8, struct.pack("<I", state["pos"]))
    ret(1)


def read_file():
    buf, n = uc.reg_read(UC_ARM_REG_R1), uc.reg_read(UC_ARM_REG_R2)
    # Le jeu ne doit jamais lire plus que le tampon reserve d'apres l'en-tete
    for at, size in state["blocks"]:
        if at <= buf < at + size and buf + n > at + size:
            state["overflow"] = (hex(buf), n, size)
    chunk = data[state["pos"] : state["pos"] + n]
    uc.mem_write(buf, chunk + b"\0" * (n - len(chunk)))
    state["pos"] += n
    uc.mem_write(FILEOBJ + 8, struct.pack("<I", state["pos"]))
    state["biggest"] = max(state["biggest"], n)


def h_read():
    read_file()
    ret(uc.reg_read(UC_ARM_REG_R2))


def h_async():
    read_file()
    ret(0)


table = {HOOKS + 8: h_seek, HOOKS + 12: h_read, HOOKS + 16: h_async, game["malloc"]: h_malloc}


def code_hook(uc, address, size, user):
    fn = table.get(address)
    if fn:
        fn()
    elif HOOKS <= address < HOOKS + 0x40:
        ret(1)


uc.hook_add(UC_HOOK_CODE, code_hook, begin=game["malloc"], end=game["malloc"] + 4)
uc.hook_add(UC_HOOK_CODE, code_hook, begin=HOOKS, end=HOOKS + 0x40)
uc.mem_write(HOOKS + 0x100, b"".join(struct.pack("<I", HOOKS + o) for o in range(0, 0x40, 4)))
uc.mem_write(HOOKS, struct.pack("<I", 0xe12fff1e) * 0x10)
uc.mem_write(FILEOBJ, struct.pack("<III", HOOKS + 0x100, 0, 0))
uc.mem_write(DONE, struct.pack("<I", 0xe12fff1e))
uc.mem_write(CTX, b"\0" * 0x200)


def call(addr, *args):
    for reg, value in zip((UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3), args):
        uc.reg_write(reg, value)
    uc.reg_write(UC_ARM_REG_SP, STACK)
    uc.reg_write(UC_ARM_REG_LR, DONE)
    try:
        uc.emu_start(addr, DONE, count=400_000_000)
    except UcError as e:
        print("PLANTAGE du decodeur ARM :", e, "pc", hex(uc.reg_read(UC_ARM_REG_PC)), "image", state.get("frame"))
        raise SystemExit(1)
    return uc.reg_read(UC_ARM_REG_R0)


u32 = lambda at: struct.unpack("<I", uc.mem_read(at, 4))[0]
if not call(game["open"], CTX, FILEOBJ, 6):
    print("open refuse le fichier")
    raise SystemExit(1)
frames, w, h, codec_nch = u32(CTX + 0xc), u32(CTX + 0x10), u32(CTX + 0x14), u32(CTX + 0x1c)
codec, nch = codec_nch & 0xffff, codec_nch >> 16
cw, ch = w // 2, h // 2
frame_bytes = w * h + 2 * cw * ch
biggest_at_open = u32(CTX + 0x24)
STRIDE = 256
bad_y = bad_c = done = 0
samples_ok = samples_all = 0
pcm_at = 0
left = {}
for i in range(min(frames, limit)):
    state["frame"] = i
    if not call(game["read"], CTX):
        print("la lecture du paquet rend 0 a", i)
        break
    decoder = u32(CTX + 0x34)
    packet = u32(decoder)
    call(game["decode"], CTX)
    y, uv = u32(decoder + 0xc), u32(decoder + 0x24)
    want = ref[i * frame_bytes : i * frame_bytes + frame_bytes]
    luma = uc.mem_read(y, STRIDE * h)
    luma = b"".join(bytes(luma[r * STRIDE : r * STRIDE + w]) for r in range(h))
    if luma != want[: w * h]:
        bad_y += 1
        if bad_y <= 3:
            first = next(k for k in range(w * h) if luma[k] != want[k])
            print(f"  image {i} : premier ecart luminance x={first % w} y={first // w} jeu {luma[first]} rust {want[first]}")
    # Chrominance : lignes de 256 octets, la seconde couleur a +0x80 ; le premier plan lu est le plan 2 de Rust
    rows = uc.mem_read(uv, STRIDE * ch)
    first_plane = b"".join(bytes(rows[r * STRIDE : r * STRIDE + cw]) for r in range(ch))
    second_plane = b"".join(bytes(rows[r * STRIDE + 0x80 : r * STRIDE + 0x80 + cw]) for r in range(ch))
    if second_plane + first_plane != want[w * h :]:
        bad_c += 1
        if bad_c <= 3:
            mine = second_plane + first_plane
            first = next(k for k in range(len(mine)) if k >= len(want) - w * h or mine[k] != want[w * h + k])
            print(f"  image {i} : premier ecart couleur a {first} (plan de {cw * ch})")
    # Son : un canal par appel, tant que le jeu en demande. 128 echantillons pour Sx, 256 pour les autres
    calls = 0
    per_call = 128 if codec == 1 else 256
    while nch and game["audio"]:
        uc.mem_write(OUT, b"\0" * 2048)
        if not call(game["audio"], CTX, OUT):
            break
        got = struct.unpack("<%dh" % per_call, uc.mem_read(OUT, per_call * 2))
        c, period = calls % nch, calls // nch
        for n in range(per_call):
            at = pcm_at + (period * per_call + n) * nch + c
            if at < len(pcm):
                samples_all += 1
                expected = int(max(-1.0, min(1.0, pcm[at])) * 32768)
                if abs(got[n] - expected) <= 1:
                    samples_ok += 1
                elif state.setdefault("badsound", 0) < 5:
                    state["badsound"] += 1
                    print(f"  son image {i} appel {calls} ech {n} : jeu {got[n]} rust {expected}")
        calls += 1
    pcm_at += calls * per_call
    # Le paquet entier doit avoir ete consomme, ni plus ni moins que ce que dit sa taille
    index = u32(CTX + 0x90)
    size = u32(CTX + 0x78 + 4 * index)
    consumed = u32(decoder) - packet
    left[size - consumed] = left.get(size - consumed, 0) + 1
    call(game["skip"], CTX)
    done += 1
name = path.replace(chr(92), "/").rsplit("/", 1)[-1]
common = sorted(left.items(), key=lambda kv: -kv[1])[:4]
print(f"{name} : {done}/{frames} images par le code ARM du jeu ; luminance fausse {bad_y}, chrominance fausse {bad_c} ; son {samples_ok}/{samples_all} ech. egaux ; "
      f"octets laisses par paquet {common} ; plus grande lecture {state['biggest']} (entete max {biggest_at_open}), depassement tampon {state.get('overflow')}")
