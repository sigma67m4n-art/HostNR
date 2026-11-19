plugins {
    id("com.android.application")
}

android {
    namespace = "app.hostnr"
    compileSdk = 35
    ndkVersion = "28.0.13004108"

    defaultConfig {
        applicationId = "app.hostnr"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
        testInstrumentationRunner = "android.test.InstrumentationTestRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }

    lint {
        abortOnError = true
        checkReleaseBuilds = true
    }
}

tasks.matching { it.name == "preBuild" }.configureEach {
    doFirst {
        listOf("arm64-v8a", "armeabi-v7a", "x86_64").forEach { abi ->
            check(file("src/main/jniLibs/$abi/libhostnr.so").exists()) {
                "Missing Rust library for $abi. Build Rust in GitHub Actions first."
            }
        }
    }
}
