import org.gradle.api.artifacts.dsl.LockMode

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.lorepia.tauri.platform"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

allprojects {
    dependencyLocking {
        lockAllConfigurations()
        lockMode.set(LockMode.STRICT)
        if (project.name == "tauri-android") {
            lockFile.set(rootProject.layout.projectDirectory.file("tauri-android-gradle.lockfile"))
        }
    }
}

dependencies {
    implementation(project(":tauri-android"))
    implementation("androidx.activity:activity:1.10.1")
    compileOnly("androidx.appcompat:appcompat:1.6.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.6.2")
}
