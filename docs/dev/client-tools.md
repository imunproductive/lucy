# Client tools

Python tools in `tools/` that answer questions about the client's code, such as those on the IDA list (`D:/Work/Coding/refs/reports/IDA-questions.md`), without a manual IDA session. Run them from the repository root.

They read what the client research produces: a dump of the running game and the decrypted game. That research and its own tools (unpacking, decrypting, dumping) live in a separate, private repository. Its folder is `ORPHEUS_RESEARCH_DIR` (default `D:/Work/Research/Overwatch`). The tools read from it:

- `dumps/ingame_7FF788F10000.exe`: an image dump of the game in a match, rebased to `0x7FF788F10000`, the base the IDA list uses;
- `game/_retail_/Overwatch.debug.exe`: the decrypted game, prepared to run under a debugger;
- `docs/analysis/*.json`: hand-made names;
- `tools/dumpfix.py` and `tools/ida_apply_rose.py`: two research scripts that rebuilding the game database loads.

Other settings: `ORPHEUS_BIN` (the `orpheus.exe` the tools start), `ORPHEUS_GAME` (the install for `stu.py`'s where-used query), `OW_CODECS_JSON`, `IDADIR`, `ROSE_BIN`.

What the tools generate goes to `research/out/` (ignored by git). All paths are in `tools/paths.py`.

## Which tool

| Question | Tool |
|---|---|
| What does statescript class `STU_x` do, which fields does it have | `python tools/stu.py STU_x` |
| What is at this address: whose evaluator, which message handler, which component | `python -m tools.owindex <address>` |
| Who reads field `m_x` (from the code) | `python tools/who_reads.py STU_x m_x` |
| What does a function return, when is it called (in the game) | `python -m tools.owdbg.probe` |
| Who reads a field or an address (in the game) | `python -m tools.owdbg.watch` |
| Who reads field N of network message M (the server sends a marker) | `python -m tools.owdbg.watch --msg M --field '#N' --send ...` |
| Which functions run only when I do X | `python -m tools.owdbg.cover run`, then `cover diff` |
| Which function crashed | `python -m tools.owdbg.crash` |

## The tools

- **`owindex/`**: an index of the client's runtime tables, built from the dump into `research/out/index/ow_index.sqlite` (`python -m tools.owindex.build`, about 10 s). `python -m tools.owindex.check` compares it with `crates/fixed/data/statescript_codecs.json`. It holds:
  - the 5,478 STU classes with their parents and fields (offset, type, network flags) and OWLib's labels. Labels only: the client keeps no names, and most classes and fields are just hashes;
  - the 1,735 statescript registrations (config 1027, state 438, action 260, entry 5, condition 4) with the winning one per class, its runtime vtable and its functions: config `evaluate` (slot 4), action `execute` (slot 10), condition `evaluate` (slot 8), state reader, block and writer;
  - all 1,216 network messages with their field layouts (the same as our schemas) and the client's handler, and the 127 entity components with factory, constructor, vtable, size and STU class (`net.py`);
  - names: `Eval_STU_x`, `Read_STU_x`, `Msg25005_Handler`, `Comp51_Ctor` and the names from the research.

  Look up: `python -m tools.owindex CD7F3617 0x7FF789BE27C0 Eval_STU_CD7F3617`. In Python: `from tools.owindex import Index; Index.open().stu(...)`, `.addr(...)`, `.messages()`, `.components()`.
- **`ida_game/`**: an IDA database of the game, `research/out/ida_game/ow_game.i64`, built without the IDA window by `python -m tools.ida_game.build` (about 20 min; `--status`, `--phase P`, `--rebuild`, `--detach`). It needs IDA 9.3 (`IDADIR`, default `D:\Program Files\IDA 9.3`) and Rose (`ROSE_BIN`). Phases:
  - `image`: checks the dump and deobfuscates about 6,300 candidate functions with Rose;
  - `create`: IDA's full analysis;
  - `rose`: puts Rose's results into the database;
  - `names`: names from the index and the research;
  - `types`: a structure per STU class. Config evaluators and action runs get `STU_<H> *stu`; state readers and writers get `StateRt_<H> *this`.

  In Python: `from tools.ida_game import open_game_idb`, then `decompile`, `func`, `xrefs_to`, `callees` and `disasm` on an RVA or an address in any known base. Each pseudocode line ends with `// @<address>`. One process at a time can open it; nothing is saved unless `save=True`.
- **`stu.py <class, address or name>`**: everything about one STU class:
  - its fields and what the network does with each;
  - its registrations, and the inherited one the client uses;
  - the code of each of its functions, as pseudocode from the game database (or disassembly), with the class's fields marked;
  - the functions they call;
  - where the game data uses the class (`orpheus script query`).

  Functions that several classes share are marked as shared, not given to the class. About 5 s without the database, 14 s with it.
- **`who_reads.py <class> <field> [--write]`**: the code that reads (or writes) one field, the strongest evidence first: a typed member access, a register known to hold the class, the field's hash as a constant, then matches by offset. It prints the `owdbg.watch` command that checks it in the game.
- **`owdbg/`** (`python -m tools.owdbg.<command>`): a small Windows debugger. Every command starts `orpheus serve` on a temporary copy of the state folder and the game in a window under the debugger, loads the relay, clicks LOGIN after the client's usual first failure, and closes the game and the server at the end. Output goes to `research/out/owdbg/`.
  - `launch`: start and keep running (`--screenshot`).
  - `probe --at SITE`: log each call: arguments, memory (`[rcx+0x18]:8`), return value, caller, `--stack`, `--caller-in`.
  - `watch --addr X`, or `--arm-at SITE --field rcx+0xA8:8`: a hardware watch; lists the instructions and functions that read or write.
  - `watch` or `probe` with `--msg ID --field '#N' --send '{"#N": value}'`: the server sends message ID with that value (the `ORPHEUS_DEBUG_SEND` routes, [troubleshooting](../user/troubleshooting.md#marker-messages)). The watch starts at the client's handler and lists the code that reads the value. Game link messages need a match: `--enter-range --send-after-match` goes into the Practice Range by itself.
  - `cover run --phase NAME:SECONDS[:click=x,y]` and `cover diff`: one-time breakpoints on function starts (12,000 are fine); the functions first reached in each phase, and the difference between two runs.
  - `crash`: registers, the stack with names, disassembly and a minidump on an exception (`testdata/crashme.c` is a test program).

## Pitfalls

- **Index.** The client's STU classes form a tree (descriptor `+0x10` first child, `+0x18` next sibling, `+0x20` parent). The qword `0x7FF78B4F1640` at the start of a descriptor is the image's shared empty string, not a type marker; field entries and message records start with it too. Rebuilding the index while another process has it open fails, and the old index stays.
- Tiny functions (`ret`, `mov al,1; ret`, `mov eax,8; ret`) are shared by unrelated classes. A vtable slot that points to one says nothing about the class.
- In config and condition `evaluate` and action `execute`/`run`, the STU instance is in **r9**; `rcx` is the runtime object. A class's descriptor builder has every field hash as a constant (`mov r8d, <hash>`): that declares the field, it doesn't read it.
- **Messages.** Each protocol has two id switches; only the client dispatcher has handlers. A case that calls `0x1CCC660` (`xor eax,eax; ret`) is a message the client ignores (25016, 25017, all of 429xx). A handler gets the message in **r8** (`handler(rcx = handler object, rdx = context, r8 = message)`); owdbg checks it on each hit with the message's vtable.
- **Database.** Build it with `tools/ida_game/build.py`: research's IDA helpers expect IDA's analysis to be off and delete real functions in a fully analysed database. A killed build leaves `.id0`/`.id1` files; run `--rebuild`. Rose takes `-f` in batches of about 1,000 and runs without a console window.
- **owdbg.** It passes the game's own exceptions on (`C0000096`, `80000004` that isn't its own step, `C000001D`, `C0000005`, foreign `int3`), swallows `406D1388`, and calls a second-chance exception a crash. Debug registers are written 8 s after the start (`--hw-delay`; `0` for start-up). The game and the server run in kill-on-close jobs, because the game keeps its own handle to the debugger. A breakpoint costs about 0.1–0.3 ms; use `--max-hits` on busy functions.
- The client's first login attempt always fails; owdbg clicks LOGIN until the relay connects (25–82 s). Now and then the relay hooks the wrong stream first and that run can't log in; run it again. If the HUD and the weapon are missing after joining a match, run it again (loose ends, "Joining and matches").
- A field watch sees only reads of the message itself. Handlers often copy a field into a local first: use `--linger`, or watch the copy.
