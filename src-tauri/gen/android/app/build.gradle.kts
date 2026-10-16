import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// The edition: UNIRAY_STORE=play builds the one for Google Play, without
// stealth mode (Play allows a device-wide tunnel only through VpnService) and
// under its own application id, so that it installs next to the full
// edition from GitHub. The Rust side reads the same variable (src-tauri/build.rs).
val playStore = System.getenv("UNIRAY_STORE") == "play"
val edition = if (playStore) "play" else "full"

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 37
    namespace = "dev.uniray.app"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = if (playStore) "dev.uniray.app.play" else "dev.uniray.app"
        buildConfigField("boolean", "PLAY_STORE", playStore.toString())
        minSdk = 24
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    // geoip.dat and geosite.dat, shared with the desktop build (scripts/fetch-xray.sh).
    sourceSets["main"].assets.srcDir("../../../resources")
    // The edition's code (the Stealth object; the full edition's stealth
    // service), resources (launcher shortcuts naming its application id) and
    // manifest additions. The manifest goes in as the build types' own, which
    // nothing else uses: a source set has a single manifest.
    sourceSets["main"].java.srcDir("src/$edition/java")
    sourceSets["main"].res.srcDir("src/$edition/res")
    sourceSets["debug"].manifest.srcFile("src/$edition/AndroidManifest.xml")
    sourceSets["release"].manifest.srcFile("src/$edition/AndroidManifest.xml")
    // uniray-engine ships as libuniray_engine.so (scripts/build-engine.sh) and
    // is run as a program, which Android allows only from the extracted
    // native library directory: extract native libraries at install.
    packaging {
        jniLibs.useLegacyPackaging = true
        // The standalone Xray of stealth mode (scripts/build-xray-android.sh),
        // run as root: not in the Google Play edition.
        if (playStore) jniLibs.excludes += listOf("**/libxray.so", "**/libxray_tun.so")
    }
    // Until a release keystore is configured (keystore.properties), release
    // builds are signed with the debug key so that they can be installed.
    val keystoreFile = rootProject.file("keystore.properties")
    if (keystoreFile.exists()) {
        val keystore = Properties().apply { keystoreFile.inputStream().use { load(it) } }
        signingConfigs {
            create("release") {
                keyAlias = keystore.getProperty("keyAlias")
                keyPassword = keystore.getProperty("password")
                storeFile = file(keystore.getProperty("storeFile"))
                storePassword = keystore.getProperty("password")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName(if (keystoreFile.exists()) "release" else "debug")
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    // The Xray core (UniRay's libXray fork), see scripts/build-libxray-android.sh.
    implementation(files("libs/libXray.aar"))
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    // Stealth mode (needs root), the full edition only: libsu runs a
    // long-lived root shell; Shizuku is the alternative privilege provider
    // (root when started as root, else an ADB shell that cannot do stealth).
    if (!playStore) {
        implementation("com.github.topjohnwu.libsu:core:6.0.0")
        implementation("dev.rikka.shizuku:api:13.1.5")
        implementation("dev.rikka.shizuku:provider:13.1.5")
    }
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
