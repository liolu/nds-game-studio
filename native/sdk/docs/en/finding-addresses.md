# Finding an address

A hook needs the address of a function of the game (where it starts in the DS memory), and changing a value needs the address of that value. Addresses are those of one version of the game: list the SHA-1 of its rom in `"games"`.

## Functions

- **The game's decompilation project**, if there is one: `config/<version>/arm9/symbols.txt` gives every function, with its name once someone found it (`OS_GetTick kind:function(arm,size=0xa0) addr:0x020ce6c8`), and `src/` its C code. Overlays have their own `symbols.txt`.
- **The game's recipe** in the runtime (`native/games/<code>.json.gz`): every translated function, `[address, size, thumb, name]`. Any of these addresses can be hooked.
- **The launcher's Code window (F5)** shows the game's sources when the decompilation is installed.
- **A disassembler** (Ghidra with dsd-ghidra, IDA, Binary Ninja) on the rom, to read the ARM code and follow who calls what.
- `api->hook` returns -1 if the address is not the start of a translated function: the log says so.

Good functions to hook are the ones the game calls once per frame or once per object: the camera update, a character's update, the input reading. Your hook receives the same `r0`-`r3` as the game's code, usually `r0` = the object.

## Values

- **Watch the memory while playing**: `--peek "frame:address:size:file"` copies memory to a file at a given frame (see [testing.md](testing.md)). Compare two dumps: what changed when the character lost a life is the counter.
- **Who writes a value**: an instrumented module (`native_build.py --instrument`) with `--watch address` logs the ARM instruction that writes there.
- **Structures**: a pointer you found in a function's code (`ldr r0, =0x02109cd4`) is often a global object; its fields are offsets read by the functions that use it.
- `api->runtime("name")` gives any function or variable of the runtime itself (`nds_frame`, the frame counter). No promise: a name can disappear in a later runtime; check for 0.

## Instructions

`api->hook_insn` hooks one instruction inside a function and gives your callback the ARM registers (`nds_regs`), which you can change. Declare the address in `"instruction_hooks"` for that game: the launcher generates the module again with it. Prefer function hooks when you can: they need nothing.
