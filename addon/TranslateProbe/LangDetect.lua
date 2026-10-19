-- Source language of chat lines, for the conversation window (header "DE -> FR", the reply
-- language) and the reply's paste checks. Restored from the trigram detector of commit 8b7750e
-- (removed with the Lua translator in c12369e) and adapted:
-- * the writing system decides first (Hangul, kana, Han, Cyrillic), as Lang.Script does;
-- * Latin text is scored with the character trigram profiles of LangData.lua (en, de, fr, es,
--   it, pt), plus everyday chat words of each language, English game slang and a small prior
--   toward English, the usual language of public channels;
-- * "unknown" (nil) instead of a guess: 2 words or fewer with no known word, a best score that
--   leads the second by less than 15% of the evidence (8% for long lines; see Lead below), or a
--   best language whose profile fits the line badly (a language none of the profiles knows); a
--   line of 2 words or fewer whose known words all point to one language is that language;
-- * per conversation, the scores of the sender's last lines are summed (Merge, Decide).
-- Detection runs only for lines added to a conversation and on paste checks (about 1.6 ms per
-- long line). Nothing is stored: scores live in the conversation's memory only.
local _, ns = ...

local LangDetect = {}
ns.LangDetect = LangDetect

local byte, sub = string.byte, string.sub

local ENGLISH_PRIOR = 2.0
local SLANG_BONUS = 3.0
local WORD_BONUS = 6.0
local LEAD = 0.15               -- the best language must lead the second by 15% of the evidence...
local LONG_LEAD = 0.08          -- ...going down to 8% for long lines (LONG_TRIGRAMS), which carry more
local SHORT_TRIGRAMS, LONG_TRIGRAMS = 40, 100
local FIT_MIN = -7.95           -- mean trigram log-probability of the best language below this, over
local FIT_TRIGRAMS = 30         -- at least this many trigrams: a language none of the profiles knows
local SHORT_WORDS = 2           -- up to this many words, a line needs a known word
LangDetect.LEAD = LEAD

-- Lead needed for a line with this many trigrams (game words in English shrink the lead of long
-- German or French ads: 0.12-0.14 measured on 2026-10-19).
local function NeededLead(trigrams)
    local t = math.min(1, math.max(0, (trigrams - SHORT_TRIGRAMS) / (LONG_TRIGRAMS - SHORT_TRIGRAMS)))
    return LEAD - (LEAD - LONG_LEAD) * t
end
LangDetect.NeededLead = NeededLead

-- English game slang: counts for English and is not scored by trigrams.
local SLANG = {}
for w in ("lf lfg lfm wts wtb wtt pst inv pls plz ty thx np gg wp gz afk brb omw idk nvm lol ok kk rdy "
    .. "bg dm tank heal healer dps ffs omg wtf u ur im dont cant wanna gonna sec"):gmatch("%S+") do
    SLANG[w] = true
end

-- Short everyday chat words that point to one language (a word listed for two languages helps
-- neither). The additions to the 8b7750e lists are the lines that detector got wrong ("moin",
-- "vamos", "valeu", "j'arrive", "c'est", "arrivo") and common words of the shared corpus
-- (tests\vectors\lang_corpus.json). Words that are also English ("was", "die", "come", "on") are
-- left out: they would pull English lines away.
local WORDS = {
    en = "hello hi hey yes yeah no thanks thank you please insane nice sorry mate guys anyone where what sure "
        .. "the and you your is are for with this that have can need know how which again",
    de = "danke bitte ja nein hallo moin servus klar genau jemand gruppe suche brauche wer wo ich bin nicht "
        .. "und ist auch noch gleich wieder kann für gut zusammen keine sucht mitglieder mehr kein bei zum einen eine wir uns heute abend morgen jo",
    fr = "merci oui non salut bonjour bonsoir besoin cherche quelqu'un groupe svp stp mdr ouais c'est j'arrive "
        .. "je pour avec pas trop qqn désolé bonne nuit les des une recrute recherche recherchés joueurs motivés objectif joué dans nous vous sur",
    es = "hola gracias sí vale alguien grupo busco necesito dónde amigo vamos buenas noches todos ahora "
        .. "vuelvo el los muy aquí vendo barato siento tengo cuánto cuesta desde",
    it = "ciao grazie sì qualcuno gruppo cerco arrivo andiamo scusate grazie mille buonanotte tutti c'è per "
        .. "dove questa subito qui da sono",
    pt = "olá obrigado obrigada sim alguém grupo procuro valeu galera lá você vc não pra pelo onde fica boa "
        .. "pessoal já volto chego daqui vendo barato minério desde",
}
local HINT = {}
for lang, list in pairs(WORDS) do
    for w in list:gmatch("%S+") do
        HINT[w] = HINT[w] or {}
        local listed = false
        for _, l in ipairs(HINT[w]) do
            listed = listed or l == lang
        end
        if not listed then
            HINT[w][#HINT[w] + 1] = lang
        end
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

-- Writing system of plain text: a language code for Hangul, kana, Han and Cyrillic ("ko", "ja",
-- "zh", "ru", "uk"), "latin", or nil when it has no letters.
local function WritingSystem(s)
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
    return "latin"
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

-- Lowercase for ASCII and the Latin-1 accented capitals (string.lower only knows ASCII).
local function Lower(s)
    s = s:lower()
    return (s:gsub("\195([\128-\158])", function(c)
        local b = byte(c)
        if b == 0x97 then   -- the multiplication sign has no lowercase
            return "\195" .. c
        end
        return "\195" .. string.char(b + 32)
    end))
end

local function NewScores()
    return { lang = {}, tri = {}, hints = {}, script = nil, words = 0, known = 0, trigrams = 0 }
end

-- Scores of one line: { lang = { [code] = score } (Latin languages), hints = { [code] = number of
-- hint or slang words for it }, script = code or nil (a non-Latin writing system decided), words,
-- known (words found in the hint or slang lists), trigrams }, and the word count.
function LangDetect.Scores(text)
    local scores = NewScores()
    if type(text) ~= "string" or issecretvalue(text) then
        return scores, 0
    end
    local s = ns.Glossary.PlainText(text):gsub("https?://%S+", " ")
    local system = WritingSystem(s)
    for _ in s:gmatch("[%a\128-\255']+") do
        scores.words = scores.words + 1
    end
    if system == nil then
        return scores, scores.words
    elseif system ~= "latin" then
        scores.script = system
        return scores, scores.words
    end
    local data = ns.LangData
    local score = scores.lang
    for lang in pairs(data.trigrams) do
        score[lang] = 0
    end
    score.en = (score.en or 0) + ENGLISH_PRIOR
    for word in Lower(s):gmatch("[%a\128-\255']+") do
        word = word:gsub("^'+", ""):gsub("'+$", "")
        if word ~= "" then
            if SLANG[word] then
                score.en = score.en + SLANG_BONUS
                scores.known = scores.known + 1
                scores.hints.en = (scores.hints.en or 0) + 1
            else
                local hint = HINT[word]
                if hint then
                    scores.known = scores.known + 1
                    for _, lang in ipairs(hint) do
                        score[lang] = score[lang] + WORD_BONUS
                        scores.hints[lang] = (scores.hints[lang] or 0) + 1
                    end
                end
                local chars = Chars(word)
                table.insert(chars, 1, " ")
                chars[#chars + 1] = " "
                for k = 1, #chars - 2 do
                    local g = chars[k] .. chars[k + 1] .. chars[k + 2]
                    scores.trigrams = scores.trigrams + 1
                    for lang, table3 in pairs(data.trigrams) do
                        local p = table3[g] or data.floor[lang]
                        score[lang] = score[lang] + p
                        scores.tri[lang] = (scores.tri[lang] or 0) + p
                    end
                end
            end
        end
    end
    return scores, scores.words
end

-- Adds a line's scores to an accumulator (a new one when acc is nil); returns it.
function LangDetect.Merge(acc, scores)
    acc = acc or NewScores()
    acc.votes = acc.votes or {}
    acc.lines = (acc.lines or 0) + 1
    if scores.script then
        acc.votes[scores.script] = (acc.votes[scores.script] or 0) + 1
    elseif next(scores.lang) then
        acc.latinLines = (acc.latinLines or 0) + 1
        for lang, value in pairs(scores.lang) do
            acc.lang[lang] = (acc.lang[lang] or 0) + value
        end
        for lang, n in pairs(scores.hints) do
            acc.hints[lang] = (acc.hints[lang] or 0) + n
        end
        for lang, value in pairs(scores.tri) do
            acc.tri[lang] = (acc.tri[lang] or 0) + value
        end
    end
    acc.words = acc.words + scores.words
    acc.known = acc.known + scores.known
    acc.trigrams = acc.trigrams + scores.trigrams
    return acc
end

-- How far the best Latin language leads the second, as a share of the evidence: the score gap
-- over the gap between the second language and the floor of a line of unknown trigrams (so a
-- long line, whose raw scores grow with its length, is measured on the same scale as a short
-- one). Returns best, lead (0..), second.
function LangDetect.Lead(acc)
    local best, bestScore, second, secondScore
    for lang, value in pairs(acc.lang) do
        if not bestScore or value > bestScore then
            second, secondScore = best, bestScore
            best, bestScore = lang, value
        elseif not secondScore or value > secondScore then
            second, secondScore = lang, value
        end
    end
    if not best then
        return nil, 0
    end
    if not secondScore then
        return best, math.huge
    end
    local floor = 0
    for _, f in pairs(ns.LangData.floor) do
        floor = math.min(floor, f)
    end
    -- the evidence: how far the second language sits above a line made only of unknown trigrams
    local base = floor * acc.trigrams - 1
    local evidence = math.max(1, secondScore - base)
    return best, (bestScore - secondScore) / evidence, second
end

-- Language of an accumulator: the writing system when most lines used one, else the best Latin
-- language, or nil (unknown) by the rules at the top of this file.
function LangDetect.Decide(acc)
    if not acc then
        return nil
    end
    local topScript, topVotes = nil, 0
    for code, n in pairs(acc.votes or {}) do
        if n > topVotes then
            topScript, topVotes = code, n
        end
    end
    if topScript and topVotes >= (acc.latinLines or 0) then
        return topScript
    end
    if not next(acc.lang) then
        return nil
    end
    if acc.words <= SHORT_WORDS then
        -- a short line is decided by its known words when they all point to one language
        -- ("moin", "vale", "obrigado"), and is unknown without one
        local only, count = nil, 0
        for lang in pairs(acc.hints) do
            only, count = lang, count + 1
        end
        if count == 1 then
            return only
        elseif count == 0 then
            return nil
        end
    end
    local best, lead = LangDetect.Lead(acc)
    if lead < NeededLead(acc.trigrams) then
        return nil
    end
    -- Finnish, Swedish, Polish, Dutch...: the best profile still fits badly, so say unknown
    -- rather than a wrong language
    if acc.trigrams >= FIT_TRIGRAMS and acc.tri[best] and acc.tri[best] / acc.trigrams < FIT_MIN then
        return nil
    end
    return best
end

-- Language of one line, or nil (unknown, or no letters).
function LangDetect.Detect(text)
    return LangDetect.Decide(LangDetect.Merge(nil, (LangDetect.Scores(text))))
end

-- The writing system alone ("latin", a language code, or nil): used to tell a Japanese or
-- Ukrainian sender apart from the translator's targets.
function LangDetect.WritingSystem(text)
    if type(text) ~= "string" or issecretvalue(text) then
        return nil
    end
    return WritingSystem(ns.Glossary.PlainText(text))
end
