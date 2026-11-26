package com.lowlat.protocol

/** One key press as the server expects it: SDL scancode + optional Shift. */
data class SdlKey(val scancode: Int, val shift: Boolean)

/**
 * Minimal character -> SDL scancode map (USB HID usage IDs, which is what SDL
 * scancodes are). Covers the keys an on-screen keyboard can realistically
 * send: letters, digits, punctuation, space, Enter, Tab, Backspace, Esc.
 */
object SdlKeys {
    const val LEFT_SHIFT = 225
    const val ENTER = 40
    const val ESC = 41
    const val BACKSPACE = 42
    const val TAB = 43
    const val SPACE = 44

    private val punctuation: Map<Char, SdlKey> = mapOf(
        '-' to SdlKey(45, false), '_' to SdlKey(45, true),
        '=' to SdlKey(46, false), '+' to SdlKey(46, true),
        '[' to SdlKey(47, false), '{' to SdlKey(47, true),
        ']' to SdlKey(48, false), '}' to SdlKey(48, true),
        '\\' to SdlKey(49, false), '|' to SdlKey(49, true),
        ';' to SdlKey(51, false), ':' to SdlKey(51, true),
        '\'' to SdlKey(52, false), '"' to SdlKey(52, true),
        '`' to SdlKey(53, false), '~' to SdlKey(53, true),
        ',' to SdlKey(54, false), '<' to SdlKey(54, true),
        '.' to SdlKey(55, false), '>' to SdlKey(55, true),
        '/' to SdlKey(56, false), '?' to SdlKey(56, true),
    )

    fun forChar(c: Char): SdlKey? = when {
        c in 'a'..'z' -> SdlKey(4 + (c - 'a'), false)
        c in 'A'..'Z' -> SdlKey(4 + (c - 'A'), true)
        c in '1'..'9' -> SdlKey(30 + (c - '1'), false)
        c == '0' -> SdlKey(39, false)
        c == '\n' || c == '\r' -> SdlKey(ENTER, false)
        c == '\b' -> SdlKey(BACKSPACE, false)
        c == '\t' -> SdlKey(TAB, false)
        c == ' ' -> SdlKey(SPACE, false)
        else -> punctuation[c]
    }
}
