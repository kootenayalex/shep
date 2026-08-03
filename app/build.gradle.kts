plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.shep.companion"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.shep.companion"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Personal sideload build; signed with the debug key on purpose.
            signingConfig = signingConfigs.getByName("debug")
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
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // QR pairing scanner — no Google Play Services, license-clean (Apache-2.0).
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    // A3 push: UnifiedPush connector — no Google Play Services, distributor-agnostic
    // (ntfy app is the distributor). Apache-2.0.
    implementation("org.unifiedpush.android:connector:2.5.0")
    // JVM unit tests for the pure logic (wire decoding, grid state) that has no
    // Android dependencies and is easy to get subtly wrong.
    testImplementation("junit:junit:4.13.2")
}
