-- dropped: the strip is not painted while one request is shown, so the Helper never receives it.
-- Its rows must show "Not received" once seen for 1.5 s; a plain re-click sends the same request
-- again and the card is translated.
local E = E2E.Start({ name = "dropped" })
local ids = E.Filler()
E.Wait(2)
E.Click(ids.guild)
E.Wait(2)
E.Mark("this request never reaches the Helper")
E.Click(ids.whisper, { drop = true })
E.Wait(5)
E.Mark("click [T] again: the same request is sent again")
E.Click(ids.whisper)
E.Wait(5)
E.Finish(2)
