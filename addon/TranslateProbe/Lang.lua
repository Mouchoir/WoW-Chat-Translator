-- Source language of a chat line, and the chain of model packs that translates it into the
-- user's target language (Mozilla's models all go through English: de -> en -> fr).
-- Scripts decide first (Hangul, kana, Han, Cyrillic). Latin text is scored with character
-- trigram profiles (LangData.lua, built by tools/make_langid.py from the client's localized UI
-- strings), plus strong hints: common chat words of each language and English game slang,
-- and a small prior toward English, the usual language of public channels.
local _, ns = ...

local Lang = {}
ns.Lang = Lang

local byte, sub = string.byte, string.sub

-- Languages the user can pick as target, and sources we may detect.
Lang.TARGETS = { "fr", "en" }
Lang.NAMES = { en = "EN", fr = "FR", de = "DE", es = "ES", it = "IT", pt = "PT", ru = "RU", zh = "ZH", ko = "KO",
    ja = "JA", uk = "UK" }

local ENGLISH_PRIOR = 2.0
local SLANG_BONUS = 3.0
local WORD_BONUS = 4.0

-- English game slang: counts for English and is not scored by trigrams.
local SLANG = {}
for w in ("lf lfg lfm wts wtb wtt pst inv pls plz ty thx np gg wp gz afk brb omw idk nvm lol ok kk rdy "
    .. "bg dm tank heal healer dps ffs omg wtf"):gmatch("%S+") do
    SLANG[w] = true
end

-- Short everyday chat words that point to one language.
local WORDS = {
    en = "hello hi hey yes yeah no thanks thank you please insane nice sorry mate guys anyone where what",
    de = "danke bitte ja nein hallo moin servus klar genau jemand gruppe suche brauche wer was wo",
    fr = "merci oui non salut bonjour bonsoir besoin cherche quelqu'un groupe svp stp mdr ouais",
    es = "hola gracias sí si vale alguien grupo busco necesito dónde amigo",
    it = "ciao grazie sì qualcuno gruppo cerco",
    pt = "olá obrigado obrigada sim alguém grupo procuro",
}
local HINT = {}
for lang, list in pairs(WORDS) do
    for w in list:gmatch("%S+") do
        HINT[w] = HINT[w] or {}
        HINT[w][#HINT[w] + 1] = lang
    end
end

-- Code point at s[i] and its byte length (s is assumed to be UTF-8).
local function CodePoint(s, i)
    local c = byte(s, i)
    if c < 0x80 then
        return c, 1
    elseif c >= 0xF0 then
        local b2, b3, b4 = byte(s, i + 1, i + 3)
        return ((c % 8) * 262144) + (((b2 or 0) % 64) * 4096) + (((b3 or 0) % 64) * 64) + ((b4 or 0) % 64), 4
    elseif c >= 0xE0 then
        local b2, b3 = byte(s, i + 1, i + 2)
        return ((c % 16) * 4096) + (((b2 or 0) % 64) * 64) + ((b3 or 0) % 64), 3
    elseif c >= 0xC0 then
        local b2 = byte(s, i + 1)
        return ((c % 32) * 64) + ((b2 or 0) % 64), 2
    end
    return c, 1
end

-- Two-letter code of the line's language, or nil when it has no letters.
function Lang.Detect(text)
    local s = ns.Glossary.PlainText(text)
    local hangul, kana, han, cyr, latin, ukrainian = 0, 0, 0, 0, 0, 0
    local i, n = 1, #s
    while i <= n do
        local cp, len = CodePoint(s, i)
        if (cp >= 0xAC00 and cp <= 0xD7A3) or (cp >= 0x1100 and cp <= 0x11FF) or (cp >= 0x3130 and cp <= 0x318F) then
            hangul = hangul + 1
        elseif cp >= 0x3040 and cp <= 0x30FF then
            kana = kana + 1
        elseif (cp >= 0x4E00 and cp <= 0x9FFF) or (cp >= 0x3400 and cp <= 0x4DBF) then
            han = han + 1
        elseif cp >= 0x0400 and cp <= 0x04FF then
            cyr = cyr + 1
            if cp == 0x0457 or cp == 0x0454 or cp == 0x0456 or cp == 0x0491 then
                ukrainian = ukrainian + 1
            end
        elseif (cp >= 65 and cp <= 90) or (cp >= 97 and cp <= 122) or (cp >= 0xC0 and cp <= 0x24F) then
            latin = latin + 1
        end
        i = i + len
    end
    if hangul > 0 and hangul >= han then
        return "ko"
    elseif kana > 0 then
        return "ja"
    elseif han > 0 then
        return "zh"
    elseif cyr > 0 and cyr >= latin then
        return ukrainian > 0 and "uk" or "ru"
    elseif latin == 0 then
        return nil
    end
    return Lang.DetectLatin(s)
end

-- UTF-8 characters of a word.
local function Chars(word)
    local chars, i = {}, 1
    while i <= #word do
        local _, len = CodePoint(word, i)
        chars[#chars + 1] = sub(word, i, i + len - 1)
        i = i + len
    end
    return chars
end

-- Best Latin-script language of plain text: trigram log-probabilities plus hints.
function Lang.DetectLatin(text)
    local data = ns.LangData
    local score = {}
    for lang in pairs(data.trigrams) do
        score[lang] = 0
    end
    score.en = (score.en or 0) + ENGLISH_PRIOR
    text = text:gsub("https?://%S+", " "):lower()
    for word in text:gmatch("[%a\128-\255']+") do
        word = word:gsub("^'+", ""):gsub("'+$", "")
        if word ~= "" then
            if SLANG[word] then
                score.en = score.en + SLANG_BONUS
            else
                for _, lang in ipairs(HINT[word] or {}) do
                    score[lang] = score[lang] + WORD_BONUS
                end
                local chars = Chars(word)
                table.insert(chars, 1, " ")
                chars[#chars + 1] = " "
                for k = 1, #chars - 2 do
                    local g = chars[k] .. chars[k + 1] .. chars[k + 2]
                    for lang, table3 in pairs(data.trigrams) do
                        score[lang] = score[lang] + (table3[g] or data.floor[lang])
                    end
                end
            end
        end
    end
    local best, bestScore = "en", score.en
    for lang, value in pairs(score) do
        if value > bestScore then
            best, bestScore = lang, value
        end
    end
    return best
end

-- Model packs to run, in order, or nil and a reason ("same", "unsupported").
function Lang.Chain(source, target)
    if not source then
        return nil, "nothing"
    elseif source == target then
        return nil, "same"
    end
    local chain = {}
    if source ~= "en" then
        chain[#chain + 1] = source .. "-en"
    end
    if target ~= "en" then
        chain[#chain + 1] = "en-" .. target
    end
    for _, pair in ipairs(chain) do
        if not ns.Engine.HasPack(pair) then
            return nil, "unsupported", pair
        end
    end
    return chain
end

-- Default target: the client language when we can translate into it, else English.
function Lang.DefaultTarget()
    local locale = (GetLocale() or "enUS"):sub(1, 2)
    for _, t in ipairs(Lang.TARGETS) do
        if t == locale then
            return t
        end
    end
    return "en"
end
