-- Instant layer of the translator: expands WoW chat abbreviations (LFM, BGs, DM, stock...).
-- Matching is per whole word and ignores case for ASCII letters. Words keep their accented
-- letters (WoW's %w is ASCII only, so "stéréo" must not split into "st" + "r" + "o").
-- Users add, override or disable entries with /tprobe gloss (TranslateProbeDB.glossary).
-- The built-in meanings below are bilingual data (en + fr), not UI strings.
local _, ns = ...
local L = ns.L

local Glossary = {}
ns.Glossary = Glossary

local MAX_SHOWN = 8

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

local WORD = "[%w\128-\255]+"   -- ASCII letters and digits plus any UTF-8 byte

local function LocalizedOne(entry, value)
    local en, fr = entry.en, entry.fr
    if value then
        en, fr = en:format(value), fr and fr:format(value)
    end
    if fr and GetLocale() == "frFR" then
        if fr:lower() == en:lower() then
            return fr
        end
        return L.GLOSS_BOTH:format(fr, en)
    end
    return en
end

local function Localized(entry, value)
    if entry[1] == nil then
        return LocalizedOne(entry, value)
    end
    local meanings = {}
    for i, one in ipairs(entry) do
        meanings[i] = LocalizedOne(one, value)
    end
    return table.concat(meanings, L.GLOSS_OR)
end

-- Meaning of one word (ASCII part already lowercased), or nil. Custom entries win; false
-- disables a built-in. Built-in keys are ASCII, so accented words only match custom entries.
local function Lookup(key, allowPlural)
    local custom = ns.db and ns.db.glossary and ns.db.glossary[key]
    if custom == false then
        return nil
    elseif type(custom) == "string" then
        return custom
    end
    if key:find("[\128-\255]") then
        return nil
    end
    if BUILTIN[key] then
        return Localized(BUILTIN[key])
    end
    for _, rule in ipairs(PATTERNS) do
        local value = key:match(rule.pattern)
        if value then
            return Localized(rule, value)
        end
    end
    -- Plurals only for entries that allow them: "bgs" -> "bg", but not "abs" -> "ab".
    if allowPlural and key:sub(-1) == "s" then
        local base = BUILTIN[key:sub(1, -2)]
        if base and base.plural then
            return Lookup(key:sub(1, -2), false)
        end
    end
end
Glossary.Lookup = function(word)
    return Lookup(word:lower(), true)
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

-- False for a line made only of links or icons, such as a shared quest.
function Glossary.HasWords(text)
    return PlainText(text):find(WORD) ~= nil
end

-- "LFM: cherche des membres (looking for more), DM: Les Mortemines (The Deadmines) / ..." or nil.
function Glossary.Describe(text)
    local parts, seen = {}, {}
    for word in PlainText(text):gmatch(WORD) do
        local key = word:lower()
        if not seen[key] then
            seen[key] = true
            local meaning = Lookup(key, true)
            if meaning then
                -- string.upper only knows ASCII: keep accented words as typed.
                local label = word:find("[\128-\255]") and word or word:upper()
                parts[#parts + 1] = L.GLOSS_PAIR:format(label, meaning)
                if #parts >= MAX_SHOWN then
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
