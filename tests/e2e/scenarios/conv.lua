-- conv: the conversation window (design A.4, A.6.4 and B.4). Three incoming bubbles over the opaque
-- bubble fill, the window drawn over a chat frame (one capture region: each tag must be kept
-- once, in the window's view), then a reply card sent with flags 14 (plate, reply, copy): row 0
-- must say "not copied - off" (every E2E run uses --no-clipboard) and the hint lane under the card
-- must be painted over. Last, a scroll leaves the first bubble half out: its hidden row gets no
-- band, its visible row keeps its number. All text is synthetic.
local E = E2E.Start({ name = "conv" })
E.Filler()
E.Wait(1.5)
E.Mark("window with three bubbles")
E.Window({
    bubbles = {
        { slot = 100, rows = 2, text = "anyone up for the mine run tonight? we still need a healer" },
        { slot = 101, rows = 3, text = "i can tank but i have to leave at ten, is that fine with everyone in the group" },
        { slot = 102, rows = 1, text = "sure, see you at the gate" },
    },
})
E.Wait(3)
E.Mark("reply card, flags 14")
E.WindowReply({ rows = 3, text = "on part dans 5 min ?", copy = true })
E.Wait(3)
E.Mark("scroll: the first bubble half out")
E.WindowScroll(30)
E.Wait(3)
E.Finish(2)
