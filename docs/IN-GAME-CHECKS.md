# In-game checks (G1-G18)

The automated suites (`docs\DESIGN.md`, section E) run without the game: the addon on a mock
client, the bridge against reference vectors, and the Helper against a stand-in game window. What
only the real client can show is listed here. Each check says what to do, what to expect and what
to report. Commands are typed from the repo root in PowerShell unless they start with `/tpr`
(typed in the WoW chat).

G1-G10 cover the rows under a chat line (inline mode). G11-G18 cover the conversation window and
replies (2026-10-19). The window design numbered its checks G9-G16; they follow the existing ones
here, so code comments that say "G9" to "G16" for the window mean:

| Design number | Here | Topic |
|---|---|---|
| G9 | G11 | client facts (`/dump`) |
| G10 | G12 | paste into the reply box |
| G11 | G13 | pipes, sending, echoes |
| G12 | G14 | chat lockdown and the window |
| G13 | G15 | the window at every scale, tabs |
| G14 | G16 | Battle.net conversations |
| G15 | G17 | settings panel |
| G16 | G18 | clipboard end to end |

## 0. One-time setup

1. **No restart needed:** `/reload` picks up the added and removed addon Lua files (measured on
   WoW Forever on 2026-10-18).
2. **Remove the old model packs** left in the AddOns folder by the Lua translator
   (`TranslateProbe_deen`, `_enfr`, `_esen`, `_koen`, `_ruen`, `_zhen`, about 194 MB). This moves
   them to the Recycle Bin (it reads the AddOns folder from the gitignored
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
   It first copies the saved variables (`WTF\Account\<account>\SavedVariables\TranslateProbe.lua`)
   to `%LOCALAPPDATA%\TranslateProbe\savedvariables-backup\`. Since addon 0.3.0 the glossary may
   hold meanings per language (schema 3), which a build before it drops at load: to roll back,
   close the game, deploy the old build and copy the backup over the saved variables file.
4. **Build the Helper** (offline, about a minute the first time):

   ```powershell
   scripts\helper.ps1 -Build
   ```

   It prints the exe size (about 1.2 MB). Stop a running Helper first (its icon, Exit, or
   `taskkill /IM TranslateProbeHelper.exe`): the running exe locks the file. The translator
   runtime folder is read from `$LlamaRuntimeDir` in `scripts\dev.local.ps1` (or `-RuntimeDir`,
   or the environment variable `TPROBE_LLAMA_RUNTIME`); the model is found through the Ollama store
   (`hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`). `TranslateProbeHelper.exe --selftest` (in
   `helper\app\target\release`) prints what it finds without loading the model.
5. **Start the game** in Windowed or Windowed (Fullscreen) mode, log in, and type `/tpr info`.
   Expect the version of the build under test with `(build 3)` and `TransformMessages yes,
   FontStringContainer yes` for each chat frame. No Lua error.
   - **[T] opens the conversation window by default** since 2026-10-19 (Ctrl-click [T] draws the
     rows under the line). For G1-G10, type `/tpr display inline` first, and `/tpr display window`
     before G11.
   - **The glossary row is off by default** since saved variables schema 4: an existing
     "Explain abbreviations" that was on is turned off once (it could not be told apart from the
     old default). `/tpr show on` (or the option) brings it back. G2 and G10 need it on.
6. **Start the Helper with diagnostics:**

   ```powershell
   scripts\helper.ps1 -Run -Diag
   ```

   It prints the path of the diagnostics file (`%LOCALAPPDATA%\TranslateProbe\logs\diag-<time>.jsonl`)
   and puts an icon in the notification area. The log is
   `%LOCALAPPDATA%\TranslateProbe\logs\helper.log`. Neither file ever holds chat text. To stop
   the Helper: right-click its icon, Exit, or `taskkill /IM TranslateProbeHelper.exe` (no `/F`
   needed since Helper 0.2.0). A second copy refuses to start (exit code 2). A Helper older than
   the addon shows "this Helper is older than the addon" once per session (and the other way
   round): addon build 3 sends replies (reply and copy flags), which a build-2 Helper drops, and
   build 2 sent up to 6 rows, which an older Helper ignores.

## G1 Bridge and capture

1. With WoW in front, `/tpr test`.
2. **Expect:** a strip of coloured dots flickers for about half a second in the top-left corner;
   a toast "TranslateProbe Helper: bridge OK (dense, margin NN, WGC 30 Hz)" for 4 s near the
   top-left of the game; a sample line with a [T] and reserved rows under it; within a few
   seconds (CPU) or about a second (GPU) the sample's translation replaces the dark rows (see
   G10 for how it should look). The first
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
   visible); open the bags; go through a loading screen.
3. **Alt-Tab** (user decision D12, 2026-10-19): Alt-Tab to a browser, then move the browser so it
   covers only half of the chat; then minimize WoW; then bring WoW back.
4. **Expect:**
   - the translation follows its rows or hides every time, and never paints over a menu, a
     tooltip or another window; a card is never one frame behind for long when scrolling;
   - after Alt-Tab, while WoW is still visible behind or beside the other window, the
     translations stay on the visible part of WoW: no raw colour tags, no dark plates, and
     nothing drawn over the browser (the part under the browser is simply hidden);
   - minimized WoW: nothing shown anywhere; back in front: the translations come back.
5. **While WoW is in the background** (conversation window open, `follow` smart, the default):
   have a friend whisper you while you are Alt-Tabbed with WoW still visible beside the other
   window. **Expect:** the new bubble is translated without bringing WoW back (the Helper reads the
   strip in the background too since 2026-10-19), never "Not received", never another message's
   translation.
6. **Report:** any case where a card stayed on top of something, lagged, did not come back, or
   where tags and plates showed after Alt-Tab.

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

## G9 Options panel

1. `/tpr` (or Esc > Options > AddOns > TranslateProbe).
2. **Expect:** the addon's page with "Target language", "Explain abbreviations", "Spell names in
   Latin letters" and "Bridge mode (advanced)"; no Lua error. In the target list every language
   shows in its own name and script: English, Français, Deutsch, Español, Italiano, Português,
   Русский, 한국어, 简体中文, 繁體中文 (report any that shows as boxes or question marks).
3. Pick another target (for example Deutsch), close the panel, `/tpr info`: the target follows.
   `/tpr target fr` then reopen the panel: the dropdown shows Français.
4. Open the **Glossary** subpage:
   - the title names the target ("Glossary: meanings in Français"); 12 rows, Previous / Next and
     the mouse wheel scroll; the page fits the settings panel without overlapping its buttons;
   - type a meaning in a built-in row (LFM), press Enter: it is marked "edited"; clear it: the
     built-in meaning comes back;
   - add an abbreviation with a meaning (bottom row, Add); switch the target on the main page and
     come back: the abbreviation is still listed, its meaning empty for the new language;
   - type in a row and, without pressing Enter, click another row's Delete/Disable or Add: the
     typed text is kept for its own row (check it after the click);
   - Disable a built-in, Enable it again; Delete your test entry.
5. `/tpr gloss list`: your entries with their meaning in the current target.
6. **Report:** anything that does not fit or overlaps, and any Lua error (in combat too: open the
   panel during a fight once).

## G10 Native look

1. With the Helper running (without `-Diag`: diagnostics draw yellow and cyan outlines on
   purpose), click [T] on a long line in another language (a German line with target French gives
   4 to 6 rows).
2. **Expect:** while translating, a grey "Translating..." in the first row; then the translation
   in the chat font (Arial Narrow) at the chat's size, light blue with a black shadow, directly on
   the chat background: no navy slab, no dark square where the tag was, no outline. Accents and
   descenders (É, à, g, p, q, y) are not clipped. A line long enough for 5 or 6 rows is no longer
   cut with "..." after 4 rows. Text shrinks a little before a last line is cut with "...".
3. Then: move a window or a damage meter over the right part of the chat, over a translation;
   restart the Helper while WoW stays open and click [T] on a new line; let the line fade.
4. **Expect:** the visible part of the translation keeps the chat colour (no colour streak
   towards the window over it); after a Helper restart the rows look the same as before. A known
   limit: when a fading line drops under 30% opacity the translation is removed, and its faint
   plate can show for the rest of the fade.
5. **Report:** a screenshot of a translated line next to normal chat lines, on a plain chat
   background and over a busy game world (city, forest), and anything that looks like a slab, a
   square or a streak.
6. **Glossary row** (user decision D11): `/tpr show on`, then Ctrl-click [T] on a line with
   abbreviations ("LF tank, PST"). **Expect:** the "Glossary: ..." row under the translation in its
   own muted colour, clearly apart from the light blue of the translation and of [T], readable on
   the chat background. `/tpr show off` removes it from the next click. **Report:** a screenshot.

## G11 Client facts for the window (`/dump`)

Type each line in the chat and copy the answers (a value, `nil`, or an error).

1. Templates and art:
   `/dump C_XMLUtil.GetTemplateInfo("DialogBorderDarkTemplate")`,
   `/dump C_XMLUtil.GetTemplateInfo("TabSystemTopButtonTemplate")`,
   `/dump C_XMLUtil.GetTemplateInfo("PanelTopTabButtonTemplate")`,
   `/dump C_Texture.GetAtlasInfo("ChatBubble-Tail")`, `/dump C_Texture.GetAtlasInfo("checkmark-minimal")`.
2. Strings and colours (on a French client too): `/dump SAY, YELL, WHISPER, TRADE, PARTY, GUILD`,
   `/dump INSTANCE_CHAT, OFFICER, ERR_CHAT_PLAYER_NOT_FOUND_S`,
   `/dump ORANGE_FONT_COLOR, GREEN_FONT_COLOR`.
3. Helpers the window and the settings use:
   `/dump GameTooltip_AddInstructionLine ~= nil, GameTooltip_AddErrorLine ~= nil, GameTooltip_SetTitle ~= nil`,
   `/dump Settings.CreateSlider ~= nil, Settings.CreateSliderOptions ~= nil, MinimalSliderWithSteppersMixin ~= nil`,
   `/dump CreateSettingsButtonInitializer ~= nil, CreateSettingsListSectionHeaderInitializer ~= nil`.
4. **Expect:** every template and atlas found, French names in French ("Dire", "Crier",
   "Chuchoter", "Commerce"), every helper `true`. Without `DialogBorderDarkTemplate` the consent
   dialog falls back to a tooltip border; without their API the settings section leaves out its
   sliders, its header or its reset button (no error).

## G12 Paste into the reply box (design G10)

1. `/tpr display window`, click [T] on a line in another language (German, for example): the
   conversation window opens. Type `/tpr test paste` (the reply box probe: it prints counts only,
   never the text), then click into the reply box of the window.
2. In a text editor outside WoW, copy, one at a time: `Grüße, ça va ? Straße`, a Cyrillic word
   (`привет`), a Hangul word (`안녕`), a text longer than 255 bytes, and two lines with a line break.
   Paste each into the reply box with Ctrl+V; also type a few letters by hand.
3. **Expect:** for each Ctrl+V one "OnTextChanged" line with "typed by you yes, Ctrl down yes" and
   no "OnChar" line; typed letters give "OnTextChanged" then "OnChar" (in that order). Accents,
   Cyrillic and Hangul appear intact in the box; the byte count turns red above 255 and the send
   buttons grey out ("Too long by N bytes"); the two lines become the first line only, with
   "Only the first line was kept".
4. Then the real flow: write a reply in your language, press Enter (Translate). In WAITING, press
   Ctrl+V: the stepper shows "2 Pasted" in green.
5. `/tpr test paste` again to stop the probe. **Report:** the probe lines (they hold numbers only)
   and anything that differs.

## G13 Pipes, sending and echoes (design G11)

1. With the probe on (`/tpr test paste`), type `a|b` in the reply box, then paste `a|b` copied from
   a text editor. **Report:** "single pipes" and "doubled pipes" for both (the addon escapes pipes
   the same way whatever the box returns).
2. Send a short text from each button, outdoors (not in an instance): a Trade or General line
   (default button "2. Trade"), Say, Yell, a whisper to a player of another realm (open the
   conversation from their whisper), and, in a group or guild, Party and Guild.
3. **Expect:** each click sends once (a quick double click sends once); no "action blocked"
   message; my bubble shows "Sending..." then "Sent" with a check within a second or two; the text
   in the chat and in my bubble is exactly the text of the box (`a|b` shows as `a|b`). Whisper an
   offline name (open a conversation, wait until they log off): "Not delivered: <name> is offline".
4. **Zone channels** (review fix of 2026-10-19: a channel button never sends to another channel).
   Before a General or LocalDefense line arrives, paste this in the chat (it prints the channel
   fields of the next channel message once: no text, no name):
   `/run local f=CreateFrame("Frame") f:RegisterEvent("CHAT_MSG_CHANNEL") f:SetScript("OnEvent",function(s,_,_,_,_,_,_,_,z,i,b) print("zone id",z,"number",i,"base",b,"found",(GetChannelName(b))) s:UnregisterAllEvents() end)`
   Then click [T] on a General line, ride to another zone and open that conversation again.
   **Expect:** in the zone of the line, the default button "1. General" sends there; in the other
   zone it is greyed with "You left this channel" (it never sends to the new zone's General).
5. **Report:** the `/run` line's output ("base" is it "General - <zone>" or "General"? "found" is
   it the channel number or 0?), any "Not confirmed" for a message that did arrive (the echo text
   may differ from the sent text: then tell which characters changed), and any Lua error.

## G14 Chat lockdown and the window (design G12)

1. `/console addonChatRestrictionsForced 1` (or a real boss with restrictions), with the window
   open and a reply translated and pasted.
2. **Expect:** the stepper in red: "Chat is locked. Press Enter, then Ctrl+V, then Enter in the
   normal chat box."; every send button greyed with the reason in its tooltip; no "action blocked"
   error; Translate still works on a new draft; the clipboard still holds the translation (Ctrl+V
   in the normal chat box pastes it).
3. `/console addonChatRestrictionsForced 0` (or the end of the fight): the buttons come back and a
   click sends.
4. **Report:** anything that was sent during the lockdown, or a button that stayed grey after it.

## G15 The window at every scale, tabs (design G13)

1. UI scale at 0.64, then 1.0; window scale 80 % and 150 % (Options > AddOns > TranslateProbe,
   "Window scale"); text size 12 and 20. After each change, click [T] on two lines (short and
   long) and translate a reply.
2. **Expect:** every bubble's tags read (translations appear); the translated text in the
   Helper's light blue (#A8D8FF), on a seamless bubble fill (no dark slab, no square where the
   tag was); a bubble scrolled half out of the thread shows no half row, at the top and the
   bottom; a tooltip over a bubble or over the reply card's lane is never painted over, nor an
   always-on-top window (Task Manager with "Always on top") placed over the lane row only; the
   window over a chat frame gives no double translation. The ChatBubble border tint looks right
   by eye.
3. **Popups and bubbles:** open the language popup ("DE v"), then right-click a bubble while it is
   open, then press Escape once. **Expect:** the first Escape closes only the popup, the second
   the window. Start the Helper after a [T] click (exit it, click [T], start it): the bubble says
   "Not received - click the bubble"; a click translates it, and a second click asks again too.
4. **Tabs** (user decision D10): open 5 or more conversations. **Expect:** a small close cross at
   the right of the selected tab and of a hovered tab only; a click on the cross, or a
   middle-click on a tab, closes it (right-click still opens its menu); a later [T] on a line of
   a closed sender reopens the tab with its earlier messages; at most 10 tabs show, the others
   behind "+N". After `/reload` the conversations are gone (memory only).
5. **Report:** a screenshot at the smallest and the largest scale, and any half row, slab or
   misplaced cross.

## G16 Battle.net conversations (design G14)

1. Click [T] on a Battle.net whisper from a friend.
2. **Expect:** the title and the tab show the friend's name as the chat shows it; the portrait is
   the question mark; the only send button is Whisper; a reply arrives as a Battle.net whisper
   and the bubble shows "Sent".
3. **Report:** a screenshot of the title and the tab.

## G17 Settings panel (design G15)

1. Esc > Options > AddOns > TranslateProbe.
2. **Expect:** the round-3 controls (target language and bridge mode dropdowns, two checkboxes)
   work without a Lua error (SkillUp records a Forever crash in the native dropdown: report it
   if it happens). Below them, "Conversation window": the [T] checkbox, "Translate new messages"
   and "Copy translated replies" as stepped sliders with a label (no dropdown), the original
   checkbox, the text size and window scale sliders, three more checkboxes, and "Window
   position" with a reset button that puts the open window back at its default place.
3. Set "Copy translated replies" to Ask, then `/tpr clip allow`: the slider follows (Allow).
4. **Report:** anything that overlaps, any missing control, any Lua error (open the panel once in
   combat too).

## G18 Clipboard end to end (design G16)

The automated runs never touch the clipboard (`--no-clipboard`); one guarded live test of the
Helper's clipboard writer runs outside the game. The whole flow is checked here.

1. Copy a harmless word in a text editor. In WoW, open a conversation, write a reply in your
   language, click "Translate to XX". **Expect:** the dialog "Copy translated replies to the
   clipboard?" with "Remember my choice" unticked; Enter decides nothing (it may open the normal
   chat box, as without the dialog); Escape closes it as Deny (this time only). With the dialog
   open, start a fight (a training dummy): movement and ability keys still work. Close the window
   with its X while the dialog is open: the dialog closes too and nothing is translated.
2. Translate again and click Allow. **Expect:** at once, Ctrl+V in a text editor pastes nothing
   (the Helper emptied the clipboard on receipt); in the reply card, grey "Translating...", then
   green "Ready - Ctrl+V to paste"; Ctrl+V in the reply box pastes the translation and the stepper
   shows "2 Pasted"; a click on the default channel sends it.
3. Win+V (clipboard history, when it is on in Windows): the translation is **not** in the history.
4. Translate again, then copy something else in another program before the green line: the card
   says "Not copied - you copied something else. Click Translate again", and your copy is kept.
5. Deny with "Remember my choice": the next Translate gives no dialog and "Ready (not copied)";
   `/tpr clip ask` brings the dialog back.
6. Helper tray menu: untick "Copy translated replies to the clipboard": "Ready (not copied - off
   in the Helper menu)". Tick "Sound when a reply is copied": a short beep on each copy.
7. **Report:** each state seen (a screenshot of the green line and of the dialog), whether
   anything old was ever pasted, and the Win+V result.

## What to send back

- The printouts of `/tpr test rows`, `/tpr test lockdown` and `/tpr test paste` (G12, G13), and
  the `/dump` answers of G11.
- The diagnostics files (`diag-*.jsonl`) and `helper.log` from
  `%LOCALAPPDATA%\TranslateProbe\logs` (numbers only, no chat text).
- A screenshot of anything that looks wrong.
