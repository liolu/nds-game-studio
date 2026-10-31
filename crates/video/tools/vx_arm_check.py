"""Fait tourner le vrai decodeur VX de Code Lyoko USA (son code ARM, image et son) sous Unicorn, et compare au decodeur Rust.

Le seul moyen d'essayer une video ecrite par `vx::encode_like` tant qu'aucun jeu a videos VX ne demarre en natif : le code du jeu
lit l'ancienne image hors des blocs decodes et calcule sur 32 bits, ce que le decodeur Rust ne voit pas.

    cargo run -p nds-video --release --example vdump -- x.vx r.yuv r.f32
    python -I vx_arm_check.py <dossier extract/usa/arm9 du projet> x.vx r.yuv r.f32 [n images]

Il faut `pip install unicorn`. Le code du jeu reste dans le projet de decompilation, jamais dans ce depot.
"""
import struct, sys
from unicorn import *
from unicorn.arm_const import *

J = sys.argv[1].rstrip("/\\") + "/"
OPEN, ADVANCE, DECODE, AUDIO, MALLOC = 0x20b5ad8, 0x20b5844, 0x20b55a4, 0x20b5418, 0x209d908
DONE, HOOKS, CTX, FILEOBJ, HEAP0, STACK, OUT = 0x2ffff00, 0x2ffe000, 0x2f00000, 0x2f01000, 0x2400000, 0x2e00000, 0x2f10000

path = sys.argv[2]
data = open(path, 'rb').read()
ref = open(sys.argv[3], 'rb').read()
pcm = open(sys.argv[4], 'rb').read()
pcm = struct.unpack('<%df' % (len(pcm) // 4), pcm)
limit = int(sys.argv[5]) if len(sys.argv) > 5 else 10 ** 9

uc = Uc(UC_ARCH_ARM, UC_MODE_ARM)
uc.ctl_set_cpu_model(UC_CPU_ARM_926)
for base, size in ((0x02000000, 0x01000000), (0x01ff8000, 0x8000), (0x04000000, 0x10000)):
    uc.mem_map(base, size)
uc.mem_write(0x02000000, open(J + "arm9.bin", 'rb').read())
uc.mem_write(0x01ff8000, open(J + "itcm.bin", 'rb').read())
uc.mem_write(0x027e0000, open(J + "dtcm.bin", 'rb').read())
state = {'heap': HEAP0, 'pos': 0, 'biggest': 0}


def ret(value=None):
    if value is not None:
        uc.reg_write(UC_ARM_REG_R0, value)
    uc.reg_write(UC_ARM_REG_PC, uc.reg_read(UC_ARM_REG_LR))


def h_malloc():
    size = uc.reg_read(UC_ARM_REG_R0)
    at = state['heap']
    state['heap'] = (at + size + 63) & ~31
    state['sizes'] = state.get('sizes', []) + [(at, size)]
    uc.mem_write(at, b'\xcd' * size)
    ret(at)


def h_seek():
    state['pos'] = uc.reg_read(UC_ARM_REG_R1)
    uc.mem_write(FILEOBJ + 8, struct.pack('<I', state['pos']))
    ret(1)


def read_file():
    buf, n = uc.reg_read(UC_ARM_REG_R1), uc.reg_read(UC_ARM_REG_R2)
    # Le jeu ne doit jamais lire plus que le tampon reserve d'apres l'en-tete
    for at, size in state.get('sizes', []):
        if at <= buf < at + size and buf + n > at + size:
            state['overflow'] = (hex(buf), n, size)
    chunk = data[state['pos']:state['pos'] + n]
    uc.mem_write(buf, chunk + b'\0' * (n - len(chunk)))
    state['pos'] += n
    uc.mem_write(FILEOBJ + 8, struct.pack('<I', state['pos']))
    state['biggest'] = max(state['biggest'], n)


def h_read():
    read_file()
    ret(uc.reg_read(UC_ARM_REG_R2))


def h_async():
    read_file()
    ret(0)


table = {HOOKS + 8: h_seek, HOOKS + 12: h_read, HOOKS + 16: h_async, MALLOC: h_malloc}


def code_hook(uc, address, size, user):
    fn = table.get(address)
    if fn:
        fn()
    elif HOOKS <= address < HOOKS + 0x40:
        ret(1)


uc.hook_add(UC_HOOK_CODE, code_hook, begin=MALLOC, end=MALLOC + 4)
uc.hook_add(UC_HOOK_CODE, code_hook, begin=HOOKS, end=HOOKS + 0x40)
uc.mem_write(HOOKS + 0x100, b''.join(struct.pack('<I', HOOKS + o) for o in range(0, 0x40, 4)))
uc.mem_write(HOOKS, struct.pack('<I', 0xe12fff1e) * 0x10)
uc.mem_write(FILEOBJ, struct.pack('<III', HOOKS + 0x100, 0, 0))
uc.mem_write(DONE, struct.pack('<I', 0xe12fff1e))
uc.mem_write(CTX, b'\0' * 0x200)


def call(addr, *args):
    for reg, value in zip((UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3), args):
        uc.reg_write(reg, value)
    uc.reg_write(UC_ARM_REG_SP, STACK)
    uc.reg_write(UC_ARM_REG_LR, DONE)
    try:
        uc.emu_start(addr, DONE, count=400_000_000)
    except UcError as e:
        print('PLANTAGE du decodeur ARM :', e, 'pc', hex(uc.reg_read(UC_ARM_REG_PC)), 'image', state.get('frame'))
        raise SystemExit(1)
    return uc.reg_read(UC_ARM_REG_R0)


u32 = lambda at: struct.unpack('<I', uc.mem_read(at, 4))[0]
if not call(OPEN, CTX, FILEOBJ, 3, 0):
    print('open refuse le fichier')
    raise SystemExit(1)
frames, w, h, nch = u32(CTX + 8), u32(CTX + 12), u32(CTX + 16), u32(CTX + 32)
cw, ch = w // 2, h // 2
frame_bytes = w * h + 2 * cw * ch
sizes_at_open = u32(CTX + 36)
bad_y = bad_c = done = 0
samples_ok = samples_all = 0
pcm_at = 0
offenders = 0
for i in range(min(frames, limit)):
    state['frame'] = i
    if not call(ADVANCE, CTX):
        print('advance rend 0 a', i)
        break
    call(DECODE, CTX)
    y, uv = u32(CTX + 0x50), u32(CTX + 0x54)
    want = ref[i * frame_bytes:i * frame_bytes + frame_bytes]
    luma = bytes(uc.mem_read(y, w * h))
    if luma != want[:w * h]:
        bad_y += 1
        if bad_y <= 2:
            first = next(k for k in range(w * h) if luma[k] != want[k])
            print(f'  image {i} : premier ecart x={first % w} y={first // w} jeu {luma[first]} rust {want[first]}')
    # Chrominance : U et V entrelaces, lignes de la largeur de la luminance
    rows = bytes(uc.mem_read(uv, w * ch))
    u = b''.join(rows[r * w:r * w + w][0::2] for r in range(ch))
    v = b''.join(rows[r * w:r * w + w][1::2] for r in range(ch))
    if u + v != want[w * h:]:
        bad_c += 1
    # Son : une trame et un canal par appel, tant que le jeu en demande
    calls = 0
    while nch:
        uc.mem_write(OUT, b'\0' * 1024)
        if not call(AUDIO, CTX, OUT):
            break
        got = struct.unpack('<128h', uc.mem_read(OUT, 256))
        c, period = calls % nch, calls // nch
        for n in range(128):
            at = pcm_at + (period * 128 + n) * nch + c
            if at < len(pcm):
                samples_all += 1
                expected = int(max(-1.0, min(1.0, pcm[at])) * 32768)
                if abs(got[n] - expected) <= 1:
                    samples_ok += 1
                elif state.setdefault('badsound', 0) < 5:
                    state['badsound'] += 1
                    print(f'  son image {i} appel {calls} ech {n} : jeu {got[n]} rust {expected}')
        calls += 1
    pcm_at += calls * 128
    # Le paquet entier a ete consomme : ni plus ni moins que ce que dit sa taille
    index = u32(CTX + 0xf0)
    used = u32(CTX + 0x34) - (u32(CTX + 0xd0 + 4 * index) + u32(CTX + 0xe0 + 4 * index))
    size = u32(CTX + 0xd8 + 4 * index)
    state['left'] = state.get('left', {})
    state['left'][size - 2 - used] = state['left'].get(size - 2 - used, 0) + 1
    done += 1
print(f'{path.replace(chr(92), "/").rsplit("/", 1)[-1]} : {done}/{frames} images par le code ARM du jeu ; luminance fausse {bad_y}, chrominance fausse {bad_c} ; son {samples_ok}/{samples_all} ech. egaux ; '
      f'octets laisses par paquet {state.get('left')} ; plus grande lecture {state["biggest"]} (entete max {sizes_at_open}), depassement tampon {state.get("overflow")}')
