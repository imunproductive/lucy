"""Static STU class dossiers and field-access search (tools/stu.py, tools/who_reads.py).

Modules:
  image     the analysed image (research/out/ida_game/ow_game.exe when it exists, else the research
            repo's 0x7FF788F10000 dump) with
            .pdata function bounds from the index, capstone decoding, strings, rip-relative reference scan
  classes   class resolution (key / address / name), fields incl. inherited, registrations, slot sharing
  code      function listings: Hex-Rays from the game IDB when it can be opened, else capstone; field
            annotation of `[reg+off]` / `(var + off)`; STU-pointer register tracking (arguments, embedded fields)
"""
