-- Target languages of the translator and the writing script of a chat line. The helper's
-- model translates from any language; the script only sizes the space reserved under a line.
local _, ns = ...

local Lang = {}
ns.Lang = Lang

local byte = string.byte

-- Languages the user can pick as target (codes sent to the helper).
Lang.TARGETS = { "en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW" }
Lang.NAMES = {
    en = "EN", fr = "FR", de = "DE", es = "ES", it = "IT", pt = "PT", ru = "RU", ko = "KO", zh = "ZH", ["zh-TW"] = "ZH-TW",
}

local BY_LOCALE = {
    enUS = "en", enGB = "en", frFR = "fr", deDE = "de", esES = "es", esMX = "es", itIT = "it",
    ptBR = "pt", ptPT = "pt", ruRU = "ru", koKR = "ko", zhCN = "zh", zhTW = "zh-TW",
}

-- Default target: the client language when it is a target, else English.
function Lang.DefaultTarget()
    return BY_LOCALE[GetLocale() or ""] or "en"
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

-- Main script of the words a player typed: "latin", "cyrillic", "hangul", "kana", "han", or
-- nil when the line has no letters.
function Lang.Script(text)
    local s = ns.Glossary.PlainText(text)
    local hangul, kana, han, cyr, latin = 0, 0, 0, 0, 0
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
        elseif (cp >= 65 and cp <= 90) or (cp >= 97 and cp <= 122) or (cp >= 0xC0 and cp <= 0x24F) then
            latin = latin + 1
        end
        i = i + len
    end
    if hangul > 0 and hangul >= han then
        return "hangul"
    elseif kana > 0 then
        return "kana"
    elseif han > 0 then
        return "han"
    elseif cyr > 0 and cyr >= latin then
        return "cyrillic"
    elseif latin == 0 then
        return nil
    end
    return "latin"
end

-- Script a target language is written in.
function Lang.TargetScript(code)
    if code == "zh" or code == "zh-TW" then
        return "han"
    elseif code == "ko" then
        return "hangul"
    elseif code == "ru" then
        return "cyrillic"
    end
    return "latin"
end
