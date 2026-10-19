# Addon (Lua side of the exe route)

The addon no longer translates. A [T] click sends a TRANSLATE request over the pixel bridge
(`docs\BRIDGE.md`) and TranslateProbe Helper draws the translation. Since 2026-10-19 the click
opens the line in the **conversation window** (one tab per sender, bubbles the Helper paints, a
reply composer); Ctrl-click keeps the first route, **inline**: rows reserved under the chat line
(a colour tag and a dark plate per row). Modules, in TOC order:

| File | Role |
|---|---|
| `Locales.lua` | `ns.L`, English base and frFR override |
| `ConvLocales.lua` | the conversation window's strings (en, frFR) and the key binding names |
| `ReplyLocales.lua` | the reply composer's strings (en, frFR) |
| `Core.lua` | saved variables (whitelist purge, schema 4), session id, print helpers, inits under `pcall` |
| `Bridge.lua` | pixel strip encoder, message builders, tag and plate markup (bridge package) |
| `Glossary.lua` | abbreviations: glossary row (`Describe`), prompt pairs (`Extract`), marked text (`PromptText`) |
| `Lang.lua` | targets, default target, script of a line |
| `LangData.lua` | character trigram profiles of en, de, fr, es, it, pt (restored from 8b7750e, `tools\make_langid.py`) |
| `LangDetect.lua` | the sender's language: writing system, trigrams, chat words, the "unknown" rule, votes |
| `Reserve.lua` | row metrics, row count estimate, reserved rows, slots (chat and window), VIEW watcher, window metrics, old-session cleanup |
| `ChatButton.lua` | the [T] link and its routing, rows under a line (`SetSuffix`), line details (`LineInfo`), link clicks and tooltips |
| `Request.lua` | the inline click flow and active requests; window bubbles' TRANSLATE (`StartWindow`, `DropWindow`) |
| `Restriction.lua` | chat restrictions: deferred clicks, lines that arrived secret, [Show them] |
| `Popup.lua` | the window's own popup menu (no Blizzard menu) |
| `Conversations.lua` | `ns.Conv`: conversations, messages, following, closed tabs, memory bounds (memory only) |
| `Bubble.lua` | bubbles (incoming, mine), the reply card, `TranslateProbeConvFont` |
| `Channels.lua` | the reply's channels and sending (work package R) |
| `Consent.lua` | the in-game clipboard consent dialog (work package R) |
| `Composer.lua` | the reply composer in `ConvFrame.composerHost` (work package R) |
| `ConvFrame.lua` | the window: tabs, header, thread, slot manager, per-row visibility, VIEW 200, position, key binding functions |
| `Commands.lua` | `/tpr` (`/tprobe`) commands |
| `Options.lua` | the options panel (Esc > Options > AddOns > TranslateProbe) and its glossary page |

`Bindings.xml` (loaded by the client from the addon folder, not through the TOC) declares the two
key bindings, unbound by default: "Show/hide the conversation window" (`TranslateProbe_ConvToggle`)
and "Reply in the conversation window" (`TranslateProbe_ConvReply`: shows it and focuses the reply
box).

Saved variables keep only `schema`, `target`, `names`, `glossShow`, `mode`, `glossary`,
`helpShown` and the conversation window's settings (below). Message text, sender names, GUIDs,
Battle.net ids, line ids, request ids, slots and conversations are never stored. Any other key is
deleted at load; an old `display = "tab"` (the dropped chat tab mode, decision 20) becomes the
default `"window"`.

### User glossary (`TranslateProbeDB.glossary[key]`)

`key` is a lowercase whole word (`Glossary.KEY_PATTERN`, accents allowed). The value is one of:

| Value | Meaning |
|---|---|
| `false` | a built-in key the user disabled (in the glossary row and in the prompt) |
| `"text"` | one meaning for every target: the format before per-target meanings, still honoured as it is |
| `{ fr = "...", de = "...", ["*"] = "..." }` | a meaning per target; `"*"` is an old every-target string kept for the other targets once one target gets its own; `""` for a target hides the `"*"` meaning there only |
| `{}` | a user abbreviation with no meaning yet (shown empty, skipped in prompts) |

Each meaning is at most 120 bytes (cut on a UTF-8 boundary, pipes become "/"); the purge drops
longer strings and unknown targets inside a table, drops an empty marker when there is no `"*"`
meaning, and drops any other value. Nothing is migrated: an old string stays a string until the
user edits it for one target. `Glossary.Extract`, `Describe` and `Entries` read a value through
`Glossary.UserMeaning(value, target)` (the target's meaning, else `"*"`, else none; `""` is none);
a user key without a meaning for the target is skipped there, a built-in key falls back to its
built-in meaning. `/tpr gloss add` and the panel both write through `Glossary.SetMeaning` (current
target only), `Glossary.Remove` and `Glossary.Enable`.

Clearing a meaning (an empty text) acts on the current target only:
- the target has its own meaning: it is removed, and an every-target (`"*"`) meaning, if any,
  shows again (so an old string comes back unchanged once its edits are cleared);
- the target shows the every-target meaning: an empty marker (`""`) hides it for this target,
  while the other targets keep it (review fix, 2026-10-18: it used to delete it for every
  target).

Built-in meanings are bilingual data (en + fr). The prompt gets the French one for target `fr` and
the English one (a pivot the model reads) for every other target; the panel shows that English
meaning marked "in English" so the user can type their own for that language.

## Options panel

`/tpr` alone and `/tpr options` open it; `/tpr help` prints the commands. Built with the 12.x
Settings API, checked on 2026-10-18 against Gethe/wow-ui-source, branch `live` (line numbers are
approximate: they were read through a page summary, not a local checkout):

| Call | Where |
|---|---|
| `Settings.VarType` (`Boolean`, `String`, `Number`) | `Interface\AddOns\Blizzard_Settings_Shared\Blizzard_Settings.lua:11` |
| `Settings.RegisterAddOnCategory(category)` | same file `:131` |
| `Settings.OpenToCategory(categoryID, scrollToElementName)` | same file `:141` |
| `Settings.RegisterVerticalLayoutCategory(name)` | same file `:151` (returns category, layout) |
| `Settings.RegisterCanvasLayoutSubcategory(parentCategory, frame, name)` | same file `:163` |
| `Settings.RegisterProxySetting(category, variable, variableType, name, default, getValue, setValue)` | same file `:175` |
| `Settings.GetSetting(variable)`, `Settings.NotifyUpdate(variable)` | same file `:199`, `:203` |
| `Settings.CreateControlTextContainer()`, `container:Add(value, label, tooltip)`, `:GetData()` | same file `:228-262` |
| `Settings.CreateCheckbox(category, setting, tooltip)` | same file `:385` |
| `Settings.CreateDropdown(category, setting, options, tooltip)` (`options` is a function returning `GetData()`) | same file `:407` |
| `SettingsCategoryMixin:GetID()` | `Blizzard_Settings_Shared\Blizzard_Category.lua:13` |
| canvas page shown: reparented, `SetAllPoints`, `Show`, then `frame:OnRefresh()` (`CallRefreshOnFrame`); left: `SetParent(nil)`, `Hide` | `Blizzard_Settings_Shared\Blizzard_SettingsPanel.lua:8` and `DisplayLayout` |
| settings calls refuse function arguments (`SecureUnpackArgs`), proxy accessors are wrapped | `Blizzard_Settings_Shared\Blizzard_SettingsInbound.lua:21`, `:50` |
| `InputBoxTemplate` (its `OnEditFocusLost` clears the highlight, so ours is hooked, not set) | `Blizzard_SharedXML\Shared\InputBox\InputBoxTemplates.xml:71` |
| `UIPanelButtonTemplate` | `Blizzard_SharedXML\Mainline\SharedUIPanelTemplates.xml:365` |

- **Main page** (vertical layout): target language dropdown (each language named in its own
  language: `Lang.NATIVE`), glossary row and romanized names checkboxes, bridge mode dropdown
  (advanced). Every control is a proxy setting on `TranslateProbeDB`; the commands go through the
  same setters (`Options.SetTarget`, `Options.SetMode`) and call `Options.Changed(key)`, which
  runs `Settings.NotifyUpdate` so an open panel shows the new value.
- **Glossary page** (canvas subcategory, built the first time it is shown): 12 rows at a time
  (Previous/Next buttons, mouse wheel), user keys first, then the built-in ones. Each row: the
  abbreviation, a mark (yours, edited, all languages, in English, disabled), the meaning for the
  current target in an edit box (Enter or leaving the box saves, Escape restores, an empty text
  removes the meaning for this target), and Delete (user key), Disable or Enable (built-in key).
  An add row takes an abbreviation (one word) and its meaning for the current target. Changing
  the target refreshes the page: the abbreviations stay, the meanings follow.
- A click on a row button or on Add first saves the text being typed in a row (a WoW button
  click does not take the keyboard focus, so the box would keep it while the rows are rebound to
  other entries, and the text would be lost or saved to another abbreviation; review fix).
- `/tpr gloss list` shows each user entry with what the current target gets: its user meaning,
  else the built-in meaning of a built-in key, else "(no meaning in XX)".
- Without the Settings API nothing is registered and `/tpr` prints `OPTIONS_NA` and the commands.

## Conversation window

Built from the conversation design of 2026-10-19 (work packages P, A, R, H) and the user's
decisions D1-D12. This section covers work package A (the window); the reply composer, channels
and consent are work package R (`Composer.lua`, `Channels.lua`, `Consent.lua`).

### Opening (D4)

| Click on [T] | `display = "window"` (default) | `display = "inline"` |
|---|---|---|
| click | the conversation window | rows under the line |
| Ctrl-click | rows under the line | the conversation window |
| Shift held | translate again without the cache, in both modes | same |

`ChatButton.WindowMode(ctrl)` decides; the [T] tooltip names the other mode
(`CONV_TIP_T_WINDOW`, `CONV_TIP_T_INLINE`). `ConvFrame.OpenFromLine(lineID, noCache)`:
1. `Conv.FromLine` finds or creates the sender's conversation (key: GUID, else `"bn:"` .. the
   Battle.net account id, else the full name) from `ChatButton.LineInfo` (event, sender, channel
   index and base name, GUID, Battle.net id; memory only; a Battle.net name is never kept) and
   `ChatButton.SourceText`. A closed conversation comes back with its history.
2. The line is added, or found again (no second bubble; its meta line is highlighted for 1.5 s,
   never the rows, so the Helper keeps reading the tags).
3. The window is shown and raised, the tab selected, the bubble scrolled into view. The keyboard
   focus is never taken unless `convFocus` is on. It also works in combat (an explicit click).
4. The slot manager gives the bubble a slot and sends its TRANSLATE (a VIEW first) before its rows
   are shown.
During a chat lockdown a line whose text is not in memory is deferred like an inline click
(`Restriction.Defer`). A line the client no longer has prints `CONV_TOO_OLD`.

### Layout

`TranslateProbeConvFrame`: `PortraitFrameTemplate`, 440 x 540 (380 x 440 to 760 x 900, resized with
`PanelResizeButtonTemplate`), first at `CENTER (-300, 60)`, strata HIGH, toplevel, clamped with the
top 16 px kept free, moved by its title bar, `SetDontSavePosition(true)`, in `UISpecialFrames`
(Escape hides it; the X hides it directly, since `HideUIPanel` does nothing for an addon frame in
combat). Window scale `convScale`, text size `convText` (the font `TranslateProbeConvFont`, a copy
of `ChatFontNormal`). Sounds on show, hide and tab clicks.

- **Title and portrait:** the full name-realm (a Battle.net account name shown, never parsed), the
  romanized name in parentheses when `names` is on; the class icon from `GetPlayerInfoByGUID`, else
  the question mark.
- **Tabs** (`PanelTopTabButtonTemplate` on a strip, `PanelTemplates_*`, both read in the Forever
  source): see below.
- **Header:** "DE v" (the sender's language; a click opens "They write in:" with "Automatic
  (Deutsch)" and each target in its own name, a pick sticks for the conversation), ">", the target
  ("Your language. Change it in Options."), "via 2. Trade" (the sender's latest channel in its
  chat colour), and "Translate new messages" (the tab's own follow choice).
- **Thread:** `InsetFrameTemplate`, a plain `ScrollFrame` with `MinimalScrollBar`
  (`ScrollUtil.InitScrollFrameWithScrollBar`), wheel step 3 rows. A banner during a chat
  restriction (the thread starts under it), "N new messages" when messages arrive while scrolled
  up (a click scrolls to the end), and "Click [T] after a chat line to start a conversation." in an
  empty window.
- **`ConvFrame.composerHost`:** full width, bottom-anchored; its height is `Composer.Height()` and
  the thread ends 6 units above it.

### Tabs (D10)

- Opening order, new ones on the right; a message never moves a tab. Up to **10 tabs are shown**,
  as many as fit (the selected one at its widest, 130, the others at their narrowest, 72: 3 at
  440 wide, 7 at 760); the others are behind "+N", whose popup lists them unread first, then by
  last activity. The selected conversation is always shown: picked from "+N" (or opened by [T]
  beyond the strip), it takes the place of the rightmost shown tab.
- Label: the short name (with the realm when two shown tabs share it) and " (N)" in white while
  unread, with a static locked highlight. Tooltip: full name, class, last channel, "Middle-click
  to close. Right-click for options."
- **Closing:** a small close cross at the tab's right, shown on the selected tab and on hover
  (its own child button: the selected tab is disabled by `PanelTemplates_SelectTab` and never gets
  `OnClick`); a middle-click (read in `OnMouseUp`, which a disabled button still gets); the
  right-click popup ("Close", "Close the others", "Translate new messages"). Closing the last tab
  hides the window; the window's X only hides it.
- **Closing keeps the history:** the conversation stays in memory and its bubbles give their slots
  back. A later message of that sender that is translated (a [T] click, or a followed line in
  translate mode) reopens the tab on the right with the whole history; a followed line in add mode
  joins the history and the tab stays closed.
- **Memory bounds:** 30 conversations (open and closed), 100 messages each (the oldest goes, with
  its slot). The 31st evicts the least recently active closed conversation first, then the least
  recently active open one that has no unread message, no pending reply (`conv.pendingReply`, set
  by the composer, or `Composer.PendingConv()`) and is not selected; when none can go, the new one
  is refused (`CONV_FULL`). Everything is lost on /reload and logout, never saved.

### Following (D3)

`Conv.OnLineTracked` runs for every new tracked line; a sender with a conversation is followed:

| `follow` | A later line becomes |
|---|---|
| `off` | nothing (only [T] adds) |
| `add` | an untranslated bubble: the original drawn by the addon and a Translate button |
| `smart` (default) | translated when the conversation is a whisper (WHISPER, BN_WHISPER: sticky), once I replied in it (`Conv.MarkReplied`), or when its box is ticked; otherwise as `add` |
| `translate` | translated |

The tab's box overrides the setting (ticked: translate; unticked: add, or off in mode off). A
translated bubble is requested only when it is in view (window shown, tab selected, within one page);
nothing is queued while the window is hidden. Following never opens the window and never takes the
focus; it marks the tab unread. `convOpenWhisper` (off by default) shows the hidden window for a
followed whisper, never in combat, never with the focus. A whisper I type in the normal chat to a
sender with a conversation (`CHAT_MSG_WHISPER_INFORM`, `CHAT_MSG_BN_WHISPER_INFORM`) becomes my
bubble ("typed"), unless it is the echo of a reply sent from the window in the last 5 s.

### Bubbles

- **Incoming:** meta line (channel in its chat colour, time, an "Original" link in gold: D5, the
  original is hidden unless `convOriginal` is on), the bubble (the ChatBubble nine-slice tinted
  with the channel colour, its translucent centre hidden, over an opaque `BUBBLE_RGB` fill), K rows
  of a tag (8 colour textures, `Bridge.ApplyTag`) and a `PLATE_RGB` plate, the tail at its atlas
  size, then the original (2 lines at most) and the glossary line when `glossShow` is on. The
  Helper paints the translation in its light blue (D8). Hover: the original, the channel and time,
  the instructions. Click: translate again with 6 rows (served from the cache; every click sends
  again, so it repairs "Not received - click the bubble"); Shift-click: without the cache; right-click: "Translate again", "Copy the original" (a box with the text
  selected, "Press Ctrl+C"), "Remove".
- **Mine:** right-aligned, the mirrored tail, the exact text in white, "You wrote: ..." under it
  when the draft differs, the meta line "You > channel time" and the state set by the composer
  (`Conv.SetOutState`: Sending..., Sent, Not confirmed, Not delivered: X is offline, Blocked).
- **The reply card** (`Bubble.AcquireReply`, laid out by the composer): "Your text: ..." with
  `card.EditButton` and `card.CancelButton`, the reply's rows (row 0 the Helper's status), and the
  lane: one row of fill right under the last row with the hint "No status above? Start or update
  TranslateProbe Helper." from the plate's x, clear of its top and bottom 2 px.
- **Geometry** (`Reserve.WindowMetrics(frame, contentWidth, contentLeft)`, `ConvFrame.Metrics()`):
  `ppu = effective scale * physH / 768`; tag blocks `W = max(2, ceil(5 / ppu))` units (at least
  5 px); rows `H = convText + 2`, `rowPx = round(H * ppu)`, pitch `rowH = rowPx / ppu`, so row tops
  are exactly `rowPx` physical px apart (textures without pixel snapping, like the strip's);
  `pad = max(6, ceil(4 / ppu))`; the border art may shade the outer 10 units (`border`), the fill
  reaches 6 units from the edge under it, so rows and tags start at `border + pad` (16 at the
  defaults, Blizzard's ChatBubble text inset). The bubble column starts 6 units into the thread's
  content, `bubbleWidth = contentWidth - 36`, `plateWidth = bubbleWidth - 2 border - 2 pad - 8 W`.
  K = `min(6, max(1, ceil(textWidth / plateWidth * factor * 1.1 - 0.15)))` with the script factors
  of the inline rows (`Reserve.EstimateWindowRows`).

### Slots, rows and VIEW 200

- Slots: 0..99 chat lines, **100..125 window bubbles**, 126 the reply (`Reserve.REPLY_SLOT`),
  127 TEST. A window slot belongs to `{ kind = "win", conv, msg }`; `Reserve.Scan` and
  `Request.ExpireOldest` never touch it. A freed window slot waits 2 s in a FIFO quarantine
  (`Reserve.FreeWindowSlot`), so the Helper never paints an old translation on the bubble that
  inherits it.
- The slot manager (`ConvFrame`, after each layout and scroll): every translated bubble within one
  page of the view gets a slot and its TRANSLATE (`Request.StartWindow`: plate flag, glossary pairs,
  the saved target, a VIEW first; a new reqId when its rows change or the slot comes back). Bubbles
  of other tabs keep their slots until the budget needs them; then the least recently seen slot
  outside the view is given back (`Request.DropWindow`: CANCEL, quarantine) and the manager tries
  again when the quarantine ends.
- **Per-row visibility:** after each layout and scroll, a row whose tag and plate are not entirely
  inside the visible thread (under the banner, above the "new messages" button) has both hidden;
  the fill shows there. The Helper never sees a partly clipped row.
- **VIEW 200** (`ConvFrame.ViewEntry`, listed first by `Reserve.View`): the bubble column's clean
  fill (x from content left + 6 + border, width `bubbleWidth - 2 border`), from the thread's top
  to the reply card's fill bottom (`Composer.ReplyCardRect()`), else the thread's bottom; `fontPx`
  and `rowPx` from the window. A chat frame the window overlaps is left out of the VIEW while no
  inline request is active. The VIEW is forced on show, hide, drag and resize end, tab switch and
  UI scale or display size changes; the 2 Hz watcher runs while the window is shown or holds slots.

### The sender's language

`LangDetect` (restored from 8b7750e and adapted): the writing system decides first (Hangul `ko`,
kana `ja`, Han `zh`, Cyrillic `ru` or `uk`); Latin text is scored with the trigram profiles of
`LangData.lua`, everyday chat words per language (`WORDS`), English game slang and a small prior
toward English. Rules added for the window:
- a line of 2 words or fewer is decided by its known words when they all point to one language
  ("moin", "vale", "obrigado") and is **unknown** (nil) without one;
- unknown when the best language leads the second by less than 15% of the evidence (`Lead`: the
  gap over the second language's distance above a line of unknown trigrams), going down to 8% for
  lines of 100 trigrams or more (`NeededLead`; measured in game on 2026-10-19: long German and
  French guild ads mixed with English game words led by only 12-14% and came out unknown);
- unknown when the best profile fits the line badly: mean trigram log-probability under -7.95 over
  30 trigrams or more (`FIT_MIN`). Finnish, Swedish, Polish and Dutch lines (the corpus's
  `unknown` list) used to come out as Italian or English;
- per conversation, the scores of the sender's last 5 lines are summed (`Merge`, `Decide`; the
  writing system wins when most lines use one).
On `tests\vectors\lang_corpus.json` (158 synthetic chat lines): 149 right, 9 unknown, 0 wrong; 91
of 96 lines of 4 words or more right, the other 5 unknown. About 1.7 ms per long line in fengari.
Unknown shows "? v" ("Language not recognized. Click to choose."). Japanese and Ukrainian are not
translator targets: `Conv.ReplyLang` is then nil and the composer asks for a language.

### The reply composer's interface (shared with work package R)

`ConvFrame.composerHost`, `ConvFrame.Relayout()`, `ConvFrame.SyncView(force)`,
`ConvFrame.CycleTab(dir)`, `ConvFrame.Metrics()`, `Bubble.AcquireReply(parent)` (`SetDraft`,
`SetRows`, `SetLaneHint`, `Layout(width, m) -> h`, `CardRect()`), `Bubble.Font()`,
`Bubble.TextWidth(text)`, `Request.NextReqId()`, `Reserve.REPLY_SLOT`, `Conv.Selected`,
`Conv.SourceLang`, `Conv.ReplyLang`, `Conv.SetReplyLang`, `Conv.AddOut`, `Conv.SetOutState`,
`Conv.MarkReplied`, `Conv.On("select" | "change" | "close" | "incoming", fn)`, `Conv.Clear(silent)`.
The window calls `Composer.Attach(host)` once, `Composer.SetConversation(conv)` on each selection,
`Composer.Height()` and `Composer.ReplyCardRect()` on each layout, and `Composer.Focus()` for
`convFocus` and the reply binding.

## Reply and send

Work package R (`Composer.lua`, `Channels.lua`, `Consent.lua`, `ReplyLocales.lua`; user decisions
D1, D2, D6, D7). Drafts, the pending reply, sent texts and their states live in Lua tables only.

### The composer

| State | Box | Enter | Stepper |
|---|---|---|---|
| EMPTY | empty | clears the focus | "1 Write your reply - Enter translates it" |
| DRAFT | typed text | Translate | same |
| WAITING | emptied by the addon after Translate, focused, "Press Ctrl+V when the green line appears" | sends nothing, "Paste with Ctrl+V first" | "1 Write > 2 Paste (Ctrl+V) > 3 Send" |
| PASTED | the pasted text (editable) | clears the focus, "Click a channel button to send" | "2 Pasted > 3 Click a channel to send" |
| SENT | emptied, focus cleared | - | "Sent to <channel>", then EMPTY |

- **Enter never sends (D1):** only a click on a channel button does, once per click, with a 1 s
  guard against a double click.
- **Translate** (Enter in DRAFT, or the button "Translate to XX"): a draft is needed; when the
  reply language is unknown (a Japanese sender, an unrecognized one) the language popup opens
  first; with `replyClipboard = "ask"` the consent dialog opens and the box gives up its focus.
  Then the reply card shows "Your text: ..." with [Edit] (the draft goes back into the box, the
  reply is cancelled) and [x] (cancelled), the box is emptied and focused, and TRANSLATE goes out on
  slot 126: rows = 1 + the estimated rows of the translation (2..6; row 0 carries the Helper's
  status), flags plate + reply (6), + copy (8) when the clipboard is allowed, + noCache (1) on
  Shift-click; the text is `Glossary.PromptText(draft)` with no glossary pair. One reply exists at
  a time: a new Translate, in any conversation, replaces it on the same slot.
- **Paste** (G12 confirms it on the client): in WAITING, `OnTextChanged(userInput)` with Ctrl
  held, or 2 characters or more inserted at once into the emptied box. The stepper says "Pasted"
  in green. A pasted line break keeps the first line, with a warning.
- **Checks that arm a button** (the first click shows an orange warning and arms that button for
  3 s; a second click sends; any text change disarms): the pasted text is my own draft; it holds a
  password-like token (8+ characters mixing 3 kinds), a URL or an e-mail address; it is confidently
  another language than the reply language; it was pasted less than 1 s after Translate (the old
  clipboard); or, in DRAFT, the typed text is confidently in my own language.
- Up arrow in an empty box recalls the last draft; Tab and Shift+Tab switch conversations; each
  conversation keeps its own draft. At combat start the box loses the focus unless a key was typed
  in it in the last 1.5 s (dropping it mid-word would turn the next letters into ability keys).
- During a chat lockdown the stepper turns red with the manual path ("Press Enter, then Ctrl+V,
  then Enter in the normal chat box"), every send button is greyed, and nothing is cancelled, so
  the clipboard keeps the translation.

### Channels

- **Buttons** (`Channels.Targets`): 1. where the sender wrote last (whisper, the numbered channel,
  say, party...), highlighted, tooltip "Default: the channel X used"; 2. Whisper unless it is the
  default; 3. Say (Yell when Say is the default), or the channel I last used in this conversation.
  Battle.net senders get Whisper only (`C_BattleNet.SendWhisper` with the account id). [Other]
  opens a popup with say, yell, party, raid, instance, guild, officer, every joined channel and
  "Whisper <name>"; a pick there puts that channel on button 3 and does not send (D1). Labels are
  Blizzard's globals (`SAY`, `YELL`, `WHISPER`...: "Dire", "Crier", "Chuchoter" on frFR).
- **A channel is found by its name** (`GetChannelName(base name)`) at send time. Its old number is
  used only when the channel now at that number has exactly that name; a zone channel left for
  another zone ("General - Elwynn Forest" while channel 1 is now "General - Westfall"), or a
  channel left and replaced by another one at the same number, greys the button with "You left
  this channel" and its own name (review fix, decision 47). A greyed button keeps a tooltip with
  the reason (not in a group, raid, instance or guild; not an officer; left; too long; locked).
- **Wire text** (`Channels.Wire`): bytes 0x01-0x03 removed, line breaks become spaces, trimmed,
  canonical pipe escaping (every `||` kept, every other `|` doubled), so the result is the same
  whether the box returns raw or doubled pipes; the 255-byte limit is checked on that result.
- **Send** (`Channels.Send`, from the button's `OnClick`, a hardware event): refused before any API
  call while chat is locked; one call per click. After it: my bubble ("Sending..."), CANCEL on slot
  126 (the Helper empties the clipboard if it still holds its own text), the box emptied and the
  focus cleared, the conversation marked replied (smart following then translates it).
- **Echo:** the server's echo (`CHAT_MSG_*` with my GUID and the same text, or `*_INFORM`) within
  5 s gives "Sent"; none gives "Not confirmed"; `ERR_CHAT_PLAYER_NOT_FOUND_S` for that name gives
  "Not delivered: X is offline"; `ADDON_ACTION_BLOCKED` naming this addon gives "Blocked".

### The clipboard consent (R1)

`Consent.Ask(onAllow, onDeny, anchor)` runs at once for a saved "allow" or "never", else shows
`TranslateProbeConsent` (`DialogBorderDarkTemplate`, else a tooltip border; 380 wide, taller when
the French text needs it; strata DIALOG, centred on the window): the title, the explanation (the
clipboard is cleared when Translate is clicked, then replaced by the translation; incoming messages
are never copied; nothing leaves the PC), "Remember my choice" (unticked) and [Allow] [Deny].

- No default button: Allow always needs a click. **Enter is left to the game** (it may open the
  chat box) and decides nothing; **Escape is Deny, this time only** ("Remember" ignored). Keyboard
  propagation is turned off only for that Escape, which hides the dialog in the same call, and
  never in combat, so a shown dialog never swallows movement or ability keys (review fix,
  decision 48).
- Allow sets the copy bit; with "Remember", `replyClipboard = "allow"`. Deny: no copy bit, the
  Helper paints "Ready (not copied)" and the placeholder becomes "Type the translation shown
  above"; with "Remember", `replyClipboard = "never"`. Options (a 3-step slider) and
  `/tpr clip ask|allow|never` change it back.
- The dialog belongs to the window: hiding the window (its X, Escape, `/tpr conv`, the binding,
  `convCombat = hide`) closes it without a decision, so nothing is sent (review fix, decision 50).
- The Helper copies only when the copy bit is set **and** its tray switch "Copy translated replies
  to the clipboard" is on (on by default, D6), and empties the clipboard as soon as a consented
  reply request arrives (D2); see `docs\HELPER.md`.

## Settings of the conversation window

Validated in `Core.lua` (schema 4); the window reads them from `ns.db` at each layout.

| Key | Values (default) | Used for |
|---|---|---|
| `display` | `window` / `inline` (`window`) | what a [T] click opens (an old `"tab"` becomes `window`) |
| `follow` | `off` / `add` / `smart` / `translate` (`smart`) | later lines of a conversation's sender |
| `replyClipboard` | `ask` / `allow` / `never` (`ask`) | the reply's clipboard consent (work package R) |
| `convOriginal` | bool (false) | originals shown under every bubble |
| `convText` | 12-20 (14) | text size of the window and of the Helper's text in it |
| `convScale` | 80-150 (100) | window scale |
| `convCombat` | `keep` / `hide` (`keep`) | hide the window during combat, show it again after |
| `convFocus` | bool (false) | put the cursor in the reply box when [T] opens a conversation |
| `convOpenWhisper` | bool (false) | a followed whisper shows the hidden window |
| `convPos` | `{ p, x, y, w, h }` numbers | the window's point p (the addon writes `"TOPLEFT"`) at (x, y) from UIParent's bottom-left corner in the window's units, and its size; saved after each drag or resize, cleared by "Reset window position" (`ConvFrame.ResetPosition`) |
| `convTipShown` | bool | the one-time tip at the first opening |

### Inline mode: the glossary row (D11)

The glossary row under a line ("Glossary: LF: cherche (looking for), ...") is drawn in its own
colour, the pale gold `e6cc80` of the Blizzard item palette (artifact quality), apart from the
light blue of the rows (`9fd4ff`) and of [T] (`66bbff`). `ChatButton.SuffixLinks` starts a row
whose text begins with `L.SUFFIX_GLOSS` with `ChatButton.GLOSS_START` (the same link, its own
colour code) instead of `SUFFIX_START`; the row cutters find either marker (`RowsAt`), and the
glossary row always follows the tagged rows. The row is off by default since schema 4
(`glossShow`, Options checkbox, `/tpr show on|off`; the migration is in `Core.lua`). In the window
the glossary goes in the bubble's glossary line and the romanized name in the title, never as
chat rows.

## Tests

- `node tests\run.js enUS` and `node tests\run.js frFR`: the harness (`tests\harness.lua`) on the
  mock client (`tests\mock.lua`) with the real `Bridge.lua`.
- Add `--stub-bridge` (or set `TPROBE_STUB_BRIDGE=1`) to load `tests\stubs\Bridge.lua` instead. A
  stub run also checks that the stub builds the same bytes as the real `Bridge.lua`.
- `luacheck addon`.
- The harness also covers the options panel on the mock Settings API (registration, dropdowns and
  checkboxes, the glossary page per target, old-format entries), the 6-row cap and the removal
  of the chat tab mode. The mock refuses templates and font objects it does not list
  (`Mock.KNOWN_TEMPLATES`, `Mock.KNOWN_FONTS`), so a new one has to be checked in the UI source
  first.
- `tests\harness_conv.lua` (run by `tests\harness.lua` with the shared context, before the final
  privacy checks): [T] routing (window, duplicate line, Shift, Ctrl, inline mode), tabs (order,
  "+N", the pick swap, wider window), closing (the cross on hover and on the selected tab,
  middle-click on the disabled selected tab, the right-click popup, the Escape list while a popup
  shows), reopening with the whole history, memory bounds and eviction order, the follow matrix
  (4 modes x public, whisper, replied), the tab's own choice, plain bubbles and Translate, the
  "new messages" button, `convOpenWhisper`, typed whispers and echoes, my bubbles' states, window
  slots (never scanned or expired, CANCEL on removal, the 2 s quarantine, hidden tabs keeping their
  slots until the budget takes the least recently seen), per-row visibility at both edges and
  after a wheel step, VIEW 200 (first, its rect, covered chat frames), metrics at UI scales 0.64
  to 1.0 (blocks at least 5 px, row pitch exactly `rowPx`), window scale and text size, the
  header and its language popup, a Japanese sender, the reply card's geometry (rows at `rowPx`,
  the lane under row K, `CardRect`), the Original link, bubble clicks and popup,
  "Copy the original", the glossary line, the romanized title, the key bindings, the empty window,
  position saving and the clamped resize, combat, the language detector on the shared corpus,
  the inline glossary row's colour, and nothing left in the saved variables.
- `tests\vectors\lang_corpus.json`: the shared language corpus (158 synthetic lines, `[code, line]`).
  `tests\run.js` gives the harness `READ_TEST_FILE(name)` to read it (fengari has no `io.open`).

The mock API that `tests\e2e` relies on is listed at the top of `tests\mock.lua`. `tests\run.js` is
also a module: `require('./run.js')` gives `BIT_SHIM`, `newState`, `runLua`, `tocFiles` and
`addonSources`.

## Decisions and deviations from the design

1. **Who calls `Mock.Load`.** `tests\run.js` loads `mock.lua`, the `bit` shim and the TOC sources
   (`ADDON_FILES`, `ADDON_SOURCES`, `ADDON_FROM`), then runs `harness.lua`, which calls
   `Mock.Load`. Reason: the purge check needs a 70 KB saved-variables fixture in place before
   ADDON_LOADED, and the session check needs old rows in the chat history before
   PLAYER_ENTERING_WORLD.
2. **`--stub-bridge`.** The design loads a stub only for a missing file; the flag forces the stub
   while the real `Bridge.lua` exists, so both can be run.
3. **`Mock.Load` fires ADDON_LOADED, PLAYER_LOGIN, then PLAYER_ENTERING_WORLD**, as the client does
   (old-session rows are cut at the first PLAYER_ENTERING_WORLD). `print` also adds the line to
   `DEFAULT_CHAT_FRAME`'s history, and chat frames keep 128 entries, as on the client.
4. **Additions to the C.2 API** (all additive): `ns.VERSION`; `ChatButton.CutRows`,
   `ChatButton.LINK_PREFIX`, `ChatButton.SUFFIX_START`; `Reserve.Measure`, `Reserve.SyncView(force)`,
   `Reserve.SlotsInUse`, `Reserve.IsWatching`, `Reserve.Counters`; `Request.Drop`, `Request.Get`,
   `Request.Counters`; `Restriction.ClearDeferred`, and `Restriction.Defer(lineID, nil)` cancels.
   Reason: when the read-only scan frees the slot of a line that left the chat, `Request` must
   forget that request (`Drop` sends CANCEL); "send VIEW when it changed or on the first send"
   needs state shared by the click flow and the watcher (`SyncView`).
5. **`Request.ClearAll`** removes rows with one `CutRows` pass over every row block of this
   session (including waiting rows and `/tpr test rows` rows), instead of `SetSuffix(nil)` per
   active line. Same result for active lines.
6. **An empty VIEW is never sent.** When no chat rectangle is readable (secret during a
   restriction, or no chat frame shown), nothing is sent and the helper keeps the last VIEW. An
   empty VIEW would erase the sticky one.
7. **Restriction details.** "Active" is tracked per restriction type. Each deferred click runs in
   its own timer callback, so an error in one cannot stop the others. Lines read back after a
   restriction skip the player's own lines (sender GUID) and lines without words. If chat is still
   locked at +1 s, the +30 s attempt retries; a generation counter makes only the latest Inactive
   event act.
8. **Hiding rows also cancels a waiting click** for that line, and a waiting click whose line is
   gone removes its "Waiting for the end of the restriction..." row.
9. **`/tpr gloss add` keeps meanings to 120 bytes**, because the purge drops longer values at the
   next load.
10. **`/tpr info`** also prints the client build line.
11. **`.luacheckrc`**: besides the listed removals, read_globals that nothing references any more
    are gone too (`C_AddOnProfiler`, `UpdateAddOnMemoryUsage`, `GetAddOnMemoryUsage`,
    `C_RestrictedActions`).
12. **`Glossary.PromptText`** first deletes any 0x01-0x03 byte from the raw text (a player cannot
    forge a mark) and uses 0x01 internally for the escaped pipe `||`.

13. **Row estimate without rounding first (review fix).** The design rounds `rowsSrc` up to a
    whole row before applying the script factor, so every Latin-to-Latin line reserved at least 2
    rows, even "gg" (measured: "gg" at 5% of the plate gave K = 2). `Reserve.EstimateRows` now uses
    the unrounded width ratio: `K = min(Reserve.MAX_ROWS, max(1, ceil(width / plateW * f - 0.15)))`
    (the cap was 4, see decision 19). Short lines reserve 1 row; long lines are unchanged.
14. **Periodic read-only scan (review fix).** The design reclaims slots only when all 127 are in
    use, so after one click the 2 Hz VIEW watcher ran for the rest of the session (the idle state
    of C.5, no ticker, was never reached) and the click that finally triggered the scan queued up
    to 126 CANCELs ahead of its own TRANSLATE. The watcher now also runs `Reserve.Scan` at most
    every 15 s while slots are in use; slots of lines gone from every chat frame are freed a few at
    a time (one CANCEL each) and the watch stops when no request is left. The scan predicate
    still returns false for every entry, so nothing is repackaged and nothing un-fades.
15. **Secret arrivals (review fixes).**
    - A line counts as arrived secret when any argument of its chat event is secret
      (`canaccessvalue(text, ...)`), because the client skips the message filters in that case,
      not only when the text is secret. The mock now applies the same rule to all arguments.
    - Before reading lines back, `Restriction` keeps only lines that some chat frame still shows
      (`ChatButton.ShownLines`, a read-only `TransformMessages` scan of `eventArgs[11]`). A
      channel no tab displays is never printed into the main chat, as on the readable path.
16. **Mode `auto` follows the colourblind filter (review fix).** `Bridge.Init` registers
    `CVAR_UPDATE`; when `colorblindSimulator` changes and the requested mode is `auto`, the mode
    is resolved again (mono or dense). The mock's `SetCVar` fires `CVAR_UPDATE` like the client.
17. **`{...}` groups are counted in UTF-8 characters** (1-32), as the design says; the first
    version counted bytes, which left out 12 Hangul characters.
18. **`Glossary.Lookup` removed.** Nothing called it (the old `Describe` uses `Find`); the design
    listed it as kept unchanged.
19. **Up to 6 reserved rows, Latin-to-Latin factor 1.35 (2026-10-18, in-game fact).** A German line
    translated into French needed more than the 4 rows it got and was cut with "..." in game. The
    cap is now `Reserve.MAX_ROWS = 6` (TRANSLATE rows 1..6, `docs\BRIDGE.md`) and the
    `latin>latin` factor went from 1.25 to 1.35. The tag's row field keeps its 2 bits, so rows 4
    and 5 carry tag rows 0 and 1 (`Bridge.TagDigits` takes `row % 4`); the helper numbers a card's
    rows on from its first visible tag. `Bridge.lua` needed no change (it never checked the range).
    `ns.ADDON_BUILD` is 2 (review fix: a Helper built for 1..4 rows silently drops a 5- or 6-row
    TRANSLATE, a parse error it only counts; with the bump it shows "this Helper is older than the
    addon", and a new Helper tells a build-1 addon to update). The Python reference
    (`tools\bridge\proto.py`) accepts 1..6 too, with a 6-row vector and a refused 7-row one in
    `tests\vectors\messages.json`.
20. **Chat tab display mode dropped (2026-10-18, user decision).** A first attempt of this round
    started a "tab" display mode (a dedicated chat tab with the original and its rows). The user
    prefers a separate conversation window, planned for the next round, so every part of it was
    removed before release: no `Tab.lua`, no `display` setting (an old `display = "tab"` is purged
    at load), no `/tpr show tab|inline` (`/tpr show` takes `on|off` for the glossary row only), no
    tab strings. Nothing of it reached a release.
21. **Per-target user glossary meanings (user request; the design kept `string|false`).** The
    design (B.4) left the user glossary schema unchanged, with one meaning for every target. The
    user asked for meanings that follow the target language, so a value may now also be a table
    of meanings per target (see "User glossary" above). Old values are valid as they are; nothing
    is rewritten at load. The change is one way: a build before it (b8a3e23 and older) accepts
    only `string|false` and deletes every table entry at load, including an old string folded
    into `"*"`. So `db.schema` is 3 since this change (review fix), and `scripts\dev.ps1` copies
    the saved variables to `%LOCALAPPDATA%\TranslateProbe\savedvariables-backup\` before each
    deploy; a rollback restores that copy with the game closed.
22. **`/tpr` alone opens the options panel** (it printed the command list); `/tpr help` prints the
    list, which now also names `/tpr options` and `/tpr help`.
23. **A VIEW before every TRANSLATE (review fix, 2026-10-18).** The design sends VIEW only when the
    chat rectangles changed or on the first send of a session. A Helper started or restarted while
    WoW keeps running then stayed in its discovery scan, whose view is the whole client width
    (the Helper took its background samples from the game world there). `Request` now calls
    `Reserve.SyncView(true)` before each TRANSLATE, re-clicks included: about 15 more bytes on the
    strip, in the same frame as the TRANSLATE. The 2 Hz watcher still sends changes.
24. **[T] opens the conversation window (2026-10-19, user decision D4).** `display` is back with the
    values `window` (default) and `inline`; Ctrl-click uses the other mode, Shift keeps meaning
    "without the cache". The inline checks of `tests\harness.lua` set `display = "inline"` first, and
    `tests\e2e\scenario_lib.lua` does the same after loading the addon (its scenarios test the rows
    under a line; without it `dropped.lua` failed: "dropped click sent no request").
25. **Closed tabs keep their history; 30 conversations x 100 messages (user decision D10).** The
    design kept 10 conversations and dropped a closed one. Now closing only takes the tab away;
    the 31st conversation evicts the least recently active closed one first, never an open one with
    an unread message or a pending reply, and at most 10 tabs are shown with the rest behind "+N".
    The thread keeps 100 messages (the design said 60).
26. **The inline glossary row has its own colour (user decision D11).** `e6cc80` (see "Inline mode:
    the glossary row"). `SuffixLinks` picks the row marker from the row's text, so the rows built by
    `Request` and by the harness's `RowsFor` stay byte for byte the same. The harness's "rows never
    duplicated" check counts both markers.
27. **Chat slots are 0..99 (design B.1).** Before, chat lines had 0..126. The harness's "slots"
    section now fills 100 chat slots and pushes 40 lines (was 10) so some leave the chat history.
28. **`convPos` holds named fields `{ p, x, y, w, h }`**, the form `Core.lua` validates (work
    package R), with x and y from UIParent's bottom-left corner (the window's own units): exact at
    any window scale, unlike offsets from a centre.
29. **Bubble geometry constants (not given by the design, to be judged in game, G13):** the
    ChatBubble border may shade the outer 10 units, the fill reaches 6 units from the edge under it
    (so its corners stay inside the rounded border), and rows start at 16 (Blizzard's ChatBubble text
    inset). My bubbles are text + 24 wide (the design said + 20, which put the text on the border
    art). The reply card has margins of 8 above the draft line and below the lane: 86 high with 2
    rows at the defaults (the design counted 82); the composer takes the height `Layout` returns.
30. **Duplicate [T]: the meta line is highlighted, not the bubble.** A highlight over the bubble
    would change the tag colours for 1.5 s.
31. **A chat frame under the window is left out of the VIEW only while no inline request is
    active at all.** The design said "when it has no inline request"; the addon does not know which
    frame shows which line, and the Helper drops duplicate tags anyway.
32. **The window's X hides the window itself** instead of `UIPanelCloseButton_OnClick`:
    `HideUIPanel` returns early when protected functions are not allowed, which is the case for an
    addon frame in combat (`UIParentPanelManager.lua`, Forever branch), and the design wants
    everything to work in combat.
33. **Popups and Escape:** no keyboard capture (`EnableKeyboard` is a protected function on the
    12.x API). The popup is in `UISpecialFrames`, and while it is shown the window leaves that list
    (it comes back one frame after the popup hides, not during `CloseSpecialWindows`' traversal), so
    the first Escape closes the popup only. A popup opened while another one is open keeps the
    window out of the list (decision 49).
34. **Language detector changes from 8b7750e:** hint words that are also English ("was", "si")
    left the German and Spanish lists, the misses of the old detector went in ("moin", "vamos",
    "valeu", "j'arrive", "c'est", "arrivo") with common words of the corpus, `WORD_BONUS` went from 4
    to 6, the short-line rule and the 15% lead (measured on the evidence above the floor) were
    added. `tools\make_langid.py` is restored from 8b7750e, unchanged, as the generator of
    `LangData.lua` (which is unchanged too). One corpus line, a German guild ad of the old harness,
    had a real invitation link: it is replaced by `https://example.org/gilde`.
35. **A followed line in add mode for a closed conversation joins its history** and the tab stays
    closed; only a translated message reopens it (the D10 wording: "when a new message from that
    sender is translated later").
36. **`/tpr clear` and `/tpr conv clear` hide the window** after clearing the conversations (an empty
    window would only say "Click [T]"). `Request.ClearAll` calls `Conv.Clear(true)`: its CLEAR made
    the Helper forget every card, so the window slots are freed without a CANCEL each.
37. **A line reprinted after a chat restriction** ([Show them]) has no line details from the filter:
    `ChatButton.LineInfo` reads the sender's GUID back (`GetChatLineSenderGUID`) and keeps the name
    `Restriction` gave; its chat event is unknown, so its bubble shows no channel until the sender
    writes again.
38. **Bubbles may wait up to 2 s for a slot** after many were freed at once (closing tabs, a clear):
    the quarantine applies, the slot manager tries again when it ends, and the bubble shows its fill
    meanwhile.
39. **French wording of the window strings** follows the approved mockup (formal "vous", as the game
    itself); the chat-side strings of `Locales.lua` keep "tu".
40. **Tabs use `PanelTopTabButtonTemplate` with `PanelTemplates_*`** (read in the Forever source,
    `SharedUIPanelTemplates.lua`), not `TabSystemTopButtonTemplate`, whose tab system owns the
    selection; the close cross is the stock `UIPanelCloseButton` at 14 x 14. The source language
    button reads "DE v" in plain text (no arrow atlas is verified on Forever).
41. **Files of other work packages touched by work package A** (each change a consequence of the
    window): `tests\harness.lua` (the glossary row marker in "rows never duplicated", the inline
    [T] tooltip, the 100 chat slots), `tests\run.js` (`READ_TEST_FILE`, so the harness can read
    `tests\vectors\lang_corpus.json`), `tests\e2e\scenario_lib.lua` (`display = "inline"`, decision
    24) and `Locales.lua` (`/tpr info` counts "chat slots in use: N of 100").
42. **Files of other work packages touched by work package R:** `tests\harness.lua` (the purge,
    options-order and display checks follow the schema 4 defaults; `db.glossShow, db.display =
    true, "inline"` keeps the existing chat checks inline) and `tests\mock.lua` (the Settings stock
    sliders, `RegisterInitializer`, the button and section header initializers: the APIs SkillUp and
    TIC use on Forever).
43. **A pick in the [Other] popup does not send** (the design listed the popup among the send
    targets): it puts that channel on button 3, and the send stays a click on a button (D1).
44. **`Channels.Wire` also turns line breaks into spaces** (a chat message is one line; the composer
    already keeps only the first pasted line, this covers a typed or recalled one).
45. **Two extra commands:** `/tpr conv reset` (the window back at its default place) and
    `/tpr test paste` (a probe of the reply box for G12 and G13 that prints counts only, never the
    text).
46. **French wording of the reply strings:** the window's strings say "vous" (decision 39); the
    options and command output keep the addon's "tu".
47. **A channel button never sends to another channel (review fix, 2026-10-19).** The design read
    the channel number with `GetChannelName(arg9)` at send time, else arg8. With that fallback the
    number alone was accepted: after a ride from Elwynn Forest to Westfall the default button
    "1. General" of a sender who wrote in "General - Elwynn Forest" sent to "General - Westfall",
    and a channel left and replaced by another at the same number sent there, both enabled and
    highlighted. Now the old number counts only when the channel there has exactly the sender's
    channel name, a line without its channel name never defaults to its number, and a greyed
    button shows its own channel name. G13 records arg9 of a zone channel on the client.
48. **The consent dialog never keeps the keyboard (review fix).** Enter was swallowed by turning
    keyboard propagation off; when combat started before the click, the dialog could not turn it
    back on (`SetPropagateKeyboardInput` is restricted in combat) and swallowed every key until a
    click. Enter now reaches the game (the dialog still decides nothing on it); only Escape is
    swallowed, and it hides the dialog in the same call.
49. **A popup opened over another one keeps the window out of the Escape list (review fix).**
    Closing the first popup scheduled the window's return for the next frame, after the second
    popup's `OnShow` had found nothing to remove, so the first Escape closed the window too. The
    scheduled return now carries a generation number and does nothing when a popup was shown
    since.
50. **The consent dialog closes with the window (review fix).** Hidden any other way, the window
    left the dialog on screen, and Allow then sent a reply for a window that was not shown.
    `ConvFrame`'s `OnHide` calls `Consent.Cancel()`.
51. **Every bubble click asks the Helper again (review fix).** The click set 6 rows, and the slot
    manager re-sent only when the row count changed, so only the first click repaired a bubble the
    Helper marked "Not received - click the bubble". The click now flags the bubble for one more
    TRANSLATE (a new reqId, served from the cache unless Shift is held).
