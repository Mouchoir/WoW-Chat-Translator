-- scroll: three cards, then new lines push them up and out of the chat, then the chat scrolls up
-- and back down, then new lines push every card out of the top. Cards follow their plates and hide
-- once a plate leaves the chat; a hint lane whose card's rows left the top stays covered.
local E = E2E.Start({ name = "scroll" })
local ids = E.Filler()
E.Wait(2)
E.Click(ids.guild)
E.Wait(1.2)
E.Click(ids.fr)
E.Wait(1.2)
E.Click(ids.whisper)
E.Wait(3)
E.Mark("new lines push the cards up")
for i = 1, 8 do
    E.Say("filler line number " .. i .. " about nothing in particular", "Hollis", "CHAT_MSG_SAY")
    E.Wait(0.6)
end
E.Wait(2)
E.Mark("scroll up 4 messages")
E.Scroll(ChatFrame1, 4)
E.Wait(3)
E.Mark("scroll up 12 messages")
E.Scroll(ChatFrame1, 12)
E.Wait(3)
E.Mark("back to the bottom")
E.Scroll(ChatFrame1, 0)
E.Wait(3)
-- every card's rows leave the top of the chat one line at a time: for a while each card's hint lane
-- is the chat's top line with no row of its card above it (an orphan lane, 2026-10-19)
E.Mark("new lines push the cards out of the top")
for i = 1, 26 do
    E.Say("filler line number " .. (8 + i) .. " about the weather", "Hollis", "CHAT_MSG_SAY")
    E.Wait(0.7)
end
E.Wait(1)
E.Finish(2)
