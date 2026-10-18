-- late_start: three clicks before the Helper runs (it starts 7 s into the scenario). It never
-- sees those requests (nor the VIEW), so it finds the rows by its discovery scan and shows "Not
-- received"; a re-click on one line repairs it.
local E = E2E.Start({ name = "late_start", helperStartMs = 7000 })
local ids = E.Filler()
E.Wait(1.5)
E.Click(ids.party)
E.Wait(1)
E.Click(ids.fr)
E.Wait(1)
E.Click(ids.whisper)
E.Wait(9.5)
E.Mark("click [T] again on one line")
E.Click(ids.fr)
E.Wait(5)
E.Finish(2)
