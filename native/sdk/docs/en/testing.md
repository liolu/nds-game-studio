# Testing your mod

## In the launcher

Modding tab of the game → **Add a folder** → your mod's folder (with `nds-mod.json` and the built library). Its settings show on its line. After a game, the line says whether the runtime loaded it, or why not.

## From the command line, without the launcher

The launcher keeps the game's generated module in its data folder: `%APPDATA%\Nds Game Studio\native\` on Windows, `~/Library/Application Support/Nds Game Studio/native/` on Mac (`AL7E-r2.exe` for instance). Run it with your rom:

```bash
AL7E-r2.exe "rom.nds" --plugin lib/windows-x86_64/my_mod.dll --mute --frames 1200 --dump shots --every 300
```

- `--plugin a.dll,b.dll`: libraries loaded without a manifest (settings are absent). `--plugins list.json`: a list like the launcher writes, with settings; `status.json` is written next to it.
- `--no-mods`: no mod at all, to compare.
- `--frames n`: stop after n frames. `--dump folder --every n` (`--dumpfrom n`): BMP images of both screens.
- `--fixed-clock`: the same images at every run, as fast as the computer allows. Use it to compare with and without your mod.
- `--keys "frame:mask[/x/y],..."`: DS buttons (A=1, B=2, Select=4, Start=8, right=16, left=32, up=64, down=128, R=256, L=512, X=1024, Y=2048) and the stylus.
- `--hid "frame:code,..."`: a key of the whole keyboard (HID code, `NDS_KEY_*`), 0 = released.
- `--look "frame:x:y,..."`: the mouse (sum of its movements).
- `--peek "frame:address:size:file,..."`: copy DS memory to a file. `--poke "frame:address:value,..."`: write a word.
- `--lang fr|en`: the language your mod gets in `api->language`.

The log (the console, or `saves/native-<code>/runtime.log` in the data folder after a game from the launcher) has every line of `api->log`, the reason a mod was refused, and the loaded mods when the game crashes.

## Before publishing

- Run with `--no-mods` and with your mod: only what you meant should change.
- Try another game if your mod has no `"games"`: the runtime only refuses mods for other games, not mods that misbehave.
- In multiplayer, players must have the same mods unless yours says `"multiplayer": "safe"`.
