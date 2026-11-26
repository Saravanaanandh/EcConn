plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.lowlat.receiver"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.lowlat.receiver"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.0")
    testImplementation(kotlin("test-junit"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")

    // Protocol module: wire formats, assembler, pairing, store, input codec
    implementation(project(":protocol"))
}