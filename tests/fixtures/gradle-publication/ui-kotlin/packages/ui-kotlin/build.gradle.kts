import org.jetbrains.kotlin.gradle.plugin.KotlinJsCompilerType.IR

plugins {
    kotlin("multiplatform") version "1.9.25"
    id("com.android.library") version "8.5.2"
    id("com.vanniktech.maven.publish") version "0.29.0"
}

kotlin {
    jvm("desktop")
    androidTarget() {
        publishLibraryVariants("release")
    }
    js(IR) {
        browser()
    }
}

android {
    namespace = "io.github.linkfoundation.ui"
    compileSdk = 34
}

mavenPublishing {
    coordinates("foundation.link.ui", "ui-kotlin", "0.1.0")
}
