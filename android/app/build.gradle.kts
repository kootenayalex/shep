import java.io.ByteArrayOutputStream
import java.util.Properties

/**
 * `git describe`, so a build on a phone can be traced back to a commit.
 *
 * versionName was the literal string "0.1.0" and had been since the repo
 * started, which made every sideload indistinguishable from every other one —
 * the exact problem the app's own protocol-mismatch banner exists to catch.
 * Falls back to the hardcoded name outside a checkout (a source zip, CI).
 */
fun gitVersionName(fallback: String): String = try {
    val out = ByteArrayOutputStream()
    exec {
        commandLine("git", "describe", "--tags", "--always", "--dirty")
        standardOutput = out
        errorOutput = ByteArrayOutputStream()
        isIgnoreExitValue = true
        workingDir = rootDir
    }
    out.toString().trim().ifEmpty { fallback }
} catch (_: Exception) {
    fallback
}

/**
 * Release signing comes from the environment or `local.properties`, never from
 * the debug key. `SHEP_ANDROID_KEYSTORE`, `SHEP_ANDROID_KEYSTORE_PASSWORD`,
 * `SHEP_ANDROID_KEY_ALIAS`, `SHEP_ANDROID_KEY_PASSWORD` — or the same four as
 * `shep.keystore`, `shep.keystore_password`, `shep.key_alias`,
 * `shep.key_password` in local.properties. A release build without them fails
 * at packaging rather than quietly shipping a debug-signed APK.
 */
val localProps = Properties().apply {
    val file = rootProject.file("local.properties")
    if (file.exists()) file.inputStream().use { load(it) }
}
fun releaseSetting(name: String): String? =
    System.getenv("SHEP_ANDROID_${name.uppercase()}") ?: localProps.getProperty("shep.$name")
val releaseKeystore: String? = releaseSetting("keystore")

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    // FCM. The first Google dependency in a repo that was otherwise org.json +
    // OkHttp on purpose; taken because nothing else wakes an Android app out of
    // Doze reliably, which is the whole job of a notification.
    id("com.google.gms.google-services")
}

android {
    namespace = "dev.shep.companion"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.shep.companion"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = gitVersionName("0.1.0")
    }

    signingConfigs {
        create("release") {
            if (releaseKeystore != null) {
                storeFile = file(releaseKeystore)
                storePassword = releaseSetting("keystore_password")
                keyAlias = releaseSetting("key_alias")
                keyPassword = releaseSetting("key_password")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("release")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.10.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    // The platform's own launch animation on API 31+, rather than an extra
    // activity — so a shep-coloured cold start costs no time to first frame.
    implementation("androidx.core:core-splashscreen:1.0.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // The bridge token at rest, keyed by the Android keystore (Pairing.kt).
    implementation("androidx.security:security-crypto:1.1.0-alpha06")
    // QR pairing scanner — no Google Play Services, license-clean (Apache-2.0).
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    // Push. FCM is the one transport that actually wakes the app from Doze.
    implementation(platform("com.google.firebase:firebase-bom:33.7.0"))
    implementation("com.google.firebase:firebase-messaging")
    // JVM unit tests for the pure logic (wire decoding, grid state) that has no
    // Android dependencies and is easy to get subtly wrong.
    testImplementation("junit:junit:4.13.2")
    // The android.jar on the unit-test classpath stubs org.json — every method
    // throws "not mocked". This is Android's own JSON implementation
    // repackaged under Apache-2.0 (the upstream org.json artifact carries the
    // "Good, not Evil" clause), so wire parsing can be tested on the JVM.
    testImplementation("com.vaadin.external.google:android-json:0.0.20131108.vaadin1")
}

// A release build with no real keystore stops here, loudly.
tasks.configureEach {
    if (name == "validateSigningRelease" || name == "packageRelease") {
        doFirst {
            if (releaseKeystore == null) {
                throw GradleException(
                    "release builds need a real keystore: set SHEP_ANDROID_KEYSTORE, " +
                        "SHEP_ANDROID_KEYSTORE_PASSWORD, SHEP_ANDROID_KEY_ALIAS and " +
                        "SHEP_ANDROID_KEY_PASSWORD (or shep.keystore etc. in local.properties)",
                )
            }
        }
    }
}
