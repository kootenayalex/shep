package dev.shep.companion

import android.content.SharedPreferences
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The token moves out of the plain preferences file exactly once, and never
 * gets deleted when there is nowhere safer for it to go.
 */
class PairingMigrationTest {

    /** Enough of SharedPreferences for strings. */
    private class FakePrefs : SharedPreferences {
        val map = mutableMapOf<String, String>()
        override fun getString(key: String, defValue: String?): String? = map[key] ?: defValue
        override fun contains(key: String): Boolean = key in map
        override fun edit(): SharedPreferences.Editor = object : SharedPreferences.Editor {
            private val puts = mutableMapOf<String, String?>()
            private val removes = mutableSetOf<String>()
            override fun putString(key: String, value: String?) = apply { puts[key] = value }
            override fun remove(key: String) = apply { removes += key }
            override fun apply() { commit() }
            override fun commit(): Boolean {
                removes.forEach { map.remove(it) }
                puts.forEach { (k, v) -> if (v == null) map.remove(k) else map[k] = v }
                return true
            }
            override fun putStringSet(key: String, values: MutableSet<String>?) = throw UnsupportedOperationException()
            override fun putInt(key: String, value: Int) = throw UnsupportedOperationException()
            override fun putLong(key: String, value: Long) = throw UnsupportedOperationException()
            override fun putFloat(key: String, value: Float) = throw UnsupportedOperationException()
            override fun putBoolean(key: String, value: Boolean) = throw UnsupportedOperationException()
            override fun clear() = apply { map.clear() }
        }
        override fun getAll(): MutableMap<String, *> = map
        override fun getStringSet(key: String, defValues: MutableSet<String>?) = defValues
        override fun getInt(key: String, defValue: Int) = defValue
        override fun getLong(key: String, defValue: Long) = defValue
        override fun getFloat(key: String, defValue: Float) = defValue
        override fun getBoolean(key: String, defValue: Boolean) = defValue
        override fun registerOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) {}
        override fun unregisterOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) {}
    }

    @Test
    fun `an old pairing moves across and leaves nothing behind`() {
        val plain = FakePrefs().apply { map["url"] = "ws://10.0.0.27:7431/"; map["token"] = "secret" }
        val secure = FakePrefs()
        assertTrue(PairingStore.migrate(plain, secure))
        assertEquals(Pairing("ws://10.0.0.27:7431/", "secret"), PairingStore.read(secure))
        assertNull(plain.map["token"])
        assertNull(plain.map["url"])
        assertFalse("nothing left to move", PairingStore.migrate(plain, secure))
    }

    @Test
    fun `a pairing already in the secure store wins over a stale plain one`() {
        val plain = FakePrefs().apply { map["url"] = "ws://old/"; map["token"] = "old" }
        val secure = FakePrefs().apply { map["url"] = "ws://new/"; map["token"] = "new" }
        assertTrue(PairingStore.migrate(plain, secure))
        assertEquals(Pairing("ws://new/", "new"), PairingStore.read(secure))
        assertTrue(plain.map.isEmpty())
    }

    /** Keystore fallback: the "secure" store is the plain file. Deleting would lose the pairing. */
    @Test
    fun `migration into the same file is a no-op`() {
        val plain = FakePrefs().apply { map["url"] = "ws://x/"; map["token"] = "t" }
        assertFalse(PairingStore.migrate(plain, plain))
        assertEquals(Pairing("ws://x/", "t"), PairingStore.read(plain))
    }

    @Test
    fun `half a pairing is no pairing`() {
        val secure = FakePrefs().apply { map["token"] = "t" }
        assertNull(PairingStore.read(secure))
    }
}
