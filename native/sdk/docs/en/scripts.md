# Script mods (Luau)

A script mod needs no compiler and no release: the repository is the mod. Players paste its link in the Modding tab; the launcher downloads the script host once (the first time a script mod is installed) and runs your [Luau](https://luau.org) script in the game.

```
my-script-mod/
  nds-mod.json
  scripts/main.luau
```

```json
{ "format": 2, "id": "skip-intro-script", "version": "1.0.0", "type": "script", "api": "1.0",
  "script": "scripts/main.luau", "games": { "AL7E": ["<rom SHA-1>"] } }
```

`template-script/` in this SDK is a complete one (Lego Star Wars II starts straight on its menu).

## The `nds` table

Same access as a native mod (`nds_mod.h`), with numbers instead of pointers:

| | |
|---|---|
| `nds.read8/16/32(address)`, `nds.write8/16/32(address, value)` | DS memory and registers |
| `nds.call(address, r0, r1, r2, r3)` | call a function of the game; returns `r0, r1` |
| `nds.hook(address, function(r0, r1, r2, r3, original) ... end)` | replace a function of the game; `original(r0, r1, r2, r3)` runs the game's code (or the next mod's) and returns `r0, r1`; return `r0, r1` |
| `nds.hook_insn(address, function(regs) ... end)` | before an instruction (declared in `"instruction_hooks"`); `regs.r0` ... `regs.r15`, `regs.cpsr` can be changed |
| `nds.on_frame(function(frame))`, `nds.on_irq(function(bits))` | each VBlank, each interrupt |
| `nds.on_gx(function(cmd, params) -> drop)`, `nds.on_polygon(function(vertices, attr, tex, pal) -> drop)` | 3D: change `params[i]` or `vertices[i].x` (`x y z w r g b s t`), return `true` to drop |
| `nds.on_present(function(w, h))` + `nds.pixel(screen, x, y[, color])` | the final images (screen 0 = top, 1 = bottom); a colour is `0xAABBGGRR` |
| `nds.on_audio(function(frames))` + `nds.sample(i[, left, right])` | the mixed sound |
| `nds.on_io_read(function(addr, size, value) -> value)`, `nds.on_io_write(function(addr, size, value) -> value or false)` | registers |
| `nds.on_rom_read(function(pos, size))` + `nds.rom_byte(i[, value])` | what the game reads from the rom |
| `nds.input()` → `{ds_keys, touch_down, touch_x, touch_y, mouse_x, mouse_y, mouse_buttons, wheel, cursor_held}`, `nds.key_down(code)` | input; key codes are `NDS_KEY_*` of `nds_mod.h` (HID) |
| `nds.take_input(on)`, `nds.capture_mouse(on)`, `nds.notice(text[, ms])`, `nds.log(...)` | |
| `nds.setting(key)`, `nds.frame()`, `nds.game_code`, `nds.language`, `nds.platform` | |

## Sandbox and permissions

The script runs in a sandbox: Luau's standard libraries are read-only, and there is no access to files, the network or the system. A mod that needs more says so in `nds-mod.json`:

```json
"permissions": ["files"]
```

The launcher shows it before installing, and installing accepts it. With `files`, `nds.read_file(path)` and `nds.write_file(path, text)` work. `network` and `system` are not available yet in this host: a mod that asks for them is refused, with the reason.

## No time limit, except an infinite loop

A script can take as long as it needs. Only a callback that does not return within 10 seconds is stopped: the script is removed from the game, the game goes on, and the mod's line in the launcher says why. An error in your script does the same, with its message. Use `pcall` to handle your own errors.

A script runs in the game's thread for most callbacks, but `on_present` and `on_audio` come from the drawing and sound threads: the host makes them wait for each other, so a slow `on_present` slows the game.
