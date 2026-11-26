package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SdlKeysTest {
    @Test
    fun letters_and_digits() {
        assertEquals(SdlKey(4, false), SdlKeys.forChar('a'))
        assertEquals(SdlKey(29, false), SdlKeys.forChar('z'))
        assertEquals(SdlKey(4, true), SdlKeys.forChar('A'))
        assertEquals(SdlKey(30, false), SdlKeys.forChar('1'))
        assertEquals(SdlKey(39, false), SdlKeys.forChar('0'))
    }

    @Test
    fun control_keys() {
        assertEquals(SdlKey(40, false), SdlKeys.forChar('\n'))
        assertEquals(SdlKey(42, false), SdlKeys.forChar('\b'))
        assertEquals(SdlKey(43, false), SdlKeys.forChar('\t'))
        assertEquals(SdlKey(44, false), SdlKeys.forChar(' '))
    }

    @Test
    fun shifted_punctuation() {
        assertEquals(SdlKey(56, false), SdlKeys.forChar('/'))
        assertEquals(SdlKey(56, true), SdlKeys.forChar('?'))
        assertEquals(SdlKey(45, true), SdlKeys.forChar('_'))
        assertEquals(SdlKey(52, true), SdlKeys.forChar('"'))
    }

    @Test
    fun unsupported_characters_are_ignored() {
        assertNull(SdlKeys.forChar('\u20AC'))
        assertNull(SdlKeys.forChar('\u0001'))
    }
}
