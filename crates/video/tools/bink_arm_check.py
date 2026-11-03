"""Fait tourner le vrai decodeur Bink d'un jeu (code ARM : image et son) sous Unicorn, et compare au decodeur Rust.

Meme raison que vx_arm_check.py : aucun jeu a videos Bink ne demarre en natif. La lecture du fichier passe par un BINKIO a nous :
la bibliotheque du jeu le prevoit (BinkSetIO), ce qui evite ses fils d'execution.

    cargo run -p nds-video --release --example vdump -- x.bik r.yuv r.f32
    python -I bink_arm_check.py <jeu> <dossier extract/<region> du projet> x.bik r.yuv r.f32 [n images]

Jeux : sims2 (Europe), giants (Battle of Giants: Dinosaurs, USA), viva (Viva Pinata, USA). La bibliotheque des Sims 2 ne joue que la voie gauche d'un son
stereo, dans les deux haut-parleurs (`mov r5, r6` a la place de la saturation), et celle de Battle of Giants retarde la droite d un
echantillon : le controle compare en consequence.

Il faut `pip install unicorn`. Le code du jeu reste dans le projet de decompilation, jamais dans ce depot.
"""
import struct, sys
import yaml
from unicorn import *
from unicorn.arm_const import *

GAMES = {
    "sims2": {"overlay": None, "open": 0x20dd1ac, "do_frame": 0x20dc8d4, "next_frame": 0x20dc6e8, "set_memory": 0x20dc1f0, "io_global": 0x2142edc, "audio_open": 0x20dfa1c, "audio_decode": 0x20df90c, "left_only": True},
    # Viva Pinata (USA) : sa bibliotheque n'a pas d'allocateur a remplacer ; on prend celui du jeu et l'appel au decodeur d'images
    "viva": {"overlay": "ov010.bin", "open": 0x20e319c, "do_frame": 0x20e26dc, "next_frame": 0x20e24ec, "set_memory": None, "alloc": 0x20e49d4, "free": 0x20e4980, "video": 0x20e55e4, "io_global": 0x20f4f60, "audio_open": 0x20e4bf8, "audio_decode": 0x20e4aa8, "left_only": False},
    "giants": {"overlay": "ov002.bin", "open": 0x20e2f8c, "do_frame": 0x20e267c, "next_frame": 0x20e2480, "set_memory": 0x20e1f0c, "io_global": 0x20f8034, "audio_open": 0x20e5880, "audio_decode": 0x20e5770, "left_only": False},
}
game = GAMES[sys.argv[1]]
J = sys.argv[2].rstrip("/\\") + "/"
path = sys.argv[3]
data = open(path, "rb").read()
ref = open(sys.argv[4], "rb").read()
pcm = open(sys.argv[5], "rb").read()
pcm = struct.unpack("<%df" % (len(pcm) // 4), pcm)
limit = int(sys.argv[6]) if len(sys.argv) > 6 else 10**9

BINK_OPEN, DO_FRAME, NEXT_FRAME, SET_MEMORY, IO_GLOBAL = game["open"], game["do_frame"], game["next_frame"], game["set_memory"], game["io_global"]
AUDIO_OPEN, AUDIO_DECODE = game["audio_open"], game["audio_decode"]
OPEN_DIRECT_IO, OPEN_DS = 0x2000000, 0x8000000
DONE, HOOKS, HEAP0, STACK, SOUND_IN, SLOTS = 0x2ffff00, 0x2ffe000, 0x2400000, 0x2e00000, 0x2f00000, 0x2ffd000
(H_ALLOC, H_FREE, H_OPEN, H_HEADER, H_FRAME, H_BUFSIZE, H_INFO, H_IDLE, H_CLOSE, H_BG) = [HOOKS + 4 * i for i in range(10)]

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
state = {"heap": HEAP0, "pos": 0, "blocks": [], "audio": bytearray(), "reads": 0}


def reg(n):
    return uc.reg_read(n)


def ret(value=None):
    if value is not None:
        uc.reg_write(UC_ARM_REG_R0, value)
    uc.reg_write(UC_ARM_REG_PC, reg(UC_ARM_REG_LR))


def h_alloc():
    size = reg(UC_ARM_REG_R0)
    at = state["heap"]
    state["heap"] = (at + size + 63) & ~31
    state["blocks"].append((at, size))
    uc.mem_write(at, b"\xcd" * size)
    ret(at)


def h_open():
    # r0 : le BINKIO a remplir
    io = reg(UC_ARM_REG_R0)
    uc.mem_write(io, b"\0" * 0x144)
    uc.mem_write(io, struct.pack("<7I", H_HEADER, H_FRAME, H_BUFSIZE, H_INFO, H_IDLE, H_CLOSE, H_BG))
    ret(1)


def read_into(dest, size):
    chunk = data[state["pos"] : state["pos"] + size]
    uc.mem_write(dest, chunk + b"\0" * (size - len(chunk)))
    state["pos"] += size
    state["reads"] += 1
    return len(chunk)


def h_header():
    offset, dest, size = reg(UC_ARM_REG_R1), reg(UC_ARM_REG_R2), reg(UC_ARM_REG_R3)
    if offset != 0xffffffff:
        state["pos"] = offset
    ret(read_into(dest, size))


def h_frame():
    # (io, numero, position ou -1, destination, taille)
    position, dest = reg(UC_ARM_REG_R2), reg(UC_ARM_REG_R3)
    size = struct.unpack("<I", uc.mem_read(reg(UC_ARM_REG_SP), 4))[0]
    if position != 0xffffffff:
        state["pos"] = position
    ret(read_into(dest, size))


table = {H_ALLOC: h_alloc, H_FREE: ret, H_OPEN: h_open, H_HEADER: h_header, H_FRAME: h_frame, H_BUFSIZE: lambda: ret(0), H_INFO: ret, H_IDLE: lambda: ret(0), H_CLOSE: ret, H_BG: lambda: ret(0)}


def code_hook(uc, address, size, user):
    fn = table.get(address)
    if fn:
        fn()


uc.hook_add(UC_HOOK_CODE, code_hook, begin=HOOKS, end=HOOKS + 0x40)
if "alloc" in game:
    table[game["alloc"]] = h_alloc
    table[game["free"]] = ret
    uc.hook_add(UC_HOOK_CODE, code_hook, begin=game["alloc"], end=game["alloc"])
    uc.hook_add(UC_HOOK_CODE, code_hook, begin=game["free"], end=game["free"])
if "video" in game:
    def h_video(uc, address, size, user):
        state["buffers"] = reg(UC_ARM_REG_R0)

    uc.hook_add(UC_HOOK_CODE, h_video, begin=game["video"], end=game["video"])
uc.mem_write(HOOKS, struct.pack("<I", 0xe12fff1e) * 0x10)
uc.mem_write(DONE, struct.pack("<I", 0xe12fff1e))


def call(addr, *args):
    for r, value in zip((UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3), args):
        uc.reg_write(r, value)
    uc.reg_write(UC_ARM_REG_SP, STACK)
    # Le cinquieme argument et les suivants vont sur la pile
    uc.mem_write(STACK, b"".join(struct.pack("<I", v) for v in args[4:]))
    uc.reg_write(UC_ARM_REG_LR, DONE)
    try:
        uc.emu_start(addr, DONE, count=800_000_000)
    except UcError as e:
        print("PLANTAGE du decodeur ARM :", e, "pc", hex(reg(UC_ARM_REG_PC)), "lr", hex(reg(UC_ARM_REG_LR)), "image", state.get("frame"))
        raise SystemExit(1)
    return reg(UC_ARM_REG_R0)


u32 = lambda at: struct.unpack("<I", uc.mem_read(at, 4))[0]
if SET_MEMORY:
    call(SET_MEMORY, H_ALLOC, H_FREE)
uc.mem_write(IO_GLOBAL, struct.pack("<I", H_OPEN))
bink = call(BINK_OPEN, 0, OPEN_DIRECT_IO | OPEN_DS)
if not bink:
    print("BinkOpen refuse le fichier")
    raise SystemExit(1)
w, h, frames = u32(bink), u32(bink + 4), u32(bink + 8)
cw, ch = (w + 1) // 2, (h + 1) // 2
frame_bytes = w * h + 2 * cw * ch
buffers = u32(bink + 0xb8) if "video" not in game else None
bad_y = bad_c = done = 0
for i in range(min(frames, limit)):
    state["frame"] = i
    call(DO_FRAME, bink)
    buffers = buffers or state["buffers"]
    index = u32(buffers + 0x14)
    base = buffers + 0x18 + index * 0x30
    want = ref[i * frame_bytes : i * frame_bytes + frame_bytes]

    def plane(at, pw, ph):
        buf, pitch = u32(at + 4), u32(at + 8)
        rows = uc.mem_read(buf, pitch * ph)
        return b"".join(bytes(rows[r * pitch : r * pitch + pw]) for r in range(ph))

    luma = plane(base, w, h)
    if luma != want[: w * h]:
        bad_y += 1
        if bad_y <= 3:
            first = next(k for k in range(w * h) if luma[k] != want[k])
            print(f"  image {i} : premier ecart luminance x={first % w} y={first // w} jeu {luma[first]} rust {want[first]}")
    # Rust range d'abord le plan cB (1), puis cR (2)
    mine = plane(base + 24, cw, ch) + plane(base + 12, cw, ch)
    if mine != want[w * h :]:
        bad_c += 1
        if bad_c <= 3:
            first = next(k for k in range(len(mine)) if mine[k] != want[w * h + k])
            print(f"  image {i} : premier ecart couleur a {first} (plan de {cw * ch}) jeu {mine[first]} rust {want[w * h + first]}")
    call(NEXT_FRAME, bink)
    done += 1

# Le son : le jeu passe par le systeme de son de la DS, on appelle directement son decodeur, paquet par paquet, comme BinkDoFrame
le32 = lambda at: struct.unpack("<I", data[at : at + 4])[0]
tracks = le32(0x28)
got = []
channels = 1
if tracks and limit >= frames:
    rate_flags = le32(0x2c + 4 * tracks)
    rate, channels = rate_flags & 0xffff, 2 if rate_flags >> 29 & 1 else 1
    table_at = 0x2c + 12 * tracks
    audio = call(AUDIO_OPEN, rate, channels, 1)
    for f in range(frames):
        at = le32(table_at + f * 4) & ~1
        size = le32(at)
        packet = data[at + 4 : at + 4 + size]
        if size < 4:
            continue
        uc.mem_write(SOUND_IN, packet)
        wanted = le32(at + 4)
        source = SOUND_IN + 4
        while wanted > 0:
            call(AUDIO_DECODE, audio, SLOTS, SLOTS + 4, source, SLOTS + 8, SOUND_IN + len(packet))
            where, count, source = u32(SLOTS), u32(SLOTS + 4), u32(SLOTS + 8)
            count = min(count, wanted)
            got.extend(struct.unpack("<%dh" % (count // 2), uc.mem_read(where, count)))
            wanted -= count
compared = min(len(got), len(pcm))
worst = 0
total = 0.0
signal = 0.0
hop = (2048 if rate >= 44100 else 1024 if rate >= 22050 else 512) * 15 // 16 if tracks else 1
for k in range(compared):
    reference = pcm[k]
    if channels > 1 and k % 2:
        if game["left_only"]:
            # Les Sims 2 : la voie gauche dans les deux haut-parleurs
            reference = pcm[k - 1]
        else:
            # Battle of Giants : la voie droite a un echantillon de retard, le premier de chaque bloc vient d'ailleurs
            if k // 2 % hop == 0:
                continue
            reference = pcm[k - 2]
    expected = max(-1.0, min(1.0, reference)) * 32767
    gap = abs(got[k] - expected)
    if gap > worst:
        worst = gap
        state["worst_at"] = k
    total += gap * gap
    signal += expected * expected
from math import log10

snr = 10 * log10(signal / total) if total > 0 and signal > 0 else float("inf")
print(f"{path.replace(chr(92), '/').rsplit('/', 1)[-1]} : {done}/{frames} images par le code ARM du jeu ; luminance fausse {bad_y}, chrominance fausse {bad_c} ; son : {len(got)} ech. du jeu pour {len(pcm)} du rust, ecart max {worst:.0f} (a {state.get('worst_at')}), {snr:.1f} dB")
