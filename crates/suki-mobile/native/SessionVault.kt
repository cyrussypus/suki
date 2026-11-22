package __PACKAGE__

import android.app.Activity
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

object SessionVault {
    private const val PREFS = "suki.session.v1"
    private const val RECORD = "record"
    private const val REGULAR = "suki.session.device.v1"
    private const val LOCKED = "suki.session.biometric.v1"
    private val sequence = AtomicInteger()
    private val results = ConcurrentHashMap<Int, String>()
    private val requests = ConcurrentHashMap<Int, CancellationSignal>()
    private val active = ConcurrentHashMap.newKeySet<Int>()
    private var master: ByteArray? = null

    @JvmStatic fun begin(activity: Activity, operation: String, token: String?): Int {
        val ticket = sequence.incrementAndGet()
        active.add(ticket)
        activity.runOnUiThread {
            if (!active.contains(ticket)) return@runOnUiThread
            try {
                run(activity, operation, token, ticket)
            } catch (_: Exception) {
                finish(ticket, "!Secure storage is unavailable. Sign in with your password or try again.")
            }
        }
        return ticket
    }

    @JvmStatic fun take(ticket: Int): String? = results.remove(ticket)

    @JvmStatic fun cancel(ticket: Int) {
        active.remove(ticket)
        requests.remove(ticket)?.cancel()
        results.remove(ticket)
    }

    private fun finish(ticket: Int, value: String) {
        requests.remove(ticket)
        if (active.remove(ticket)) results[ticket] = value
    }

    private fun key(alias: String): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val builder = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
        if (alias == LOCKED) {
            builder.setUserAuthenticationRequired(true)
            if (Build.VERSION.SDK_INT >= 30) {
                builder.setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
            } else {
                builder.setUserAuthenticationValidityDurationSeconds(-1)
            }
        }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(builder.build())
        }.generateKey()
    }

    private fun cipher(key: SecretKey, encrypt: Boolean, data: ByteArray? = null): Cipher =
        Cipher.getInstance("AES/GCM/NoPadding").apply {
            if (encrypt) init(Cipher.ENCRYPT_MODE, key)
            else init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, data!!.copyOfRange(0, 12)))
        }

    private fun seal(cipher: Cipher, data: ByteArray): ByteArray = cipher.iv + cipher.doFinal(data)
    private fun open(cipher: Cipher, data: ByteArray): ByteArray = cipher.doFinal(data.copyOfRange(12, data.size))
    private fun enc(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)
    private fun dec(value: String): ByteArray = Base64.decode(value, Base64.NO_WRAP)
    private fun secret(): ByteArray = ByteArray(32).apply { SecureRandom().nextBytes(this) }

    private fun confirm(activity: Activity, ticket: Int, c: Cipher, success: (Cipher) -> Unit) {
        val cancel = CancellationSignal()
        requests[ticket] = cancel
        if (!active.contains(ticket)) { requests.remove(ticket); return }
        val builder = BiometricPrompt.Builder(activity)
            .setTitle("Unlock Suki")
            .setSubtitle("Confirm your identity to use the saved session")
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL)
        } else {
            builder.setNegativeButton("Use password", activity.mainExecutor) { _, _ ->
                finish(ticket, "!Unlock cancelled. You can sign in with your password.")
            }
        }
        builder.build().authenticate(BiometricPrompt.CryptoObject(c), cancel, activity.mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    if (!active.contains(ticket)) return
                    try {
                        success(result.cryptoObject?.cipher ?: throw IllegalStateException())
                    } catch (_: Exception) {
                        finish(ticket, "!Could not unlock secure storage. Use your Suki password.")
                    }
                }
                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    finish(ticket, "!Biometric unlock could not complete. You can sign in with your password.")
                }
            })
    }

    private fun run(activity: Activity, operation: String, token: String?, ticket: Int) {
        val prefs = activity.getSharedPreferences(PREFS, Activity.MODE_PRIVATE)
        val record = prefs.getString(RECORD, null)?.split(":")
        val locked = record?.first() == "1"
        val alias = if (locked) LOCKED else REGULAR
        fun save(protect: Boolean, wrapped: ByteArray, raw: ByteArray, value: String) {
            val data = seal(cipher(SecretKeySpec(raw, "AES"), true), value.toByteArray(Charsets.UTF_8))
            if (!prefs.edit().putString(RECORD, "${if (protect) 1 else 0}:${enc(wrapped)}:${enc(data)}").commit())
                throw IllegalStateException()
            if (master !== raw) master?.fill(0)
            master = raw
            finish(ticket, if (protect) "1" else "0")
        }
        when (operation) {
            "enabled" -> finish(ticket, if (locked) "1" else "0")
            "clear" -> {
                if (!prefs.edit().remove(RECORD).commit()) throw IllegalStateException()
                master?.fill(0)
                master = null
                val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
                store.deleteEntry(REGULAR)
                store.deleteEntry(LOCKED)
                finish(ticket, "0")
            }
            "load", "confirm", "disable" -> {
                if (record == null) { finish(ticket, ""); return }
                val wrapped = dec(record[1])
                val reader = cipher(key(alias), false, wrapped)
                val onOpen: (Cipher) -> Unit = { unlocked ->
                    val raw = open(unlocked, wrapped)
                    val data = dec(record[2])
                    val value = open(cipher(SecretKeySpec(raw, "AES"), false, data), data).toString(Charsets.UTF_8)
                    when (operation) {
                        "disable" -> {
                            val next = secret()
                            save(false, seal(cipher(key(REGULAR), true), next), next, value)
                            raw.fill(0)
                        }
                        "confirm" -> { raw.fill(0); finish(ticket, "1") }
                        else -> { master?.fill(0); master = raw; finish(ticket, value) }
                    }
                }
                if (locked) confirm(activity, ticket, reader, onOpen) else onOpen(reader)
            }
            "save", "password_save" -> {
                val value = requireNotNull(token)
                if (record != null && master != null) {
                    save(locked, dec(record[1]), requireNotNull(master), value)
                } else {
                    val raw = secret()
                    val protect = locked && operation != "password_save"
                    val writer = cipher(key(if (protect) LOCKED else REGULAR), true)
                    if (protect) confirm(activity, ticket, writer) { save(true, seal(it, raw), raw, value) }
                    else {
                        save(false, seal(writer, raw), raw, value)
                        if (locked) {
                            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
                            store.deleteEntry(LOCKED)
                        }
                    }
                }
            }
            "enable" -> {
                require(record != null && master != null && !locked)
                val data = dec(record[2])
                val value = open(cipher(SecretKeySpec(requireNotNull(master), "AES"), false, data), data).toString(Charsets.UTF_8)
                val raw = secret()
                confirm(activity, ticket, cipher(key(LOCKED), true)) { save(true, seal(it, raw), raw, value) }
            }
            else -> throw IllegalArgumentException()
        }
    }
}
