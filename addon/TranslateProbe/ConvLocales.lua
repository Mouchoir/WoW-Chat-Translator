-- User-visible strings of the conversation window (tabs, header, thread, bubbles, popups), added
-- to ns.L: English first, then the frFR overrides, as Locales.lua does. The reply composer's
-- strings live in ReplyLocales.lua. The French wording follows the mockup the user approved
-- (formal "vous", as the game itself); the chat-side strings of Locales.lua keep "tu".
-- Key binding names (Bindings.xml) are globals the key binding panel reads.
local _, ns = ...
local L = ns.L

local en = {
    CONV_TITLE = "Conversations",
    CONV_EMPTY = "Click [T] after a chat line to start a conversation.",
    CONV_TIP_T_WINDOW = "Translate this line in the conversation window (needs Translate Companion). Ctrl-click: under the line. Shift-click: translate again.",
    CONV_TIP_T_INLINE = "Translate this line (needs Translate Companion). Ctrl-click: in the conversation window. Shift-click: translate again.",
    CONV_FIRST_TIP = "Conversation window: [T] adds a player's line here and Translate Companion translates it. Ctrl-click [T] to translate under the line instead. Middle-click a tab to close it.",
    CONV_FULL = "too many conversations are kept: close a tab first.",
    CONV_TOO_OLD = "this line is too old: the client no longer has its text.",

    CONV_FOLLOW = "Translate new messages",
    CONV_FOLLOW_TIP = "Translate this player's next messages automatically while this tab shows them.",
    CONV_VIA = "via %s",
    CONV_ARROW = ">",
    CONV_YOUR_LANG = "Your language. Change it in Options.",
    CONV_LANG_TIP = "Their language, detected from their last messages. Click to choose.",
    CONV_LANG_SET_TIP = "Their language, chosen by you. Click to change.",
    CONV_LANG_UNKNOWN_TIP = "Language not recognized. Click to choose.",
    CONV_LANG_NOT_TARGET = "%s is not a language the translator writes: your reply needs another language.",
    CONV_THEY_WRITE = "They write in:",
    CONV_AUTOMATIC = "Automatic (%s)",
    CONV_AUTOMATIC_UNKNOWN = "Automatic (not recognized)",

    CONV_TAB_HINT = "Middle-click to close. Right-click for options.",
    CONV_TAB_CHANNEL = "Last message: %s",
    CONV_TAB_CLOSE = "Close",
    CONV_TAB_CLOSE_OTHERS = "Close the others",
    CONV_CLOSE_TIP = "Close this conversation. Its messages come back if this player writes again (until /reload).",
    CONV_OVERFLOW_TIP = "More conversations",
    CONV_NEW_ONE = "1 new message",
    CONV_NEW_MANY = "%d new messages",
    CONV_BANNER = "Chat is restricted: new messages will be offered when it ends.",

    CONV_ORIGINAL = "Original",
    CONV_HIDE = "Hide",
    CONV_TRANSLATE = "Translate",
    CONV_BUBBLE_HINT_CLICK = "Click: translate again (more lines if cut).",
    CONV_BUBBLE_HINT_SHIFT = "Shift-click: without the cache. Right-click: more.",
    CONV_BUBBLE_HINT_PLAIN = "Click Translate to translate this message.",
    CONV_HELPER_TIP = "Coloured blocks? Translate Companion is not running.",
    CONV_MENU_RETRANSLATE = "Translate again",
    CONV_MENU_COPY = "Copy the original",
    CONV_MENU_REMOVE = "Remove",
    CONV_COPY_HINT = "Press Ctrl+C to copy, then Escape.",

    CONV_YOU_TO = "You > %s",
    CONV_YOU_WROTE = "You wrote: %s",
    CONV_OUT_SENDING = "Sending...",
    CONV_OUT_SENT = "Sent",
    CONV_OUT_UNCONFIRMED = "Not confirmed",
    CONV_OUT_OFFLINE = "Not delivered: %s is offline",
    CONV_OUT_BLOCKED = "Blocked",

    CONV_YOUR_TEXT = "Your text:",
    CONV_EDIT = "Edit",
    CONV_CANCEL_TIP = "Cancel this reply",
    CONV_LANE_HINT = "No status above? Start Translate Companion.",
    -- the hint lane under an incoming bubble's rows (user request U1), its short form when the
    -- bubble is too narrow, and the button that opens the Translate Companion dialog
    CONV_LANE_IN = "No translation above? Start Translate Companion.",
    CONV_LANE_SHORT = "Start Translate Companion.",
    CONV_DOWNLOAD = "Download",
    CONV_CLEARED = "conversations cleared.",

    CONV_BINDING_HEADER = "TranslateProbe",
    CONV_BINDING_TOGGLE = "Show/hide the conversation window",
    CONV_BINDING_REPLY = "Reply in the conversation window",
}

local frFR = {
    CONV_TITLE = "Conversations",
    CONV_EMPTY = "Cliquez sur [T] après une ligne du chat pour commencer une conversation.",
    CONV_TIP_T_WINDOW = "Traduire cette ligne dans la fenêtre de conversation (il faut Translate Companion). Ctrl-clic : sous la ligne. Maj-clic : retraduire.",
    CONV_TIP_T_INLINE = "Traduire cette ligne (il faut Translate Companion). Ctrl-clic : dans la fenêtre de conversation. Maj-clic : retraduire.",
    CONV_FIRST_TIP = "Fenêtre de conversation : [T] ajoute ici la ligne d'un joueur et Translate Companion la traduit. Ctrl-clic sur [T] pour traduire sous la ligne. Clic milieu sur un onglet pour le fermer.",
    CONV_FULL = "trop de conversations sont gardées : fermez d'abord un onglet.",
    CONV_TOO_OLD = "cette ligne est trop ancienne : le client n'a plus son texte.",

    CONV_FOLLOW = "Traduire ses nouveaux messages",
    CONV_FOLLOW_TIP = "Traduire automatiquement les prochains messages de ce joueur quand cet onglet les affiche.",
    CONV_VIA = "via %s",
    CONV_YOUR_LANG = "Votre langue. Modifiable dans les options.",
    CONV_LANG_TIP = "Sa langue, reconnue d'après ses derniers messages. Cliquez pour choisir.",
    CONV_LANG_SET_TIP = "Sa langue, choisie par vous. Cliquez pour changer.",
    CONV_LANG_UNKNOWN_TIP = "Langue non reconnue. Cliquez pour choisir.",
    CONV_LANG_NOT_TARGET = "Le traducteur n'écrit pas en %s : votre réponse demande une autre langue.",
    CONV_THEY_WRITE = "Il ou elle écrit en :",
    CONV_AUTOMATIC = "Automatique (%s)",
    CONV_AUTOMATIC_UNKNOWN = "Automatique (non reconnue)",

    CONV_TAB_HINT = "Clic milieu pour fermer. Clic droit pour les options.",
    CONV_TAB_CHANNEL = "Dernier message : %s",
    CONV_TAB_CLOSE = "Fermer",
    CONV_TAB_CLOSE_OTHERS = "Fermer les autres",
    CONV_CLOSE_TIP = "Fermer cette conversation. Ses messages reviennent si ce joueur écrit de nouveau (jusqu'au /reload).",
    CONV_OVERFLOW_TIP = "Autres conversations",
    CONV_NEW_ONE = "1 nouveau message",
    CONV_NEW_MANY = "%d nouveaux messages",
    CONV_BANNER = "Le chat est restreint : les nouveaux messages seront proposés à la fin.",

    CONV_ORIGINAL = "Original",
    CONV_HIDE = "Masquer",
    CONV_TRANSLATE = "Traduire",
    CONV_BUBBLE_HINT_CLICK = "Clic : retraduire (plus de lignes si c'est coupé).",
    CONV_BUBBLE_HINT_SHIFT = "Maj-clic : sans le cache. Clic droit : plus de choix.",
    CONV_BUBBLE_HINT_PLAIN = "Cliquez sur Traduire pour traduire ce message.",
    CONV_HELPER_TIP = "Des blocs de couleur ? Translate Companion n'est pas lancé.",
    CONV_MENU_RETRANSLATE = "Retraduire",
    CONV_MENU_COPY = "Copier l'original",
    CONV_MENU_REMOVE = "Retirer",
    CONV_COPY_HINT = "Appuyez sur Ctrl+C pour copier, puis sur Échap.",

    CONV_YOU_TO = "Vous > %s",
    CONV_YOU_WROTE = "Vous avez écrit : %s",
    CONV_OUT_SENDING = "Envoi...",
    CONV_OUT_SENT = "Envoyé",
    CONV_OUT_UNCONFIRMED = "Non confirmé",
    CONV_OUT_OFFLINE = "Non remis : %s n'est pas en ligne",
    CONV_OUT_BLOCKED = "Bloqué",

    CONV_YOUR_TEXT = "Votre texte :",
    CONV_EDIT = "Modifier",
    CONV_CANCEL_TIP = "Annuler cette réponse",
    CONV_LANE_HINT = "Pas d'état au-dessus ? Lancez Translate Companion.",
    CONV_LANE_IN = "Pas de traduction au-dessus ? Lancez Translate Companion.",
    CONV_LANE_SHORT = "Lancez Translate Companion.",
    CONV_DOWNLOAD = "Télécharger",
    CONV_CLEARED = "conversations effacées.",

    CONV_BINDING_TOGGLE = "Afficher/cacher la fenêtre de conversation",
    CONV_BINDING_REPLY = "Répondre dans la fenêtre de conversation",
}

for key, value in pairs(en) do
    L[key] = value
end
if GetLocale() == "frFR" then
    for key, value in pairs(frFR) do
        L[key] = value
    end
end

BINDING_HEADER_TRANSLATEPROBE = L.CONV_BINDING_HEADER
BINDING_NAME_TRANSLATEPROBE_CONV_TOGGLE = L.CONV_BINDING_TOGGLE
BINDING_NAME_TRANSLATEPROBE_CONV_REPLY = L.CONV_BINDING_REPLY
