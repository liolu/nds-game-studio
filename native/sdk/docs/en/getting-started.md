# Getting started: your first native mod

A native mod is a small library (`.dll` on Windows, `.dylib` on Mac) that the game's runtime loads. It only knows `sdk/nds_mod.h`: the runtime gives it a table of functions (`nds_api`) to read and write the DS memory, replace any function of the game, watch the 3D commands, draw on the final images, play with the sound, read the keyboard and the mouse.

Players install it with a link to your repository, from the game's **Modding** tab. The launcher downloads the archive for their computer from your latest release and checks it against `SHA256SUMS`. Nothing is compiled on their computer.

## 1. Make your repository from the template

1. Copy the `template/` folder of this SDK into a new repository (GitHub, public).
2. In `nds-mod.json`, change `id` (letters, digits and `-`), `name`, `name_fr`, `description`, `author`.
3. Rename `src/my_mod.c` if you like, and the library names in `"plugin"` to match.

`nds-mod.json`:

| Field | Meaning |
|---|---|
| `"format": 2`, `"type": "native"` | A mod that brings code |
| `"api": "1.0"` | The version of `nds_mod.h` it was made for. A runtime with another major version refuses it, with the reason |
| `"games"` | Game codes (`["AL7E"]`), or code → SHA-1 of the roms it was tested with (`{"AL7E": ["7c1a..."]}`). Absent: every game. Addresses change from one version of a game to another: list the roms when you use addresses |
| `"plugin"` | Computer → library: `windows-x86`, `macos-arm64`, `macos-x86_64` |
| `"settings"` | What players can change in the launcher: `bool`, `int`, `float` (`min`, `max`, `step`), `choice` (`choices`), `string`, `keybind` (default by key name: `"KeyC"`, `"F9"`). `api->setting(mod, key)` gives the value as text; a key is its HID code (`NDS_KEY_*`) |
| `"presets"` | Values for several settings at once: `{"name": "...", "values": {...}}` |
| `"instruction_hooks"` | `{"AL7E": ["0x020cea88"]}`: instructions your mod hooks with `api->hook_insn`. The launcher generates the game again with them |
| `"multiplayer": "safe"` | Your mod does not change the game: players with and without it can play together |
| `"save": "separate"` | Your mod touches the save: it gets its own |

## 2. Write the code

```c
#include "../sdk/nds_util.h"

NDS_MOD_EXPORT int nds_mod_load(const nds_api *api, nds_mod *mod) {
    api->log(mod, "loaded for %s", api->game_code);
    return 0; /* anything else: refused, with api->fail(mod, "why") */
}
```

- **Replace a function of the game**: `api->hook(mod, address, my_function, &original)`. Every function of the game has the same signature (`nds_fn`): `r0`-`r3`, then the first 10 words of the stack; it returns `r0` (low) and `r1` (high). Call `original` to run the game's code (or the next mod's), or do not.
- **Callbacks**: `on_frame` (each VBlank), `on_gx` (3D commands), `on_polygon` (vertices you can change), `on_present` (the final images), `on_audio`, `on_io_read`, `on_io_write`, `on_irq`, `on_rom_read`.
- **Memory**: always through `api->read32`, `api->write32` (and 8, 16 bits) or `api->memory(address)`. Never `*(u32 *)0x02100000`: the DS memory is not at its DS address on a 64-bit computer.
- **Input**: `api->input(&in)` gives the DS buttons, the stylus, the mouse and the whole keyboard; `api->take_input` keeps the keys from the game, `api->capture_mouse` holds the cursor (F7 always gives it back to the player), `api->notice` shows a message.
- **Threads**: `on_present` runs in the drawing thread and `on_audio` in the sound thread; everything else in the game's thread.
- **Windows without a C library**: the 32-bit Windows module has no C library. Write plain C (no `printf`, `malloc`, `sin`): `sdk/nds_util.h` has sine, cosine, arc tangent and settings helpers. On Mac, the C library is there.

## 3. Build and try

```bash
./build.sh windows-x86      # Windows: WinLibs gcc (https://winlibs.com, the 64-bit one, it builds -m32) on the PATH
./build.sh macos-arm64      # Mac
```

Then see [testing.md](testing.md): run the game from the command line with your library, or install your folder in the launcher (Modding tab, Add a folder).

## 4. Publish

Push, then tag a version that matches `nds-mod.json`:

```bash
git tag v1.0.0
git push --tags
```

`.github/workflows/release.yml` builds for the three computers and publishes `<id>-1.0.0-<computer>.zip` with `SHA256SUMS`. Players paste your repository's link in the Modding tab. Before installing, the launcher says « Runs code on this computer » and shows where it comes from.

Next: [finding an address](finding-addresses.md), [testing](testing.md).
