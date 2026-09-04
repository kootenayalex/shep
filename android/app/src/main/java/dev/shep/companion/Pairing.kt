package dev.shep.companion

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/** The one secret the app holds: where the bridge is and what it accepts. */
data class Pairing(val url: String, val token: String)

/**
 * Where the pairing lives.
 *
 * The bridge token used to sit in the plain `shep` preferences file, which
 * `adb backup`, a rooted phone or a device-to-device transfer could read as
 * text. It now lives in an [EncryptedSharedPreferences] file keyed by the
 * Android keystore, and a pairing saved by an older build is moved across the
 * first time anything asks for it. Everything that dials the bridge — the
 * activity, push actions, the widget, the FCM registration — reads through
 * here, so there is one answer to "are we paired".
 *
 * A device whose keystore cannot back the master key (it happens on some
 * emulators and after a factory reset with a stale key) falls back to the
 * plain file rather than making the app unusable; the migration is skipped in
 * that case because there is nowhere safer to move to.
 */
object PairingStore {
    private const val PLAIN = "shep"
    private const val SECURE = "shep.secure"
    const val KEY_URL = "url"
    const val KEY_TOKEN = "token"

    @Volatile private var secureCache: SharedPreferences? = null

    private fun plain(context: Context): SharedPreferences =
        context.getSharedPreferences(PLAIN, Context.MODE_PRIVATE)

    private fun secure(context: Context): SharedPreferences {
        secureCache?.let { return it }
        val store = runCatching {
            val key = MasterKey.Builder(context.applicationContext)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
            EncryptedSharedPreferences.create(
                context.applicationContext,
                SECURE,
                key,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            )
        }.getOrElse { plain(context) }
        secureCache = store
        return store
    }

    fun load(context: Context): Pairing? {
        val store = secure(context)
        migrate(plain(context), store)
        return read(store)
    }

    fun isPaired(context: Context): Boolean = load(context) != null

    fun save(context: Context, url: String, token: String) {
        secure(context).edit().putString(KEY_URL, url).putString(KEY_TOKEN, token).apply()
        val plain = plain(context)
        if (plain !== secureCache) plain.edit().remove(KEY_URL).remove(KEY_TOKEN).apply()
    }

    fun clear(context: Context) {
        secure(context).edit().remove(KEY_URL).remove(KEY_TOKEN).apply()
        plain(context).edit().remove(KEY_URL).remove(KEY_TOKEN).apply()
    }

    /**
     * Move a pairing an older build left in [plain] into [secure], once.
     *
     * Returns whether anything was moved. A [secure] that *is* the plain file
     * (the keystore fallback) is left alone: moving would delete the only copy.
     */
    fun migrate(plain: SharedPreferences, secure: SharedPreferences): Boolean {
        if (plain === secure) return false
        val token = plain.getString(KEY_TOKEN, null) ?: return false
        val url = plain.getString(KEY_URL, null)
        if (secure.getString(KEY_TOKEN, null) == null) {
            secure.edit().apply {
                putString(KEY_TOKEN, token)
                if (url != null) putString(KEY_URL, url)
            }.apply()
        }
        plain.edit().remove(KEY_TOKEN).remove(KEY_URL).apply()
        return true
    }

    fun read(store: SharedPreferences): Pairing? {
        val url = store.getString(KEY_URL, null) ?: return null
        val token = store.getString(KEY_TOKEN, null) ?: return null
        return Pairing(url, token)
    }
}
