# Native DS games on macOS: port plan

Status: steps 1-7 of section 5 are written, not yet built or run on Windows. The no-ROM harness (4.1) passes on
macOS, natively on arm64 and as x86_64 under Rosetta. On arm64, `native_build.py` builds AL7E from its recipe and the
player's ROM, and the module boots headless (intro, logos, title screen). Branch `macos-native` on the fork `lasscat/nds-game-studio`.

The studio itself (launcher, engine, mod tools) already builds and ships for macOS: `build.yml` makes universal
binaries and an `.app`. Only the **native game pipeline** is Windows-only: translating a game (`arm2c.py`), building its
module (`native_build.py`), running it (`nitro-pc`) and showing it in the launcher (`native_game.rs`).

## 1. Why it is Windows-only today

| # | Assumption | Where |
|---|---|---|
| 1 | **32-bit identity mapping**: a DS address is used directly as a host pointer. `RD32(a)` is `*(u32 *)a`. The runtime maps RAM at `0x02000000`, IO at `0x04000000`, VRAM at `0x06000000`, and so on. | `arm2c.py` PRELUDE (`RD*/WR*/P32`), `core.c` `nds_start`/`reserve` |
| 2 | **Host pointers stored in 32 bits**: `(u32)&func` for code pointers, `lr = (u32)&&L…` for internal subroutines, `sp = (u32)&stack_[n]` (the C stack is DS stack memory), and x86 function pointers written into DS memory (`nds_fixups`). `nds_call_target` treats values outside the DS range as x86 addresses. | `arm2c.py:341`, `:447`, `:621`; `core.c` `load_arm9`, `nds_call_target` |
| 3 | **i386 assembly**: `OS_SaveContext`/`OS_LoadContext` (green threads), the stack switch in `nds_start`, `nds_irq_tramp`, `rep movsb/stosb`, x87 `fyl2x` | `core.c:23,26,364,449,696`, `snd.c:27` |
| 4 | **Win32 called directly** through `kernel32.def`/`winmm.def`: `VirtualAlloc/Protect`, `CreateFileA`, threads and events, `QueryPerformanceCounter`, file mappings, `waveOut*` audio, vectored exception handler | `nitro.h`, every `nitro-pc/*.c` |
| 5 | **Toolchain**: WinLibs `gcc -m32`, `ld -m i386pe --image-base 0x10000000`, `dlltool`, bundled `python.exe`; the CI job runs on `windows-latest` only | `native_build.py`, `make_toolchain.py`, `ci_toolchain.ps1`, `native-toolchain.yml` |
| 6 | **Launcher**: `SharedFrames::new` returns `None` off Windows, modules are named `*.exe`, the toolchain is detected through `python/python.exe` | `crates/engine/src/native_game.rs` |

## 2. Measured on this Mac (Apple Silicon, macOS 27.2, Apple clang 21)

| Test | Result |
|---|---|
| arm64: `mmap(0x02000000, MAP_FIXED)` | refused: the 4 GiB `__PAGEZERO` covers all of it |
| arm64 linked with `-pagezero_size 0x1000` | **killed by the kernel at launch** (exit 137) |
| x86_64 under Rosetta, small pagezero | low fixed mapping still refused |
| arm64: reserve a 4 GiB + guard window anywhere | works (`0x7000000000`) |
| page size | 16 KiB (Windows: 4 KiB) |
| 32-bit (`-arch i386`) | no longer exists on macOS |

**Conclusion:** keeping DS addresses as host addresses cannot work on Apple Silicon. Rosetta does not help either, and
Apple is winding Rosetta down after macOS 27. The port therefore needs a **memory window**: DS address `a` lives at
`nds_mem + a`. The same design also covers Intel Macs (x86_64), and later Linux or 64-bit Windows.

## 3. Design

### 3.1 Memory window

- Reserve 4 GiB + 64 KiB as `PROT_NONE` at any address (`nds_mem`). Inside it, commit the same regions `nds_start`
  maps today (ITCM, main RAM and mirrors, IO, palettes, VRAM, OAM, GBA slot). Green-thread stacks stay at window offset
  `0x20000000+`, as `stack_alloc` does now.
- `nds_mem` is a `uptr` (`__UINTPTR_TYPE__`). `#define DS(a) ((u8 *)(nds_mem + (u32)(a)))` and
  `#define HOST2DS(p) ((u32)((uptr)(p) - nds_mem))`, in `nitro.h` and in the `arm2c.py` PRELUDE. Integer arithmetic
  rather than `(u8 *)0 + a`, which is undefined behavior.
- **Windows 32-bit (`_WIN32 && __i386__`, `NDS_IDENTITY`) keeps `nds_mem` as the constant 0**, so the compiler folds
  it away: same generated code, no regression risk for the current platform. Elsewhere `nds_start` gets the window
  from `os_mem_window()` (new in `os.h`, to implement in step 5), and `reserve`/`stack_alloc` commit inside it with
  `os_mem_alloc(DS(a), …)`.
- `REG32/16/8(a)` go through `DS()`: every use in the runtime is a DS address.
- Runtime functions that replace SDK functions (`hle.c`) take DS addresses as `u32`, as the translated caller passes
  them, and callbacks go through `nds_call_target`.
- Game data the runtime names (`OSi_ThreadInfo`, `OSi_IrqThreadQueue`) go through `GAME_SYM(name)` /
  `GAME_ADDR(name)`: the absolute symbol of `dsabs.s` on Windows, a `const u32 nds_addr_<name>` that the darwin
  build must emit in step 6.
- Generated code: `RD*/WR*/P32` go through `DS()`. Frames become `sp = HOST2DS(&stack_[n])`. This is valid because
  all game code runs on stacks inside the window: the main stack is switched at startup and the SDK threads use
  `stack_alloc`. In instrumented builds, `HOST2DS` asserts that its pointer is inside the window, which catches game code
  running on a render, audio or input thread.
- On arm64 this costs almost nothing, since `ldr w0, [x_base, w_off, uxtw]` is one instruction.

### 3.2 Code pointers stay DS addresses

- `(u32)&func` → emit the ARM address (`0x0201234du`, with the Thumb bit).
- **Drop `nds_fixups`**: DS memory keeps the game's original function pointers. This is also closer to the real
  hardware, because games can compare, hash or store those pointers.
- `nds_call_target` always maps DS address → host function. The binary search becomes a direct-mapped table over the
  code ranges (main RAM `0x02000000-0x02400000` + ITCM, 2-byte granularity, about 16 MiB of virtual memory touched
  lazily): one load per indirect call. `thread_tramp` also calls through it.
  **Done** in `nitro-pc/calltab.c` (no assembly, no system header, so the harness builds it as is). `nds_start`
  fills `code_map` from `nds_fn_table` before loading arm9. The direct range is `0x01FF8000-0x02400000`, a static
  `.bss` array of 8 MiB (win32) or 16 MiB (64-bit). A miss inside that range is fatal right away. The binary search
  remains only for addresses outside it (DTCM, mirrors). Values that are not DS code are fatal too: `MAGIC_LR`, gosub
  tags, and the old x86 addresses that used to pass through.
- Internal subroutines (`gosub_funcs`): `lr = (u32)&&L…` becomes `lr = GOSUB_TAG + k`, with a per-function
  `static void *const gs_[] = { &&L…, … }`. Returns (`goto *(void *)lr` / `pc_val`) become `goto *gs_[x - GOSUB_TAG]`.
  `GOSUB_TAG` sits above `0xFFF00000`, a range a real return address never uses and that differs from `MAGIC_LR`.
  **Done**: `GOSUB_TAG = 0xFFF00000`, and `gs_` ends with a 0 so that `GOSUB_RET(x)` can bounds-check `k`. A
  computed return goes to the C caller on `MAGIC_LR` and to `gs_[k]` on a tag. Any other value is a real code
  address, so it becomes a tail call through `nds_call_target`, as `bx rX` already does. Before, it was a jump to
  garbage.

### 3.3 Threads and assembly

- `Slot` widens to `uintptr_t` fields. New `OS_SaveContext`/`OS_LoadContext` and stack switch for **arm64** (x19-x30,
  sp, d8-d15) and **x86_64** (rbx, rbp, r12-r15, rsp, rip). The i386 version stays behind `#if`.
- `rep movsb/stosb` → `__builtin_memcpy/memset` off Windows. The x87 `fyl2x` becomes `log()`.
- `nds_irq_tramp` and the exception handler are used only by `--with-c` (see 3.5).
- **Done** in `nitro-pc/context.c` (no system header, so the harness builds it as is): `Slot`, `OS_SaveContext`/
  `OS_LoadContext`, `nds_ctx_init` (first start of a thread, used by `OS_InitContext`) and `nds_run_on_stack` (the
  startup switch to the 16 MiB stack). Three versions behind `#if`: i386 (the old code moved as is, same offsets, same
  bytes), x86_64 and arm64; the Slot offsets are checked by `_Static_assert`. Symbol names go through
  `__USER_LABEL_PREFIX__` (`_` on Mach-O and win32, none on ELF). A new thread starts in `nds_ctx_start` with its
  argument and entry in two callee-saved registers (rbx/r12, x19/x20), and stacks stay 16-byte aligned. On 64-bit
  `nds_copy`/`nds_zero` are `__builtin_memcpy/memset`, `ln()` in `snd.c` is `__builtin_log`, and `nds_irq_tramp`
  is a fatal stub.

### 3.4 Platform layer

New `nitro-pc/os.h` with a small API. `os_win32.c` holds today's code, moved verbatim. `os_posix.c` and
`audio_coreaudio.c` are new:

| Need | Win32 now | macOS |
|---|---|---|
| window, stacks, buffers | `VirtualAlloc` | `mmap` (+ `mprotect`) |
| rom, save, wav, bmp | `CreateFileA/ReadFile/WriteFile` | `open/read/write` |
| render pool, render, input, audio threads | `CreateThread`, `CreateEventA`, `WaitForSingleObject` | `pthread_create`, `dispatch_semaphore` |
| frame pacing | `QueryPerformanceCounter`, `Sleep`, `timeBeginPeriod` | `mach_absolute_time`, `nanosleep` |
| frames for the launcher | `OpenFileMappingA/MapViewOfFile` | `shm_open/mmap` |
| sound | `waveOut*` | AudioQueue (CoreAudio) pulling from `snd.c`'s mixer |
| RTC | `GetLocalTime` | `localtime_r` |
| `--prof` (dev only) | `SuspendThread/GetThreadContext` | `thread_get_state` on the main thread (later, optional) |

The runtime is freestanding on Windows (its own `libc.c`). On macOS it links `libSystem`, as every Mac binary must.

**Done** (step 5). `os_posix.c` and `audio_coreaudio.c` are macOS only (`#error` elsewhere); `native_build.py` picks
the platform files from `OS_SOURCES`, and the other sources are shared.
- Window: `mmap(PROT_NONE, MAP_NORESERVE)` of 4 GiB + 64 KiB anywhere. `os_mem_alloc` inside the window is
  `mprotect(RW)` rounded to 16 KiB pages, so it keeps contents like `MEM_COMMIT`. The small regions (palettes, OAM)
  get a whole page. Outside the window it is `mmap`, and it fails if a required address is taken, like `VirtualAlloc`.
- Entry: dyld calls `main` (in `os_posix.c`, which calls `nds_start`). Arguments come from `_NSGetArgv`. `os_exit` is
  `_exit`, so no destructor runs under the render or audio threads.
- Events: auto-reset, as on Windows. `set` raises a flag and signals a `dispatch_semaphore` only on 0 → 1, and
  `wait` takes the flag or sleeps, so several sets before one wait count once.
- `os_time_*`: `mach_absolute_time` (24 MHz on Apple Silicon, 1 GHz on Intel). `os_shm_open` adds the leading `/`
  if the name has none. `--prof` works (`thread_get_state`).
- Faults: `SIGSEGV`/`SIGBUS` on an alternate stack, with the DS address, read/write (ESR WnR, #PF W bit) and
  pc/sp/fp for the log. Single-stepping and `os_fault_call` (the `--with-c` trap) are fatal.
- Sound: an AudioQueue at the mixer's rate (stereo s16), 6 buffers of 384 frames, as with waveOut. The queue's
  callback frees a played buffer, which wakes `os_audio_acquire` on the audio thread of `snd.c`. The output therefore
  sets the mixer's pace, and `snd.c` does not change. With `--mute`, `snd.c` never opens the output.
- Checked without a ROM, arm64 and x86_64: stub game symbols boot `nds_start` (all regions, header copy, stack switch
  into the window, time, sleep), an access outside the regions logs its DS address and exits 4, and `--shm` maps an
  object made by `shm_open`. Audio: `sh native/tests/audio_smoke.sh [--arch x86_64] [--mute]` plays 2 s of a 440 Hz
  sine (left, then right) through `os_thread_start` + `os_audio_*` exactly as `snd.c` does, and checks that the
  output pulls it in real time.

### 3.5 Out of scope on 64-bit: `--with-c`

Matched decomp C uses real pointers: they become 64-bit on the host, which breaks struct layouts and every pointer
stored in DS memory. That mode also relies on the IO trap (4 KiB `VirtualProtect` + single-step). macOS therefore runs
**translated mode only**, which is already the default. `native_build.py` refuses `--with-c` off win32.

Since step 3, `--with-c` on Windows has also lost its function pointers. DS memory now holds ARM addresses, without
`nds_fixups`. Matched C that calls a pointer read from game data therefore jumps to ARM bytes, and a `&func` taken
in C is an x86 address that `nds_call_target` rejects. Translated mode is not affected. If `--with-c` still matters,
the C compiler needs its own route, for example calls through `nds_call_target` in the decomp headers.

### 3.6 Build (`native_build.py`)

- A host profile chooses the build. Windows stays as it is now. darwin uses
  `clang -arch arm64|x86_64 -std=gnu89 -O1 -fwrapv -fno-strict-aliasing -ffp-contract=off -fno-stack-protector -fno-omit-frame-pointer -w`.
  The runtime is built with `-std=gnu11 -O2 -Werror=pointer-to-int-cast -Werror=int-to-pointer-cast`, which surfaces
  every 32-bit pointer assumption left.
- `-ffp-contract=off` is required: clang on arm64 fuses `a*b+c` into FMA by default, which rounds differently from the
  DS.
- Link with `clang`: no `dlltool`/`.def`, and no absolute data symbols, since translated code uses numeric addresses.
  The output is `build/<code>/<code>` (no `.exe`). ld64 ad-hoc signs arm64 output, so a locally built module runs
  without Gatekeeper prompts.
- Parse ld64's missing-symbol format (`"_foo", referenced from:`) for the stub pass. Use `creationflags` only on
  Windows (`nice` + QoS on macOS).
- Keep calling convention details in mind: `CALLF` passes 14 `u32` through `u64 (*)()`. This works with Apple's arm64
  ABI as long as callees are non-variadic and the dialect stays pre-C23. The build pins `-std`.

**Done** (step 6). `--arch arm64|x86_64` (default: this Mac's). `--instrument` uses
`-finstrument-functions-after-inlining`, clang having no exclusion list. Two things the Windows build never needed:
- **Game functions get a host prefix, `ds_<name>`**. Games ship their own `memcpy`, `memset`, `sprintf`, `exit`… (22
  of them in AL7E), which take DS addresses. Under their real names, the runtime's libSystem calls and the
  `memcpy`/`memset` that clang emits by itself would bind to them, with truncated 64-bit pointers. Functions the
  runtime replaces keep their name. The runtime calls a game function by name through `GAME_FN(name)` (`nitro.h`),
  only `OS_SelectThread` today.
- **The platform layer replaces no game function.** `os_posix.c` defines `main`, and so does the game (crt0 passes
  `&main` to the SDK): counting it as a runtime replacement sent the game into `nds_start` again, in a loop.
- Game data the runtime names: `lift/dsaddr.c` with `const unsigned nds_addr_<name>` (instead of `dsabs.s`).
- Priority: `nice 10`, and compilers run under `taskpolicy -c utility` (P-cores still allowed, unlike `-b`).

Two translator and runtime bugs found on the way, **which affect Windows too** (to offer upstream):
- **Stack arguments past the 6th were lost.** `CALLF` forwarded 6 stack words (`STACK_ARGS`), and a callee read the 7th
  from its own uninitialized `stack_`. AL7E has three functions that read more, up to 9 (printf-style formatting,
  `func_020549e4` gets a pointer as its 7th): the title logo was missing, and x86_64 and instrumented builds crashed
  in the TT Games intro. Whatever the stack held decided the outcome, on Windows too. `STACK_ARGS` is now 10, the
  parameters are named `a0..a9` (`s8` is a type), and `CALLF` is generated from it. Harness case `t_args14`.
- `--instrument`: the DMA watch log read DS address 0 on every `MI_StopDma` (`src = dst = 0`), since the default
  `nds_watch = 1` matched `dst <= 1`.

Measured on this Mac, AL7E: build in about 27 s (lifted code: 4741 functions, no stubs), module 7 MiB.
`--mute --dump … --every 300 --frames 3000` runs in 50 s (paced at 60 fps, about 10 % of one core) and exits 0. The
ten frames (intro, LucasArts and TT Games 3D logos, Amaze credit, title screen) are byte-identical on arm64, on x86_64
under Rosetta, and in both instrumented builds.

### 3.7 Launcher (`crates/engine/src/native_game.rs`)

- `SharedFrames` on unix: `shm_open("/ndsgs-<pid>-<code>")` (macOS caps names at 31 chars; the current name is close
  to that limit), `ftruncate`, `mmap`, then `shm_unlink` on drop.
- Module file names use `std::env::consts::EXE_SUFFIX`, both when generating and when scanning `native/`.
- Toolchain: `python/bin/python3` on unix. The asset becomes `native-toolchain-<v>-macos-<arch>.zip`. Before
  generating, check `xcrun --find clang`; if it is missing, show `tr("…", "…")` explaining `xcode-select --install`.
  All new text follows the i18n rule.
- Lower the generator's priority (`setpriority`/`taskpolicy -b`) so the Mac stays usable, as `IDLE` does on Windows.

**Done** (step 7), in `native_game.rs`:
- `SharedFrames` on unix is `shm_unlink` (stale name) + `shm_open(O_CREAT|O_EXCL)` + `ftruncate` + `mmap`; `drop` does
  `munmap` + `shm_unlink`. The name is `/ndsgs-<pid>-<code>` (`shm_name`), refused above 31 characters. Unit test
  `la_memoire_partagee_est_vue_des_deux_cotes_puis_detruite` opens it the way `os_shm_open` does.
- `CAN_GENERATE` is true on Windows and macOS. Module names go through `EXE_SUFFIX` (`module_file`, `module_code` for
  the scan of `native/`); a generated module is `<code>-<toolchain>` in `data_dir()/native`.
- `check_compiler()` runs `xcrun --find clang` before anything is downloaded; failure is a `tr(…)` message naming
  `xcode-select --install`, shown on the game's page and in the toast.
- The generator runs at `setpriority(10)` (`pre_exec`); `native_build.py` adds `taskpolicy -c utility` to its compilers.
  `taskpolicy -b` was not used (it confines the work to the E-cores), as in 3.6.
- Toolchain: `python/bin/python3`; asset `native-toolchain-<v>-macos-<arm64|x86_64>.zip` (cached in
  `native-toolchain/<v>-macos-<arch>`). Local tests, without the CI archive: `NDSGS_TOOLCHAIN_ZIP=<zip>` (as before),
  `NDSGS_TOOLCHAIN_DIR=<folder>`, or a `native-toolchain` folder next to the binary. `native/tools/dev_toolchain.sh`
  builds that folder from the repo (symlink to `native/`, Python + capstone).

### 3.8 Toolchain and CI

- `make_toolchain.py --platform macos-arm64|macos-x86_64`: translator, runtime, recipes, python-build-standalone
  (install_only) and the matching capstone wheel. **No compiler**, because the Apple SDK may not be redistributed:
  players need the Xcode Command Line Tools.
- `native-toolchain.yml`: add `macos-latest` (arm64) and an Intel runner, plus a job that compiles the runtime with
  `-Werror` on macOS and with `-m32` on Windows. That job needs no ROM and catches regressions on both sides.
- Bump `TOOLCHAIN` in `export_recipes.py` (project rule after any change in `native/`).

**Done** (step 8):
- `make_toolchain.py --platform macos-arm64|macos-x86_64 --python <install_only .tar.gz or folder> --capstone <.whl or
  folder> --out x.zip`. Python is python-build-standalone 3.13.18 (release 20261026), capstone 5.0.7 from the wheel of the
  same platform. The archive keeps the stdlib and drops tests, Tk, pip, headers and the two extra copies of
  `bin/python3` (18 MiB each): **22 MiB** per platform. Links are followed and the exec bit is kept (the launcher's zip
  extraction restores it). `NOTICE.txt` lists the licenses, capstone's text is in `licenses/`. No compiler.
- `ci_toolchain.sh macos-arm64|macos-x86_64 <zip>`: downloads Python (sha256 pinned), `pip download`s the wheel of the
  *target* platform, assembles, then unzips the result and runs its `python3` with `import capstone` as a check.
- `native-toolchain.yml`: `runtime-macos` (arm64 on `macos-latest`, x86_64 on `macos-15-intel`) and `runtime-windows` need
  no ROM. They run `native/tests/runtime_check.py`, which compiles every runtime file with the flags and source list of
  `native_build.py` (macOS: `-Werror`, plus the pointer-cast errors; Windows: `gcc -m32` from WinLibs), then
  `arm2c_test.py`. `-Wdeprecated-non-prototype` stays a warning: calls through unprototyped `Fn` pointers are deliberate
  (3.6). `toolchain-macos` waits for `runtime-macos`. Archives are published to the `native-toolchain` release only from
  `experimental`; from any other branch they are workflow artifacts.
- Launcher check, empty data dir, `NDSGS_TOOLCHAIN_ZIP=<arm64 zip>`, `native_check AL7E <rom> out.png 150`: the zip is
  extracted into `native-toolchain/10-macos-arm64` (`python3` executable), `AL7E-10` is generated (Mach-O arm64, 7 MiB) and
  the runtime starts. Run on its own, that module dumps the expected frames (Amaze credit at frame 900). **The
  `native_check` screenshot is entirely black**, and byte-identical with `NDSGS_TOOLCHAIN_DIR` (dev toolchain, no zip), so
  it is not the packaging: the on-screen path of step 7 is still unchecked.
- `TOOLCHAIN` is 10. `native/games/index.json` only had the version changed (it was still 3): it is written by
  `export_recipes.py`, which needs the decompiled projects and ROMs, so the game stats were left as they were.

## 4. Validation

No ROM is, or ever will be, in the repo or the CI, so:

1. **Translator unit test, no ROM**: a small C harness built on macOS and Windows covers `RD/WR` + `LDRW` rotation,
   flag macros, `CALLF` with 10 and 14 arguments, gosub dispatch, and `nds_call_target` on Thumb and ARM addresses.
   **Done**: `native/tests/`. `arm2c_cases.s` holds small ARM/Thumb functions. Their bytes are embedded in
   `arm2c_test.py`, which lifts them with `arm2c.Lifter`, checks the generated C (no `(u32)&`, no `goto *(void *)`,
   literals are ARM addresses, `gs_` tags), then compiles `harness.c` + `lifted.c` + `nitro-pc/calltab.c` +
   `nitro-pc/context.c` twice (normal and `NDS_INSTRUMENT`) and runs it. The green-thread test switches to a window
   stack with `nds_run_on_stack`, then two threads made by `nds_ctx_init` hand over to each other a million times
   each through `OS_SaveContext`/`OS_LoadContext`, checking callee-saved values (integer and `d8-d15`), their stack,
   frame alignment and `nds_cpsr`.
   - macOS / Linux: `pip install capstone==5.0.7`, then `python3 native/tests/arm2c_test.py`. The harness reserves a
     4 GiB window and runs the translated code on a thread whose stack is inside the window. On Apple Silicon,
     `--arch x86_64` builds the Intel version, which Rosetta runs.
   - Windows: `python native\tests\arm2c_test.py`, either with the launcher toolchain's `python\python.exe`, which
     already has capstone, or with any Python + capstone and WinLibs on the `PATH`. The script borrows the compiler
     path and `LIFT_FLAGS`/`RUNTIME_FLAGS` from `native_build.py` and builds with `gcc -m32` (`nds_mem = 0`, RAM
     committed at `0x02000000`). **Not run on Windows yet.**
   - After editing `arm2c_cases.s`: `python3 native/tests/arm2c_test.py --regen`. This needs clang with the ARM
     target, which the macOS one has. It applies the `R_ARM_CALL` relocations itself.
2. **Golden frames** with the player's own ROM of a verified game (AJTP, AR5E, AL7E): run the same headless command on
   Windows (today's 32-bit build) and on macOS, then compare the BMPs and the `--wav` output:
   `--mute --frames 3000 --dump out --every 300 --keys …`. Green threads and frame skipping can shift timing, so start
   with visual comparison and use exact matching on early frames.
3. Run through the real engine: `cargo run --release -p nds-engine --example native_check -- …`.
4. Speed: full speed on an M1 at the default HD scale, measured with frame times in the log.

## 5. Order of work (one reviewable commit or PR each; Windows keeps working at every step)

| Step | Change | Done when |
|---|---|---|
| 1 | Extract `os.h` + `os_win32.c` (no behavior change). **Code done, not yet built or run on Windows** | Windows module builds and boots the same |
| 2 | `DS()`/`HOST2DS()` in translator and runtime, `nds_mem = 0` on win32. **Code done**: runtime passes `clang -target x86_64-apple-macos -fsyntax-only -Werror=pointer-to-int-cast -Werror=int-to-pointer-cast` (arm64: only the x86 inline asm of step 4 remains); with `clang -target i686-w64-mingw32`, lifted code is byte-identical and runtime code differs only where a call now goes through `nds_call_target`. Not yet built with GCC or run on Windows | same golden frames on Windows |
| 3 | Code pointers as DS addresses, direct-mapped dispatch, no fixups, gosub tags. **Code done**: no-ROM harness passes on macOS (arm64), runtime still passes the step 2 checks (macOS `-Werror` casts, i686-mingw syntax). Not yet built with GCC or run on Windows | same golden frames on Windows |
| 4 | 64-bit `Slot`, arm64/x86_64 context switch, no x86 inline asm on 64-bit. **Code done**: harness passes on macOS arm64 and x86_64 (Rosetta); the whole runtime except `os_win32.c` compiles for `arm64-apple-macos` and `x86_64-apple-macos` with the `-Werror` cast flags, and for `i686-w64-mingw32`, where `context.c` assembles to the old i386 bytes. Not yet built with GCC or run on Windows | unit harness passes on macOS |
| 5 | `os_posix.c` + CoreAudio + POSIX shm. **Code done**: the whole runtime (arm64, x86_64) compiles with `-std=gnu11 -O2 -Wall -Werror=pointer-to-int-cast -Werror=int-to-pointer-cast` and links against libSystem + AudioToolbox; only the game's own symbols are missing (step 6). Audio smoke test passes on both | runtime compiles with `-Werror` on macOS |
| 6 | darwin profile in `native_build.py`. **Done**: AL7E boots headless on arm64 and reaches its title screen (3.6) | **milestone: a game boots headless on macOS**, frames dumped |
| 7 | Launcher integration (`native_game.rs`). **Code done**: module generated by the launcher from a local toolchain folder and started with the shared-memory frames (see 3.7); not yet checked on screen | Play button works on macOS |
| 8 | Toolchain packaging + CI jobs. **Done locally**: arm64 zip built by `ci_toolchain.sh`, extracted by the launcher from `NDSGS_TOOLCHAIN_ZIP` on an empty data dir, AL7E module generated and booted headless. The CI jobs have not run yet | launcher downloads the macOS toolchain and generates a module |
| 9 | Docs: `README.md`, `native/README.md`, `GAMES.md` platform column (in French, like the rest of the repo) | |

Steps 1-3 also benefit Windows (fewer hidden x86 assumptions) and are the ones to offer upstream first.

## 6. Risks and open questions

- **Hidden `u32` ↔ pointer casts** in the runtime (about 130, 73 of them in `core.c`). The `-Werror` cast flags find
  them; each one is either a DS address (`DS()`) or a real host pointer (widen it).
- **Game code on a foreign stack**: if any runtime thread ever calls game code, `HOST2DS` breaks. Covered by the
  instrumented-build assert.
- **Cost of the indirect-call lookup and the base add**: expected to be small. Measure in step 3 on Windows, where
  the base add is free, to isolate the lookup.
- **Xcode Command Line Tools required** for players. Possible later step: ship a prebuilt, signed runtime from the CI,
  plus LLVM `clang` + `ld64.lld`, and compile only the translated game code.
- **Testing needs a real ROM** of a verified game, and a Windows machine for the golden reference. The maintainer
  has both; this Mac needs a Rust toolchain (`rustup`), `pip install capstone`, and a ROM the user owns.
