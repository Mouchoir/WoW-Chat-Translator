-- Luacheck config for WoW Forever addon Lua (Lua 5.1 runtime, 12.x API).
std = "lua51"
max_line_length = false   -- localization tables have long string lines
unused_args = false       -- WoW callbacks pass self/event args we often ignore
exclude_files = { "node_modules", "tests" }

read_globals = {
    -- Lua helpers the game adds
    "wipe", "print", "time", "issecretvalue", "canaccessvalue", "GetTime", "bit", "math",

    -- Frames, timers, client
    "CreateFrame", "UIParent", "C_Timer", "GetLocale", "GetBuildInfo", "WOW_PROJECT_ID", "GetPhysicalScreenSize",
    "GetCVar", "IsShiftKeyDown", "UnitGUID", "C_Intl", "Enum", "GameTooltip",

    -- Chat
    "C_ChatInfo", "ChatFrameUtil", "EventRegistry", "DEFAULT_CHAT_FRAME",
}

globals = {
    -- SavedVariable and slash handlers (globals WoW reads/writes)
    "TranslateProbeDB", "SLASH_TRANSLATEPROBE1", "SLASH_TRANSLATEPROBE2", "SlashCmdList",
}
