-- Translate Companion's status file (docs\ADDON.md, "Translate Companion presence"). The addon
-- ships it empty. Translate Companion overwrites the deployed copy of this one file, atomically,
-- with a table of numbers and short ASCII strings:
--   { v = 1, version = "0.6.0", build = 6, state = "running" or "stopped", heartbeat = <unix seconds> }
-- at its start, every 60 s while it runs and on a clean exit. WoW reads it at login and /reload
-- only (Presence.lua); nothing else in this folder is ever written by it.
TranslateProbe_CompanionStatus = nil
