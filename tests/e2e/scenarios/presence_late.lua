-- presence_late: Translate Companion starts while the popup is open (user request R1, 2026-10-19:
-- no /reload). The popup and its VIEW 201 come 3 s before the app, and the VIEW is not sent again:
-- the app has no VIEW and must find the card in the middle of the client by its discovery scan
-- (docs\HELPER.md, "Presence card"), then paint it within about a second of its start. Continue
-- then closes the popup and the held click runs. All text is synthetic.
local E = E2E.Start({ name = "presence_late", helperStartMs = 4000 })
local ids = E.Filler()
E.Wait(1)
E.Mark("popup (VIEW 201 sent once, before the app runs)")
E.Popup({ resend = 0 })
E.Wait(7)
E.Mark("Continue: the popup closes, the held click runs")
E.PopupClose()
E.Click(ids.fr)
E.Wait(4)
E.Finish(2)
