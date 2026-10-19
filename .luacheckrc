-- Luacheck config for WoW Forever addon Lua (Lua 5.1 runtime, 12.x API).
std = "lua51"
max_line_length = false   -- localization tables have long string lines
unused_args = false       -- WoW callbacks pass self/event args we often ignore
exclude_files = { "node_modules", "tests" }

read_globals = {
    -- Lua helpers the game adds
    "wipe", "print", "time", "date", "issecretvalue", "canaccessvalue", "GetTime", "GetServerTime", "bit", "math",
    "strtrim", "strsplit", "strlenutf8", "tContains", "tinsert", "tremove", "CopyTable", "Round", "Clamp",
    "Mixin", "CreateFromMixins", "CreateColor",

    -- Frames, timers, client
    "CreateFrame", "CreateFont", "UIParent", "C_Timer", "GetLocale", "GetBuildInfo", "WOW_PROJECT_ID",
    "GetPhysicalScreenSize", "GetScreenWidth", "GetScreenHeight", "GetCVar", "C_Intl", "Enum", "Constants",
    "GameTooltip", "GameTooltip_SetTitle", "GameTooltip_AddNormalLine", "GameTooltip_AddHighlightLine",
    "GameTooltip_AddInstructionLine", "GameTooltip_AddErrorLine", "GameTooltip_AddColoredLine", "GameTooltip_Hide",
    "GetAppropriateTooltip", "PlaySound", "SOUNDKIT", "PixelUtil", "NineSliceUtil", "C_Texture", "C_XMLUtil",
    "ScrollUtil", "HideUIPanel",

    -- Input and combat state
    "IsShiftKeyDown", "IsControlKeyDown", "IsAltKeyDown", "InCombatLockdown", "UnitAffectingCombat",
    "GetCurrentKeyBoardFocus",

    -- Units, players, groups
    "UnitGUID", "UnitName", "GetRealmName", "GetNormalizedRealmName", "GetPlayerInfoByGUID", "Ambiguate",
    "IsInGroup", "IsInRaid", "IsInGuild", "IsInInstance", "C_GuildInfo", "CanEditOfficerNote",
    "LE_PARTY_CATEGORY_HOME", "LE_PARTY_CATEGORY_INSTANCE", "RAID_CLASS_COLORS", "C_ClassColor",
    "CLASS_ICON_TCOORDS", "LOCALIZED_CLASS_NAMES_MALE",

    -- Chat
    "C_ChatInfo", "C_BattleNet", "ChatFrameUtil", "EventRegistry", "DEFAULT_CHAT_FRAME", "ChatTypeInfo",
    "GetChannelName", "GetChannelList", "C_RestrictedActions",

    -- Links of a message (Links.lua): what a chat frame does with a click, item quality colours
    "SetItemRef", "IsModifiedClick", "GetFixedLink", "ChatEdit_InsertLink", "ColorManager", "C_ColorOverrides",
    "C_Item", "ITEM_QUALITY_COLORS",

    -- Templates' global functions
    "PanelTemplates_SetNumTabs", "PanelTemplates_SetTab", "PanelTemplates_GetSelectedTab",
    "PanelTemplates_UpdateTabs", "PanelTemplates_TabResize", "PanelTemplates_ResizeTabsToFit",
    "PanelTemplates_AnchorTabs", "PanelTemplates_SelectTab", "PanelTemplates_DeselectTab",
    "PanelTemplates_EnableTab", "PanelTemplates_DisableTab", "PanelTemplates_SetTabEnabled",
    "PanelTemplates_HideTab", "PanelTemplates_ShowTab", "PanelTemplates_SetTabShown", "PanelTemplates_GetTabWidth",
    "InputBoxInstructions_OnTextChanged",

    -- Font objects and colours
    "GameFontNormal", "GameFontNormalSmall", "GameFontNormalLarge", "GameFontHighlight", "GameFontHighlightSmall",
    "GameFontDisable", "GameFontDisableSmall", "ChatFontNormal", "ChatFontSmall",
    "NORMAL_FONT_COLOR", "HIGHLIGHT_FONT_COLOR", "GRAY_FONT_COLOR", "GREEN_FONT_COLOR", "RED_FONT_COLOR",
    "ORANGE_FONT_COLOR", "DISABLED_FONT_COLOR", "WHITE_FONT_COLOR", "YELLOW_FONT_COLOR", "LIGHTGRAY_FONT_COLOR",

    -- Blizzard global strings (localized by the client)
    "SAY", "YELL", "WHISPER", "EMOTE", "PARTY", "RAID", "RAID_WARNING", "INSTANCE_CHAT", "GUILD", "OFFICER",
    "TRADE", "CHANNEL", "CLOSE", "OKAY", "CANCEL", "ACCEPT", "DECLINE", "HUD_EDIT_MODE_RESET_POSITION",
    "ERR_CHAT_PLAYER_NOT_FOUND_S",
}

globals = {
    -- SavedVariable and slash handlers (globals WoW reads/writes)
    "TranslateProbeDB", "SLASH_TRANSLATEPROBE1", "SLASH_TRANSLATEPROBE2", "SlashCmdList",
    -- Escape closes the frames listed here (the addon appends its window, guarded with tContains)
    "UISpecialFrames",
    -- Key bindings (Bindings.xml): header and names shown in the key binding panel, and the
    -- functions the bindings call
    "BINDING_HEADER_TRANSLATEPROBE", "BINDING_NAME_TRANSLATEPROBE_CONV_TOGGLE", "BINDING_NAME_TRANSLATEPROBE_CONV_REPLY",
    "TranslateProbe_ConvToggle", "TranslateProbe_ConvReply",
}
