-- presence: the Translate Companion popup with Translate Companion running (user request R1,
-- 2026-10-19). The popup's VIEW 201 (sent when it opens and every 2 s, as the addon does) leads the
-- app to the status card of slot 125, which it paints "Translate Companion detected" within about
-- a second; when the popup closes (Continue) the card goes, and the click it held runs (inline rows:
-- a VIEW and a TRANSLATE). All text is synthetic.
local E = E2E.Start({ name = "presence" })
local ids = E.Filler()
E.Wait(1.5)
E.Mark("popup with the status card (VIEW 201, again every 2 s)")
E.Popup({ resend = 2 })
E.Wait(4)
E.Mark("Continue: the popup closes, the held click runs")
E.PopupClose()
E.Click(ids.fr)
E.Wait(4)
E.Finish(2)
