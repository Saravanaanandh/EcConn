package com.lowlat.receiver

import android.content.Context
import android.content.SharedPreferences
import android.content.res.Configuration
import android.os.Build
import android.provider.Settings
import android.util.DisplayMetrics
import android.view.Surface
import android.view.WindowManager
import java.util.UUID

/**
 * Provides display properties and persistent device identity for advertising and input mapping.
 */
object DisplayInfo {

    fun getDeviceId(context: Context): String {
        val prefs: SharedPreferences = context.getSharedPreferences("lowlat_prefs", Context.MODE_PRIVATE)
        val existing = prefs.getString("device_id", null)
        if (!existing.isNullOrBlank()) return existing

        val androidId = Settings.Secure.getString(context.contentResolver, Settings.Secure.ANDROID_ID)
        val id = if (!androidId.isNullOrBlank() && androidId != "9774d56d682e549c") {
            "android-$androidId"
        } else {
            "android-" + UUID.randomUUID().toString().substring(0, 8)
        }
        prefs.edit().putString("device_id", id).apply()
        return id
    }

    fun getDeviceName(): String {
        val manufacturer = Build.MANUFACTURER.replaceFirstChar { it.uppercase() }
        val model = Build.MODEL
        return if (model.startsWith(manufacturer, ignoreCase = true)) {
            model
        } else {
            "$manufacturer $model"
        }.replace("|", "-")
    }

    fun getScreenDimensions(context: Context): Pair<Int, Int> {
        val wm = context.getSystemService(Context.WINDOW_SERVICE) as? WindowManager
            ?: return Pair(1920, 1080)

        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val metrics = wm.currentWindowMetrics
            val bounds = metrics.bounds
            Pair(bounds.width(), bounds.height())
        } else {
            val dm = DisplayMetrics()
            @Suppress("DEPRECATION")
            wm.defaultDisplay.getRealMetrics(dm)
            Pair(dm.widthPixels, dm.heightPixels)
        }
    }

    fun getOrientationDegrees(context: Context): Int {
        return if (context.resources.configuration.orientation == Configuration.ORIENTATION_PORTRAIT) {
            90
        } else {
            0
        }
    }
}
