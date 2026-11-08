import javax.inject.Inject
import org.gradle.process.ExecOperations
import org.gradle.work.DisableCachingByDefault

plugins {
    id("com.android.application")
}

android {
    namespace = "club.suitsu31.kagari.simulator"
    compileSdk = 36
    // The only build-tools the (read-only, Nix-managed) SDK has.
    buildToolsVersion = "37.0.0"
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "club.suitsu31.kagari.simulator"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Until a release keystore exists.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

/** Builds `libkagari_android.so` for every ABI with cargo-ndk into [outputDir]. */
@DisableCachingByDefault(because = "cargo tracks its own incremental state")
abstract class CargoNdkTask : DefaultTask() {
    @get:Input
    abstract val release: Property<Boolean>

    @get:Internal
    abstract val ndkDir: DirectoryProperty

    @get:Internal
    abstract val crateDir: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @get:Inject
    abstract val exec: ExecOperations

    init {
        outputs.upToDateWhen { false }
    }

    @TaskAction
    fun build() {
        val args = mutableListOf(
            "cargo", "ndk", "--platform", "29", "-t", "arm64-v8a", "-t", "x86_64",
            "-o", outputDir.get().asFile.absolutePath, "build", "--lib",
        )
        if (release.get()) {
            args += "--release"
        }
        exec.exec {
            workingDir = crateDir.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get().asFile.absolutePath)
            commandLine(args)
        }
    }
}

androidComponents.onVariants { v ->
    val t = tasks.register<CargoNdkTask>("cargoNdk${v.name.replaceFirstChar(Char::uppercase)}") {
        release.set(v.buildType == "release")
        crateDir.set(rootProject.layout.projectDirectory)
        ndkDir.set(androidComponents.sdkComponents.ndkDirectory)
    }
    v.sources.jniLibs?.addGeneratedSourceDirectory(t, CargoNdkTask::outputDir)
}
