# In-game checks (G1-G8)

The automated suites (`docs\DESIGN.md`, section E) run without the game: the addon on a mock
client, the bridge against reference vectors, and the Helper against a stand-in game window. What
only the real client can show is listed here. Each check says what to do, what to expect and what
to report. Commands are typed from the repo root in PowerShell unless they start with `/tpr`
(typed in the WoW chat).

## 0. One-time setup

1. **Close the game completely** (exit to desktop, not `/reload`): this version adds and removes
   addon files, which the client only picks up at a full start.
2. **Remove the old model packs** left in the AddOns folder by the Lua translator
   (`TranslateProbe_deen`, `_enfr`, `_esen`, `_koen`, `_ruen`, `_zhen`, about 194 MB). With the
   game closed, this moves them to the Recycle Bin (it reads the AddOns folder from the gitignored
   `scripts\dev.local.ps1`):

   ```powershell
   . .\scripts\dev.local.ps1
   Add-Type -AssemblyName Microsoft.VisualBasic
   foreach ($root in $WowAddOnsPaths) {
       Get-ChildItem -LiteralPath $root -Directory -Filter 'TranslateProbe_*' | ForEach-Object {
           [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($_.FullName, 'OnlyErrorDialogs', 'SendToRecycleBin')
       }
   }
   ```

3. **Deploy the addon:** `dev.cmd` (it mirrors `addon\TranslateProbe` into the AddOns folder).
4. **Build the Helper** (offline, about a minute the first time):

   ```powershell
   scripts\helper.ps1 -Build
   ```

   It prints the exe size (about 1.1 MB). The translator runtime folder is read from
   `$LlamaRuntimeDir` in `scripts\dev.local.ps1` (or `-RuntimeDir`, or the environment variable
   `TPROBE_LLAMA_RUNTIME`); the model is found through the Ollama store
   (`hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`). `TranslateProbeHelper.exe --selftest` (in
   `helper\app\target\release`) prints what it finds without loading the model.
5. **Start the game** in Windowed or Windowed (Fullscreen) mode, log in, and type `/tpr info`.
   Expect `version 0.2.0 (build 1)` and `TransformMessages yes, FontStringContainer yes` for each
   chat frame. No Lua error.
6. **Start the Helper with diagnostics:**

   ```powershell
   scripts\helper.ps1 -Run -Diag
   ```

   It prints the path of the diagnostics file (`%LOCALAPPDATA%\TranslateProbe\logs\diag-<time>.jsonl`)
   and puts an icon in the notification area. The log is
   `%LOCALAPPDATA%\TranslateProbe\logs\helper.log`. Neither file ever holds chat text. To stop
   the Helper: right-click its icon, Exit (a second copy refuses to start, exit code 2).

## G1 Bridge and capture

1. With WoW in front, `/tpr test`.
2. **Expect:** a strip of coloured dots flickers for about half a second in the top-left corner;
   a toast "TranslateProbe Helper: bridge OK (dense, margin NN, WGC 30 Hz)" for 4 s near the
   top-left of the game; a sample line with a [T] and reserved rows under it; within a few
   seconds (CPU) or about a second (GPU) the sample is translated on the dark rows. The first
   start of the translator shows "Starting the translator..." (or "Preparing the GPU, first time
   only (up to a minute)..." on the first GPU run).
3. Exit the Helper from its icon, then repeat with GDI capture:

   ```powershell
   scripts\helper.ps1 -Run -Diag --capture gdi
   ```

4. **Report:** the toast numbers of both runs (mode, margin, capture, Hz), how long the first
   translation took, your in-game Gamma, Contrast and Brightness settings, and the two
   diagnostics files (a `tone` record in them means the Helper measured a display curve on the
   strip and corrects it).

## G2 Reserved rows

1. `/tpr test rows` and copy the printout (metrics, string heights).
2. **Expect:** a test line with 3 rows under it; with the Helper running each row reads
   "Overlay OK (row N, block P px)" and the colour tags at the start of the rows are painted over.
3. **Report:**
   - whether the rows wrap, whether the tag blocks look tinted by the line colour, any gap or
     shadow between rows;
   - whether a tooltip appears when hovering [T] and when hovering a reserved row (and disappears
     when leaving it);
   - whether clicking a reserved row hides the rows.

## G3 Live use

1. Click [T] on 3 real lines (short, long, another language).
2. Then, one at a time: scroll the chat; let new messages push the lines up; switch chat tabs;
   move and resize the chat window; wait more than 2 minutes for the fade; Alt-Z; open the world
   map; the Esc menu; right-click a player name over a translation (the menu must stay fully
   visible); open the bags; go through a loading screen; alt-tab to a browser over WoW.
3. **Expect:** the translation follows its rows or hides every time, and never paints over a
   menu, a tooltip or another window. A card is never one frame behind for long when scrolling.
4. **Report:** any case where a card stayed on top of something, lagged, or did not come back.

## G4 Chat restriction (a dungeon boss)

1. Before the pull, click [T] on a line from before the fight.
2. During the fight: `/tpr test lockdown` and copy the printout.
3. After the fight: one line "N messages arrived during the restriction. [Show them]" appears
   when lines arrived while chat was locked. Click [Show them], then [T] on one reprinted line.
4. **Expect:** the pre-fight line is translated during the fight (or waits with "Waiting for the
   end of the restriction..." and translates after it); the reprinted lines carry a [T] that
   works; lines from channels no chat tab shows are not reprinted.
5. **Report:** the `/tpr test lockdown` printout and whether the strip and the cards showed
   during the fight.

## G5 Performance

1. Show the FPS (Ctrl+R). Note it with no translation shown, with 3 translations shown, and while
   a translation runs on the GPU.
2. In a busy city, run `nvidia-smi` while translating (free VRAM; the Helper switches to CPU below
   700 MB free).
3. **Report:** the three FPS values and the free VRAM; the `perf` records of the diagnostics file
   give the Helper's own CPU use.

## G6 Scales and fonts

1. UI scale at its minimum, then at its maximum; chat font size 12, 14 and 18. After each change,
   `/tpr test` and `/tpr test rows`.
2. **Expect:** the bridge toast and the "Overlay OK" rows at every setting.
3. **Report:** any setting where the rows or the toast fail, with the `/tpr test rows` printout.

## G7 Reload

1. With translations on screen, `/reload`.
2. **Expect:** the old reserved rows are removed at login, no Lua error; new clicks work.

## G8 Colourblind filter (optional)

1. Turn the colourblind filter on in the game settings (Accessibility).
2. `/tpr info` should report bridge mode `mono` while the mode is `auto` (it follows the setting
   at once; `/tpr mode auto` also re-reads it). `/tpr test` must still work.
3. Turn the filter off: the mode goes back to `dense`.

## What to send back

- The printouts of `/tpr test rows` and `/tpr test lockdown`.
- The diagnostics files (`diag-*.jsonl`) and `helper.log` from
  `%LOCALAPPDATA%\TranslateProbe\logs` (numbers only, no chat text).
- A screenshot of anything that looks wrong.
