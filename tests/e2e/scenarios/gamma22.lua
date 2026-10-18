-- gamma22: the basic scenario seen through WoW Gamma 2.2 (fakewow applies
-- out = 255 * (in / 255) ^ (1 / 2.2) to everything it draws).
local E = E2E.Start({ name = "gamma22", gamma = 2.2 })
local ids = E.Filler()
E.Wait(2)
E.Click(ids.party)
E.Wait(1.5)
E.Click(ids.trade)
E.Wait(1.5)
E.Click(ids.long)
E.Wait(8)
E.Finish(2)
