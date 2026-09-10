import org.gradle.api.artifacts.dsl.LockMode

buildscript {
    configurations.classpath {
        resolutionStrategy.activateDependencyLocking()
    }
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.4.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.20")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
    dependencyLocking {
        lockAllConfigurations()
        lockMode.set(LockMode.STRICT)
        if (project.name == "tauri-android") {
            lockFile.set(rootProject.layout.projectDirectory.file("tauri-android-gradle.lockfile"))
        }
    }
}

tasks.register("clean").configure {
    delete("build")
}
