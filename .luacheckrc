-- Luacheck config for WoW Forever addon Lua (Lua 5.1 runtime, 12.x API).
std = "lua51"
max_line_length = false   -- localization tables have long string lines
unused_args = false       -- WoW callbacks pass self/event args we often ignore
exclude_files = { "node_modules", "tests" }

read_globals = {
    -- Lua helpers the game adds
    "wipe", "print", "date", "time", "issecretvalue", "debugprofilestop", "debugstack", "geterrorhandler",
    "GetTime", "ERROR_SCRIPT_EXECUTION_LIMIT",
    table = { fields = { "create" } },

    -- Frames, timers, client
    "CreateFrame", "C_Timer", "GetLocale", "GetFramerate", "GetBuildInfo", "WOW_PROJECT_ID",
    "InCombatLockdown", "IsShiftKeyDown", "UnitGUID", "GetInstanceInfo", "GetRealZoneText",
    "GetScriptBucketThrottleLimits", "C_AddOnProfiler", "UpdateAddOnMemoryUsage", "GetAddOnMemoryUsage",
    "C_RestrictedActions", "C_InstanceEncounter", "C_PvP", "C_Intl", "Enum",

    -- Chat
    "C_ChatInfo", "ChatFrameUtil", "EventRegistry", "C_EncodingUtil", "C_AddOns",
}

globals = {
    -- SavedVariable and slash handlers (globals WoW reads/writes)
    "TranslateProbeDB", "SLASH_TRANSLATEPROBE1", "SLASH_TRANSLATEPROBE2", "SlashCmdList",
    "TranslateProbeModels",   -- filled by the LoadOnDemand model packs
}
