plugins {
    id("com.android.application") version "9.5.1" apply false
    // Must match the Kotlin that AGP's built-in Kotlin support uses (2.2.10 for AGP 9.5.1).
    id("org.jetbrains.kotlin.plugin.compose") version "2.2.10" apply false
}
