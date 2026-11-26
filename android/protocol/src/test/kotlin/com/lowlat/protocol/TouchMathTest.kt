package com.lowlat.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class TouchMathTest {
    @Test
    fun fit_letterboxes_and_maps_the_centre() {
        val rect = TouchMath.rect(1000f, 1000f, 1920, 1080, FitMode.Fit)
        assertEquals(0f, rect.left, 0.001f)
        assertEquals(218.75f, rect.top, 0.001f)
        assertEquals(1000f, rect.width, 0.001f)
        assertEquals(562.5f, rect.height, 0.001f)

        assertEquals(32768 to 32768, TouchMath.normalize(500f, 500f, rect))
        assertEquals(0 to 0, TouchMath.normalize(0f, 218.75f, rect))
        assertEquals(65535 to 65535, TouchMath.normalize(1000f, 781.25f, rect))
        // A touch in the letterbox is clamped to the nearest video pixel.
        assertEquals(32768 to 0, TouchMath.normalize(500f, 0f, rect))
    }

    @Test
    fun fill_crops_the_sides_but_keeps_centre_mapping() {
        val rect = TouchMath.rect(1000f, 1000f, 1920, 1080, FitMode.Fill)
        assertTrue(rect.left < 0f)
        assertEquals(1000f, rect.height)
        assertEquals(32768 to 32768, TouchMath.normalize(500f, 500f, rect))
        assertEquals(0 to 0, TouchMath.normalize(-389f, 0f, rect))
    }

    @Test
    fun zero_sizes_do_not_divide_by_zero() {
        val rect = TouchMath.rect(0f, 0f, 1920, 1080, FitMode.Fit)
        assertEquals(0 to 0, TouchMath.normalize(10f, 10f, rect))
    }
}
