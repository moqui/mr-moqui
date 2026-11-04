import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// Firebase Cloud Messaging (native push, plans/Mobile-Desktop.md MD6 MT2) only when the Firebase project's
// google-services.json is here: its plugin, its library and src/fcm (PushService) are then in the build. Without it
// the app builds and runs with push off (Push.kt asks for FcmPush by name), and no Firebase class is in it.
val fcm = file("google-services.json").exists()
if (fcm) {
    apply(plugin = "com.google.gms.google-services")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 37
    namespace = "org.moqui.mobile"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "org.moqui.mobile"
        minSdk = 26
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
        buildConfigField("boolean", "FCM", fcm.toString())
    }
    if (fcm) {
        sourceSets.getByName("main").java.srcDir("src/fcm/java")
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

// src/fcm's manifest (PushService) merged only with Firebase.
androidComponents {
    onVariants { variant ->
        if (fcm) variant.sources.manifests.addStaticManifestFile("src/fcm/AndroidManifest.xml")
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
    // rustls-platform-verifier's Kotlin component, at the version of the Rust crate in Cargo.lock (below).
    implementation("org.rustls:rustls-platform-verifier")
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    // the ring's CallStyle notification and the full-screen intent check (CallService.kt)
    implementation("androidx.core:core:1.13.1")
    if (fcm) {
        implementation(platform("com.google.firebase:firebase-bom:33.7.0"))
        implementation("com.google.firebase:firebase-messaging")
    }
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

// rustls-platform-verifier's Android component (crates/moqui-client/README.md): its Maven archive on GitHub, at the
// version of rustls-platform-verifier-android in crates/moqui-mobile/Cargo.lock, as its README sets it up. The archive
// is pinned to a commit of its maven-archive branch (which has 0.1.0 to 0.2.0), not the branch itself, and serves only
// the org.rustls group; a newer rustls-platform-verifier-android needs the commit that added it here.
repositories {
    maven {
        url = uri(
            "https://github.com/rustls/rustls-platform-verifier/raw/" +
                "1aa691352a5e0c215210dfc9a3c2f2078639dc91/android-release-support/maven/"
        )
        content {
            includeGroup("org.rustls")
        }
    }
}

abstract class RustlsVersion : ValueSource<String, RustlsVersion.Params> {
    interface Params : ValueSourceParameters {
        val lockFile: RegularFileProperty
    }

    companion object {
        const val CRATE_NAME = "rustls-platform-verifier-android"
    }

    override fun obtain(): String {
        val version = parameters.lockFile.get().asFile.readLines().let { lines ->
            val nameIdx = lines.indexOfFirst { it.trim() == "name = \"$CRATE_NAME\"" }
            if (nameIdx < 0) {
                null
            } else {
                lines.drop(nameIdx + 1)
                    .firstOrNull { it.trimStart().startsWith("version = ") }
                    ?.substringAfter('"', "")
                    ?.substringBefore('"', "")
                    ?.takeIf { it.isNotEmpty() }
            }
        }
        return version ?: error("$CRATE_NAME not found in Cargo.lock")
    }
}

val rustlsPlatformVerifierVersion = providers.of(RustlsVersion::class.java) {
    parameters.lockFile.set(layout.projectDirectory.file("../../../Cargo.lock"))
}

configurations.configureEach {
    resolutionStrategy.eachDependency {
        if (requested.group == "org.rustls" && requested.name == "rustls-platform-verifier") {
            useVersion(rustlsPlatformVerifierVersion.get())
            because("the Kotlin component must be the version of ${RustlsVersion.CRATE_NAME}")
        }
    }
}

apply(from = file("tauri.build.gradle.kts"))
