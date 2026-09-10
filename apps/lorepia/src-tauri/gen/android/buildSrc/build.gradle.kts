import org.gradle.api.artifacts.dsl.LockMode

plugins {
    `kotlin-dsl`
}

gradlePlugin {
    plugins {
        create("pluginsForCoolKids") {
            id = "rust"
            implementationClass = "RustPlugin"
        }
    }
}

repositories {
    google()
    mavenCentral()
}

dependencyLocking {
    lockAllConfigurations()
    lockMode.set(LockMode.STRICT)
}

dependencies {
    compileOnly(gradleApi())
    implementation("com.android.tools.build:gradle:9.3.2")
    // buildSrc otherwise exposes AGP's older Kotlin plugin to the whole app build.
    implementation("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.20")
}
