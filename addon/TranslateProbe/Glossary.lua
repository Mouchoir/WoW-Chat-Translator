-- Instant layer of the translator: expands WoW chat abbreviations (LFM, BGs, DM, stock...).
-- Matching is per whole word and ignores case. Users add, override or disable entries with
-- /tprobe gloss; their entries live in TranslateProbeDB.glossary.
local _, ns = ...

local Glossary = {}
ns.Glossary = Glossary

local MAX_SHOWN = 8

-- key (lowercase) -> { en = English meaning, fr = French meaning }, or a list of such meanings
-- when one abbreviation has several (all are shown, the reader picks). Plain English words
-- that collide too often ("if" for Ironforge, "arm" for the Armory) are left out on purpose.
local BUILTIN = {
    -- Grouping and trade
    lf = { en = "looking for", fr = "cherche" },
    lfg = { en = "looking for group", fr = "cherche un groupe" },
    lfm = { en = "looking for more", fr = "cherche des membres" },
    pug = { en = "pick-up group", fr = "groupe de joueurs au hasard" },
    inv = { en = "invite", fr = "invite" },
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
    lvl = { en = "level", fr = "niveau" },

    -- Roles and fights
    mt = { en = "main tank", fr = "tank principal" },
    ot = { en = "off tank", fr = "second tank" },
    dps = { en = "damage dealer", fr = "dégâts" },
    oom = { en = "out of mana", fr = "plus de mana" },
    cc = { en = "crowd control", fr = "contrôle de foule" },
    aoe = { en = "area of effect", fr = "dégâts de zone" },
    rez = { en = "resurrect", fr = "résurrection" },
    rdy = { en = "ready", fr = "prêt" },
    dot = { en = "damage over time", fr = "dégâts sur la durée" },
    hot = {
        { en = "Hall of Thanes, dungeon", fr = "donjon Hall of Thanes" },
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
    bg = { en = "battleground", fr = "champ de bataille" },
    wsg = { en = "Warsong Gulch", fr = "Goulet des Chanteguerres" },
    ab = { en = "Arathi Basin", fr = "Bassin Arathi" },
    av = { en = "Alterac Valley", fr = "Vallée d'Alterac" },

    -- Dungeons
    rfc = { en = "Ragefire Chasm", fr = "Gouffre de Ragefeu" },
    wc = { en = "Wailing Caverns", fr = "Cavernes des lamentations" },
    dm = { en = "The Deadmines", fr = "Les Mortemines" },
    sfk = { en = "Shadowfang Keep", fr = "Donjon d'Ombrecroc" },
    stock = { en = "The Stockade", fr = "La Prison" },
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
    st = { en = "Sunken Temple", fr = "Temple d'Atal'Hakkar" },
    brd = { en = "Blackrock Depths", fr = "Profondeurs de Blackrock" },
    lbrs = { en = "Lower Blackrock Spire", fr = "Pic Blackrock inférieur" },
    ubrs = { en = "Upper Blackrock Spire", fr = "Pic Blackrock supérieur" },
    dme = { en = "Dire Maul East", fr = "Hache-Tripes Est" },
    dmw = { en = "Dire Maul West", fr = "Hache-Tripes Ouest" },
    dmn = { en = "Dire Maul North", fr = "Hache-Tripes Nord" },
    strat = { en = "Stratholme", fr = "Stratholme" },
    scholo = { en = "Scholomance", fr = "Scholomance" },

    -- Raids
    mc = { en = "Molten Core", fr = "Cœur du Magma" },
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

local function LocalizedOne(entry, value)
    local en, fr = entry.en, entry.fr
    if value then
        en, fr = en:format(value), fr and fr:format(value)
    end
    if fr and GetLocale() == "frFR" then
        if fr:lower() == en:lower() then
            return fr
        end
        return fr .. " (" .. en .. ")"
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
    return table.concat(meanings, " / ")
end

-- Meaning of one lowercase word, or nil. Custom entries win; false disables a built-in.
local function Lookup(word, allowPlural)
    local custom = ns.db and ns.db.glossary and ns.db.glossary[word]
    if custom == false then
        return nil
    elseif type(custom) == "string" then
        return custom
    end
    if BUILTIN[word] then
        return Localized(BUILTIN[word])
    end
    for _, rule in ipairs(PATTERNS) do
        local value = word:match(rule.pattern)
        if value then
            return Localized(rule, value)
        end
    end
    -- Plurals: "bgs" falls back to "bg", "stocks" to "stock".
    if allowPlural and #word > 2 and word:sub(-1) == "s" then
        return Lookup(word:sub(1, -2), false)
    end
end
Glossary.Lookup = function(word)
    return Lookup(word:lower(), true)
end

-- Strips colors, links and raid icons so only the words typed by the player are scanned.
local function PlainWords(text)
    text = text:gsub("|c%x%x%x%x%x%x%x%x", " "):gsub("|r", " ")
    text = text:gsub("|H.-|h.-|h", " "):gsub("|T.-|t", " "):gsub("{.-}", " ")
    return text
end

-- "LFM: cherche des membres (looking for more), DM: Les Mortemines (The Deadmines)" or nil.
function Glossary.Describe(text)
    local parts, seen = {}, {}
    for word in PlainWords(text):gmatch("%w+") do
        local key = word:lower()
        if not seen[key] then
            seen[key] = true
            local meaning = Lookup(key, true)
            if meaning then
                parts[#parts + 1] = word:upper() .. ": " .. meaning
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
    return (table.concat(parts, ", "):gsub("|", "/"))
end

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
