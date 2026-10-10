plugins {
    id("com.android.application")
    kotlin("android")
}

android {
    namespace = "com.faeknet.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.faeknet.android"
        minSdk = 28
        targetSdk = 35
        versionCode = 2
        versionName = "0.56.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}
