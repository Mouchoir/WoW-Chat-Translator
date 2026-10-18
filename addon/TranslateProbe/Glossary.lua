-- WoW chat abbreviations (LFM, BGs, DM, stock...): shown in a glossary row under a clicked
-- line, and sent to the helper as prompt pairs so the model reads them right.
-- Matching is per whole word and ignores case for ASCII letters. Words keep their accented
-- letters (WoW's %w is ASCII only, so "stéréo" must not split into "st" + "r" + "o").
-- Users add, override or disable entries with /tpr gloss (TranslateProbeDB.glossary); user
-- entries have no language and are sent as typed for any target.
-- The built-in meanings below are bilingual data (en + fr), not UI strings. Meanings follow the
-- translation target: French for "fr", English for every other target (a pivot the model reads).
-- This file also builds the marked text the helper translates (PromptText).
local _, ns = ...
local L = ns.L

local Glossary = {}
ns.Glossary = Glossary

-- key (lowercase) -> { en = English meaning, fr = French meaning }, or a list of such meanings
-- when one abbreviation has several (all are shown, the reader picks). plural = true lets
-- "<key>s" match too. Plain words that collide too often ("if" for Ironforge, "arm" for the
-- Armory) are left out on purpose.
local BUILTIN = {
    -- Grouping and trade
    lf = { en = "looking for", fr = "cherche" },
    lfg = { en = "looking for group", fr = "cherche un groupe" },
    lfm = { en = "looking for more", fr = "cherche des membres" },
    pug = { en = "pick-up group", fr = "groupe de joueurs au hasard" },
    inv = { { en = "invite", fr = "invite" }, { en = "inventory", fr = "inventaire" } },
    pst = { en = "please send tell", fr = "chuchotez-moi" },
    wts = { en = "want to sell", fr = "vend" },
    wtb = { en = "want to buy", fr = "achète" },
    wtt = { en = "want to trade", fr = "échange" },
    ah = { en = "auction house", fr = "hôtel des ventes" },
    mats = { en = "materials", fr = "composants" },
    boe = { en = "bind on equip", fr = "lié quand équipé" },
    bop = { en = "bind on pickup", fr = "lié quand ramassé" },
    ml = { en = "master looter", fr = "maître du butin" },
    summ = { en = "summon", fr = "invocation" },
    lvl = { en = "level", fr = "niveau", plural = true },

    -- Roles and fights
    mt = { en = "main tank", fr = "tank principal" },
    ot = { en = "off tank", fr = "second tank" },
    dps = { en = "damage dealer", fr = "personnage qui inflige des dégâts" },
    oom = { en = "out of mana", fr = "plus de mana" },
    cc = { { en = "crowd control", fr = "contrôle de foule" }, { en = "hi (French slang)", fr = "coucou" } },
    aoe = { en = "area of effect", fr = "dégâts de zone" },
    rez = { en = "resurrect", fr = "résurrection" },
    rdy = { en = "ready", fr = "prêt" },
    dot = { en = "damage over time", fr = "dégâts sur la durée", plural = true },
    hot = {
        plural = true,
        { en = "Hall of Thanes (dungeon)", fr = "donjon Hall of Thanes" },
        { en = "heal over time", fr = "soin sur la durée" },
        { en = "hot", fr = "chaud" },
    },

    -- Chat
    pls = { en = "please", fr = "s'il te plaît" },
    plz = { en = "please", fr = "s'il te plaît" },
    ty = { en = "thank you", fr = "merci" },
    thx = { en = "thanks", fr = "merci" },
    np = { en = "no problem", fr = "pas de souci" },
    gg = { en = "good game", fr = "bien joué" },
    wp = { en = "well played", fr = "bien joué" },
    gz = { en = "congratulations", fr = "félicitations" },
    grats = { en = "congratulations", fr = "félicitations" },
    afk = { en = "away from keyboard", fr = "absent" },
    brb = { en = "be right back", fr = "je reviens" },
    omw = { en = "on my way", fr = "j'arrive" },
    idk = { en = "I don't know", fr = "je ne sais pas" },
    nvm = { en = "never mind", fr = "laisse tomber" },

    -- Battlegrounds
    bg = { en = "battleground", fr = "champ de bataille", plural = true },
    wsg = { en = "Warsong Gulch", fr = "Goulet des Chanteguerres" },
    ab = { en = "Arathi Basin", fr = "Bassin Arathi" },
    av = { en = "Alterac Valley", fr = "Vallée d'Alterac" },

    -- Dungeons
    rfc = { en = "Ragefire Chasm", fr = "Gouffre de Ragefeu" },
    wc = { { en = "Wailing Caverns", fr = "Cavernes des lamentations" }, { en = "toilet", fr = "toilettes" } },
    dm = {
        { en = "The Deadmines", fr = "Les Mortemines" },
        { en = "Dire Maul", fr = "Hache-Tripes" },
        { en = "direct message", fr = "message privé" },
    },
    sfk = { en = "Shadowfang Keep", fr = "Donjon d'Ombrecroc" },
    stock = {
        plural = true,
        { en = "The Stockade", fr = "La Prison" },
        { en = "stock (goods)", fr = "stock (marchandise)" },
    },
    stockade = { en = "The Stockade", fr = "La Prison" },
    bfd = { en = "Blackfathom Deeps", fr = "Profondeurs de Brassenoire" },
    gnomer = { en = "Gnomeregan", fr = "Gnomeregan" },
    rfk = { en = "Razorfen Kraul", fr = "Kraal de Tranchebauge" },
    sm = { en = "Scarlet Monastery", fr = "Monastère Écarlate" },
    gy = { en = "Graveyard", fr = "Cimetière" },
    lib = { en = "Library", fr = "Bibliothèque" },
    armory = { en = "Armory", fr = "Armurerie" },
    cath = { en = "Cathedral", fr = "Cathédrale" },
    rfd = { en = "Razorfen Downs", fr = "Souilles de Tranchebauge" },
    ulda = { en = "Uldaman", fr = "Uldaman" },
    zf = { en = "Zul'Farrak", fr = "Zul'Farrak" },
    mara = { en = "Maraudon", fr = "Maraudon" },
    st = { en = "Sunken Temple", fr = "Le temple d'Atal'Hakkar" },
    brd = { en = "Blackrock Depths", fr = "Profondeurs de Blackrock" },
    lbrs = { en = "Lower Blackrock Spire", fr = "Pic Blackrock inférieur" },
    ubrs = { en = "Upper Blackrock Spire", fr = "Pic Blackrock supérieur" },
    dme = { en = "Dire Maul East", fr = "Hache-Tripes Est" },
    dmw = { en = "Dire Maul West", fr = "Hache-Tripes Ouest" },
    dmn = { en = "Dire Maul North", fr = "Hache-Tripes Nord" },
    strat = { plural = true, { en = "Stratholme", fr = "Stratholme" }, { en = "strategy", fr = "stratégie" } },
    scholo = { en = "Scholomance", fr = "Scholomance" },

    -- Raids
    mc = { { en = "Molten Core", fr = "Cœur du Magma" }, { en = "Mind Control", fr = "Contrôle mental" } },
    ony = { en = "Onyxia's Lair", fr = "Repaire d'Onyxia" },
    bwl = { en = "Blackwing Lair", fr = "Repaire de l'Aile noire" },
    zg = { en = "Zul'Gurub", fr = "Zul'Gurub" },
    aq20 = { en = "Ruins of Ahn'Qiraj", fr = "Ruines d'Ahn'Qiraj" },
    aq40 = { en = "Temple of Ahn'Qiraj", fr = "Temple d'Ahn'Qiraj" },
    naxx = { en = "Naxxramas", fr = "Naxxramas" },

    -- Cities
    sw = { en = "Stormwind", fr = "Hurlevent" },
    org = { en = "Orgrimmar", fr = "Orgrimmar" },
    ogri = { en = "Orgrimmar", fr = "Orgrimmar" },
    tb = { en = "Thunder Bluff", fr = "Les Pitons-du-Tonnerre" },
    uc = { en = "Undercity", fr = "Fossoyeuse" },
    xr = { en = "The Crossroads", fr = "La Croisée" },
}
Glossary.BUILTIN = BUILTIN

-- Numbered forms: lf2m, lf1, ...
local PATTERNS = {
    { pattern = "^lf(%d+)m$", en = "looking for %s more", fr = "cherche %s membre(s)" },
    { pattern = "^lf(%d+)$", en = "looking for %s", fr = "cherche %s" },
}

-- Words that a model tends to translate wrong for one target. Used in prompts only, never
-- shown: the glossary row stays about abbreviations.
Glossary.FALSE_FRIENDS = {
    fr = { cheap = "pas cher" },
}

local WORD = "[%w\128-\255]+"   -- ASCII letters and digits plus any UTF-8 byte
local MEANING_SEP = " / "       -- prompt data, not a UI string
local MAX_PAIRS, MAX_TERM, MAX_MEANING, MAX_PAIR_BYTES = 8, 48, 120, 512
local MAX_PROMPT = 1024
local MAX_BRACE = 32

local function DbTarget()
    return ns.db and ns.db.target or "en"
end

local function One(one, target, value)
    local en, fr = one.en, one.fr
    if value then
        en, fr = en:format(value), fr and fr:format(value)
    end
    return en, (target == "fr" and fr) or nil
end

-- Prompt meaning of a built-in entry: French when the target is fr and the entry has it, else
-- English; several meanings joined with " / ".
function Glossary.MeaningFor(entry, target, value)
    if entry[1] == nil then
        local en, fr = One(entry, target, value)
        return fr or en
    end
    local meanings = {}
    for i, one in ipairs(entry) do
        local en, fr = One(one, target, value)
        meanings[i] = fr or en
    end
    return table.concat(meanings, MEANING_SEP)
end

-- Display form for the glossary row: "fr (en)" when the target is fr, else en.
local function DisplayOne(one, target, value)
    local en, fr = One(one, target, value)
    if fr then
        if fr:lower() == en:lower() then
            return fr
        end
        return L.GLOSS_BOTH:format(fr, en)
    end
    return en
end

local function Display(entry, target, value)
    if entry[1] == nil then
        return DisplayOne(entry, target, value)
    end
    local meanings = {}
    for i, one in ipairs(entry) do
        meanings[i] = DisplayOne(one, target, value)
    end
    return table.concat(meanings, L.GLOSS_OR)
end

-- Meaning of one word (ASCII part already lowercased), or nil, and true when it comes from a
-- user entry. User entries win; false disables a key (and its plural). Built-in keys are ASCII,
-- so accented words only match user entries. forPrompt: prompt meaning with the target's false
-- friends; otherwise the display form.
local function Find(key, allowPlural, target, forPrompt)
    local custom = ns.db and ns.db.glossary and ns.db.glossary[key]
    if custom == false then
        return nil
    elseif type(custom) == "string" then
        return custom, true
    end
    if key:find("[\128-\255]") then
        return nil
    end
    if forPrompt then
        local friends = Glossary.FALSE_FRIENDS[target]
        if friends and friends[key] then
            return friends[key]
        end
    end
    local entry = BUILTIN[key]
    if entry then
        if forPrompt then
            return Glossary.MeaningFor(entry, target)
        end
        return Display(entry, target)
    end
    for _, rule in ipairs(PATTERNS) do
        local value = key:match(rule.pattern)
        if value then
            if forPrompt then
                return Glossary.MeaningFor(rule, target, value)
            end
            return Display(rule, target, value)
        end
    end
    -- Plurals only for entries that allow them: "bgs" -> "bg", but not "abs" -> "ab".
    if allowPlural and key:sub(-1) == "s" then
        local base = BUILTIN[key:sub(1, -2)]
        if base and base.plural then
            return Find(key:sub(1, -2), false, target, forPrompt)
        end
    end
end

-- The words typed by the player: colors, links (quests, items...), textures, atlases,
-- protected Battle.net names and raid icons are removed.
local function PlainText(text)
    text = text:gsub("|c%x%x%x%x%x%x%x%x", " "):gsub("|r", " ")
    text = text:gsub("|H.-|h.-|h", " "):gsub("|T.-|t", " "):gsub("|A.-|a", " ")
    text = text:gsub("|K.-|k", " "):gsub("{.-}", " ")
    return text
end
Glossary.PlainText = PlainText

-- Latin-letter spelling of a name or text written in another script (Hangul, Han,
-- Cyrillic...), using the ICU transliterator built into the client. Nil when the text is
-- already ASCII or the client cannot transliterate it.
function Glossary.Romanize(text)
    if type(text) ~= "string" or issecretvalue(text) or not text:find("[\128-\255]") then
        return nil
    end
    if not (C_Intl and C_Intl.Transliterate) then
        return nil
    end
    local ok, out = pcall(C_Intl.Transliterate, text, "Any-Latin")
    if ok and not issecretvalue(out) and type(out) == "string" and out ~= "" and out ~= text then
        return (out:gsub("|", "/"))
    end
end

-- The text a reader sees: link brackets kept ("[Linen Cloth]"), codes, icons and protected
-- Battle.net names removed. Used to measure the line.
function Glossary.VisibleText(text)
    text = text:gsub("|c%x%x%x%x%x%x%x%x", ""):gsub("|r", "")
    text = text:gsub("|H.-|h(.-)|h", "%1"):gsub("|T.-|t", ""):gsub("|A.-|a", "")
    text = text:gsub("|K.-|k", ""):gsub("{.-}", "")
    return text
end

-- False for a line made only of links or icons, such as a shared quest.
function Glossary.HasWords(text)
    return PlainText(text):find(WORD) ~= nil
end

-- "LFM: cherche des membres (looking for more), DM: Les Mortemines (The Deadmines) / ..." or
-- nil. Meanings follow the target (default: the saved target); false friends are never shown.
function Glossary.Describe(text, target)
    target = target or DbTarget()
    local parts, seen = {}, {}
    for word in PlainText(text):gmatch(WORD) do
        local key = word:lower()
        if not seen[key] then
            seen[key] = true
            local meaning = Find(key, true, target, false)
            if meaning then
                -- string.upper only knows ASCII: keep accented words as typed.
                local label = word:find("[\128-\255]") and word or word:upper()
                parts[#parts + 1] = L.GLOSS_PAIR:format(label, meaning)
                if #parts >= MAX_PAIRS then
                    break
                end
            end
        end
    end
    if #parts == 0 then
        return nil
    end
    -- Chat links end at the next "|", so user text must not contain one.
    return (table.concat(parts, L.GLOSS_SEP):gsub("|", "/"))
end

-- Prompt pairs for the helper: array of { term as typed, meaning }. User entries first, then
-- the others, each in order of appearance; one pair per lowercase word; pairs whose meaning is
-- the word itself are skipped. Caps: 8 pairs, term 48 bytes, meaning 120 bytes, and
-- sum(2 + #term + #meaning) <= 512 (the TRANSLATE message limits).
function Glossary.Extract(text, target)
    target = target or DbTarget()
    local user, other, seen = {}, {}, {}
    for word in PlainText(text):gmatch(WORD) do
        local key = word:lower()
        if not seen[key] then
            seen[key] = true
            local meaning, isUser = Find(key, true, target, true)
            if meaning and meaning:lower() ~= key then
                local list = isUser and user or other
                list[#list + 1] = { word, meaning }
            end
        end
    end
    local cut = ns.Bridge.Utf8Cut
    local out, used = {}, 0
    for _, list in ipairs({ user, other }) do
        for _, p in ipairs(list) do
            if #out >= MAX_PAIRS then
                return out
            end
            local term, meaning = cut(p[1], MAX_TERM), cut(p[2], MAX_MEANING)
            local cost = 2 + #term + #meaning
            if used + cost > MAX_PAIR_BYTES then
                return out
            end
            out[#out + 1] = { term, meaning }
            used = used + cost
        end
    end
    return out
end

-- Wraps each "{...}" of 1-32 characters (UTF-8) in 0x02/0x03 marks, outside the marks
-- already present.
local function MarkBraces(s)
    local out, pos = {}, 1
    while true do
        local a, b = s:find("\2.-\3", pos)
        local segment = s:sub(pos, (a or #s + 1) - 1)
        out[#out + 1] = (segment:gsub("{([^{}]*)}", function(inner)
            local chars = select(2, inner:gsub("[^\128-\191]", ""))
            if chars >= 1 and chars <= MAX_BRACE then
                return "\2{" .. inner .. "}\3"
            end
        end))
        if not a then
            break
        end
        out[#out + 1] = s:sub(a, b)
        pos = b + 1
    end
    return table.concat(out)
end

-- Text sent to the helper: codes removed, link names ("[Linen Cloth]") and "{...}" groups
-- wrapped in 0x02/0x03 marks so the model keeps them, brackets a player typed left plain.
-- At most 1024 bytes, cut on a UTF-8 boundary and never inside a mark.
function Glossary.PromptText(raw)
    local s = raw:gsub("[\1\2\3]", "")    -- a typed control byte must never pass for a mark
    s = s:gsub("||", "\1")                -- escaped pipe, restored at the end
    s = s:gsub("|c%x%x%x%x%x%x%x%x", ""):gsub("|r", "")
    s = s:gsub("|H.-|h(.-)|h", function(name)
        if name == "" then
            return ""
        end
        return "\2" .. name .. "\3"
    end)
    s = s:gsub("|T.-|t", ""):gsub("|A.-|a", ""):gsub("|K.-|k", "")
    s = MarkBraces(s)
    s = s:gsub("\1", "|")
    s = ns.Bridge.Utf8Cut(s, MAX_PROMPT)
    local open, close = s:match(".*()\2"), s:match(".*()\3")
    if open and (not close or open > close) then
        s = s:sub(1, open - 1)
    end
    return s
end

-- Pattern of one glossary key typed in a command: a whole word, accents allowed.
Glossary.KEY_PATTERN = WORD

function Glossary.IsBuiltin(word)
    return BUILTIN[word:lower()] ~= nil
end

function Glossary.Count()
    local n = 0
    for _ in pairs(BUILTIN) do
        n = n + 1
    end
    return n
end

ns.OnLoad(function()
    ns.db.glossary = ns.db.glossary or {}
end)
