// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import androidx.annotation.RequiresApi
import java.security.KeyStore
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The app's device keys sealed by an AES-GCM key of the Android Keystore, which never leaves it
 * (crates/moqui-mobile/src/keystore.rs, called from Rust through JNI, src/android.rs). The sealed bytes are the IV's
 * length, the IV and the ciphertext with its tag; Rust keeps them in the app's private files.
 *
 * The device keys' key is not tied to the lock screen: the outbox, an incoming call and a push use the keys while the
 * phone is locked. A second key, for what no background work reads (the chat store passphrases), opens only while the
 * phone is unlocked, from Android 9 (`setUnlockedDeviceRequired`; plans/Security-Review.md SEC-CLI-113): its methods
 * are the `...Unlocked` ones, and before Android 9 `readyUnlocked` refuses, so Rust seals those by the first key.
 *
 * A third key, the vault's device memory key (`...Auth`, plans/Vault-Instances.md V5), opens only for a short while after
 * the user confirmed with the lock screen or a strong biometric ([confirmUser]).
 */
object KeyVault {
    private const val ALIAS = "org.moqui.mobile.device-keys"
    private const val UNLOCKED_ALIAS = "org.moqui.mobile.unlocked-keys"
    private const val TAG_BITS = 128
    private const val AUTH_ALIAS = "org.moqui.mobile.vault-memory"
    /** How long after the user confirms the device memory key opens. */
    private const val AUTH_SECONDS = 30

    private fun key(alias: String = ALIAS): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
        if (alias == UNLOCKED_ALIAS && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            spec.setUnlockedDeviceRequired(true)
        }
        generator.init(spec.build())
        return generator.generateKey()
    }

    /** Make or find the Keystore key, so a device that cannot is known before a key is kept. */
    @JvmStatic
    fun ready() {
        key()
    }

    /** Make or find the key of an unlocked phone; refused before Android 9, which cannot tie a key to the lock. */
    @JvmStatic
    fun readyUnlocked() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            throw UnsupportedOperationException("no key for an unlocked phone before Android 9")
        }
        key(UNLOCKED_ALIAS)
    }

    @JvmStatic
    fun seal(plain: ByteArray): ByteArray = sealBy(ALIAS, plain)

    @JvmStatic
    fun open(sealed: ByteArray): ByteArray = openBy(ALIAS, sealed)

    /**
     * `plain` sealed by the key of an unlocked phone; refused while the phone is locked. A key the Keystore invalidated
     * is made again (what it sealed no longer opens: a passphrase is the server's, and comes back from it).
     */
    @JvmStatic
    fun sealUnlocked(plain: ByteArray): ByteArray = try {
        sealBy(UNLOCKED_ALIAS, plain)
    } catch (err: KeyPermanentlyInvalidatedException) {
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(UNLOCKED_ALIAS)
        sealBy(UNLOCKED_ALIAS, plain)
    }

    /** `sealed` opened by the key of an unlocked phone; refused while the phone is locked. */
    @JvmStatic
    fun openUnlocked(sealed: ByteArray): ByteArray = openBy(UNLOCKED_ALIAS, sealed)

    /**
     * The vault's device memory key (plans/Vault-Instances.md §3.5, V5): it seals and opens only for [AUTH_SECONDS]
     * after the user confirmed with the lock screen or a strong biometric ([confirmUser]), only while the phone is
     * unlocked, and a new biometric enrolled invalidates it. Android 9 and later; a phone with no secure lock screen
     * cannot make it, and the app then offers no device memory.
     */
    @RequiresApi(Build.VERSION_CODES.P)
    private fun authKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(AUTH_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(AUTH_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .setUnlockedDeviceRequired(true)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            spec.setUserAuthenticationParameters(
                AUTH_SECONDS,
                KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL,
            )
        } else {
            @Suppress("DEPRECATION")
            spec.setUserAuthenticationValidityDurationSeconds(AUTH_SECONDS)
        }
        generator.init(spec.build())
        return generator.generateKey()
    }

    private fun dropAuthKey() {
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(AUTH_ALIAS)
    }

    /** Make or find the device memory key; refused before Android 9 and on a phone with no secure lock screen. */
    @JvmStatic
    fun readyAuth() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            throw UnsupportedOperationException("the vault's device memory needs Android 9")
        }
        authKey()
    }

    /**
     * `plain` (a device memory key) sealed by the device memory key. The user is asked to confirm first when they have
     * not lately; refused when they do not. A key the Keystore invalidated is made again.
     */
    @RequiresApi(Build.VERSION_CODES.P)
    @JvmStatic
    fun sealAuth(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        try {
            cipher.init(Cipher.ENCRYPT_MODE, authKey())
        } catch (err: KeyPermanentlyInvalidatedException) {
            dropAuthKey()
            return sealAuth(plain)
        } catch (err: UserNotAuthenticatedException) {
            if (!confirmUser()) throw SecurityException("the user did not confirm with the lock screen")
            cipher.init(Cipher.ENCRYPT_MODE, authKey())
        }
        val iv = cipher.iv
        return byteArrayOf(iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    /**
     * `sealed` opened by the device memory key: null while the user has not confirmed lately (Rust then asks,
     * [confirmUser], and tries again), and an empty array when the Keystore invalidated the key, whose memories no
     * longer open.
     */
    @RequiresApi(Build.VERSION_CODES.P)
    @JvmStatic
    fun openAuth(sealed: ByteArray): ByteArray? {
        require(sealed.isNotEmpty()) { "nothing sealed" }
        val ivSize = sealed[0].toInt()
        require(ivSize in 12..16 && sealed.size > 1 + ivSize) { "not sealed by this vault" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        try {
            cipher.init(Cipher.DECRYPT_MODE, authKey(), GCMParameterSpec(TAG_BITS, sealed, 1, ivSize))
        } catch (err: KeyPermanentlyInvalidatedException) {
            dropAuthKey()
            return ByteArray(0)
        } catch (err: UserNotAuthenticatedException) {
            return null
        }
        return cipher.doFinal(sealed, 1 + ivSize, sealed.size - 1 - ivSize)
    }

    /**
     * Ask the user for the lock screen or a strong biometric (BiometricPrompt over the app's activity) and wait for
     * the answer, up to two minutes; whether they confirmed. Called from Rust's threads, never the main one, and only
     * while the app's activity is there.
     */
    @JvmStatic
    fun confirmUser(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return false
        if (Looper.myLooper() == Looper.getMainLooper()) return false
        val activity = CallManager.appActivity() ?: return false
        val answered = CountDownLatch(1)
        val confirmed = AtomicBoolean(false)
        val cancel = CancellationSignal()
        activity.runOnUiThread {
            try {
                val builder = BiometricPrompt.Builder(activity)
                    .setTitle(activity.getString(R.string.vault_unlock_title))
                    .setDescription(activity.getString(R.string.vault_unlock_reason))
                when {
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.R -> builder.setAllowedAuthenticators(
                        BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                    )
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q -> {
                        @Suppress("DEPRECATION")
                        builder.setDeviceCredentialAllowed(true)
                    }
                    else -> builder.setNegativeButton(activity.getString(android.R.string.cancel), activity.mainExecutor) { _, _ ->
                        answered.countDown()
                    }
                }
                builder.build().authenticate(cancel, activity.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                        confirmed.set(true)
                        answered.countDown()
                    }

                    override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                        answered.countDown()
                    }
                })
            } catch (err: RuntimeException) {
                answered.countDown()
            }
        }
        if (!answered.await(120, TimeUnit.SECONDS)) cancel.cancel()
        return confirmed.get()
    }

    private fun sealBy(alias: String, plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key(alias))
        val iv = cipher.iv
        return byteArrayOf(iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    private fun openBy(alias: String, sealed: ByteArray): ByteArray {
        require(sealed.isNotEmpty()) { "nothing sealed" }
        val ivSize = sealed[0].toInt()
        require(ivSize in 12..16 && sealed.size > 1 + ivSize) { "not sealed by this vault" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(alias), GCMParameterSpec(TAG_BITS, sealed, 1, ivSize))
        return cipher.doFinal(sealed, 1 + ivSize, sealed.size - 1 - ivSize)
    }
}
