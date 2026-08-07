import org.gradle.internal.os.OperatingSystem

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

/**
 * The ABIs we ship. `x86_64` is here for the emulator — dropping it makes the
 * app untestable without a physical device.
 */
val abis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")

/** Where cargo-ndk drops `libkessel_ffi.so`, one directory per ABI. */
val rustJniLibs = layout.buildDirectory.dir("rustJniLibs")

/**
 * Build the Rust console for every ABI.
 *
 * Always `--release`, even for a debug APK. A debug build of `kessel-vm` is
 * roughly an order of magnitude slower, which for a machine that must finish a
 * frame inside 16 ms is the difference between a game and a slideshow — and
 * nobody debugs the VM through this app anyway; `cargo test` is right there.
 */
val cargoBuild by tasks.registering(Exec::class) {
    group = "build"
    description = "Cross-compile kessel-ffi for Android via cargo-ndk"

    workingDir = rootProject.file("../crates")
    // Re-run when the Rust changes, and not otherwise: this is the slow task in
    // the build, so an accurate input set is worth stating explicitly.
    inputs.dir(rootProject.file("../crates/vm/src"))
    inputs.dir(rootProject.file("../crates/ffi/src"))
    inputs.file(rootProject.file("../crates/Cargo.toml"))
    inputs.file(rootProject.file("../crates/ffi/Cargo.toml"))
    outputs.dir(rustJniLibs)

    // cargo-ndk locates the toolchain through the NDK; point it at the one the
    // Android plugin already resolved rather than hoping the shell has it set.
    environment("ANDROID_NDK_HOME", android.sdkDirectory.resolve("ndk/${android.ndkVersion}"))

    val cargo = if (OperatingSystem.current().isWindows) "cargo.exe" else "cargo"
    commandLine(
        buildList {
            add(cargo)
            add("ndk")
            abis.forEach { add("-t"); add(it) }
            add("-o"); add(rustJniLibs.get().asFile.absolutePath)
            add("build"); add("--release"); add("-p"); add("kessel-ffi")
        }
    )

    doFirst {
        val ndk = android.sdkDirectory.resolve("ndk/${android.ndkVersion}")
        check(ndk.isDirectory) {
            "NDK ${android.ndkVersion} not found at $ndk — install it via the SDK Manager " +
                "(Tools > SDK Manager > SDK Tools > NDK)."
        }
    }
}

android {
    namespace = "dev.kessel"
    compileSdk = 35
    ndkVersion = "27.0.12077973"

    defaultConfig {
        applicationId = "dev.kessel"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += abis }
    }

    sourceSets["main"].apply {
        jniLibs.srcDir(rustJniLibs)
        // The games ship straight out of the repo's games/ directory. Copying
        // them in would fork the corpus that crates/vm/tests/games_compile.rs
        // guards, and the copy would rot.
        assets.srcDir(rootProject.file("../games"))
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }
}

tasks.named("preBuild") { dependsOn(cargoBuild) }

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.ui)
    implementation(libs.androidx.ui.graphics)
    implementation(libs.androidx.ui.tooling.preview)
    implementation(libs.androidx.material3)
    debugImplementation(libs.androidx.ui.tooling)

    testImplementation(libs.junit)
    // android.jar's org.json is a stub that throws on every call, so a unit
    // test of Controls.parse would test nothing without a real implementation
    // on the test classpath.
    testImplementation(libs.json)
    androidTestImplementation(libs.androidx.junit)
}
