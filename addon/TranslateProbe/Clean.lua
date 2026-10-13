-- Tidies a chat line before it reaches a translation model. The small models were trained on
-- clean text: a lowercase "i", missing apostrophes ("dont") or chat shorthand ("u", "idk")
-- derail them. English lines get word fixes; every Latin line gets a capital first letter
-- and a final full stop when it has none. Game abbreviations (LFM, DM...) are left alone:
-- the glossary line explains them.
local _, ns = ...

local Clean = {}
ns.Clean = Clean

local ENGLISH = {
    i = "I", im = "I'm", ive = "I've",
    dont = "don't", doesnt = "doesn't", didnt = "didn't", cant = "can't", wont = "won't", isnt = "isn't",
    arent = "aren't", wasnt = "wasn't", werent = "weren't", havent = "haven't", hasnt = "hasn't",
    hadnt = "hadn't", wouldnt = "wouldn't", shouldnt = "shouldn't", couldnt = "couldn't",
    youre = "you're", ure = "you're", theyre = "they're", thats = "that's", whats = "what's", whos = "who's",
    theres = "there's", u = "you", ya = "you", ur = "your", r = "are", pls = "please", plz = "please",
    thx = "thanks", ty = "thank you", ppl = "people", bc = "because", cuz = "because", coz = "because",
    idk = "I don't know", nvm = "never mind", ofc = "of course", tho = "though", rn = "right now",
    atm = "at the moment", abt = "about", gonna = "going to", wanna = "want to", gotta = "have to",
    dunno = "don't know", smth = "something", sth = "something", sry = "sorry", np = "no problem",
    btw = "by the way", imo = "in my opinion", tbh = "to be honest",
}

-- Text with English shorthand expanded (whole words, any case except ALL CAPS game tags).
local function FixEnglish(text)
    return (text:gsub("%f[%a'][%a']+", function(word)
        local fixed = ENGLISH[word:lower()]
        if fixed and (word == word:lower() or #word == 1) then
            return fixed
        end
        return word
    end))
end

-- Capital first letter and a final full stop for a line that ends with a letter or digit.
local function Sentence(text)
    text = text:gsub("^%s+", ""):gsub("%s+$", "")
    text = text:gsub("^(%l)", string.upper)
    if text:find("[%w\128-\255]$") then
        text = text .. "."
    end
    return text
end

-- Prepared input for the first model of a chain, given the detected source language.
function Clean.ForTranslation(text, source)
    if source == "en" then
        text = FixEnglish(text)
    end
    if source == "en" or source == "fr" or source == "de" or source == "es" or source == "it" or source == "pt" then
        text = Sentence(text)
    end
    return text
end
