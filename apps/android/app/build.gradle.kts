import org.gradle.process.ExecOperations
import java.io.ByteArrayOutputStream
import javax.inject.Inject
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// Clone cache of the tree-sitter sources (see FetchGrammarSources below).
val grammarSourcesDir = layout.projectDirectory.dir("../.grammar-sources")

// The Cargo workspace (Rust core) is at the root of the repository: apps/android/app -> ../../..
val repoRootDir = layout.projectDirectory.dir("../../..")

// ABIs shipped in the APK, for the libraries built here (libtomi_core.so, libtomi_highlight.so) and for packaging. Every phone with
// minSdk 33 (recent phones included) is 64-bit ARM. Add "x86_64" to run on an emulator: everything is rebuilt for it, nothing else to change.
val nativeAbis = listOf("arm64-v8a")

// Value of a release-signing setting: Gradle property first (for example in ~/.gradle/gradle.properties on a maintainer's machine),
// then environment variable (CI). Blank counts as absent.
fun releaseSigningValue(name: String): String? =
    providers.gradleProperty(name).orElse(providers.environmentVariable(name)).orNull?.trim()?.takeIf { it.isNotEmpty() }

// Rust core profile: "release" (default: what ships, fast crypto) or "debug" (faster to compile, much slower at run time).
// Switch with `-PtomiCoreProfile=debug`.
val tomiCoreRelease = providers.gradleProperty("tomiCoreProfile").orElse("release").map { it != "debug" }

android {
    namespace = "eu.shaffe.tomi"
    // API 37.2: Compose 1.13 (compose-bom 2026.11) needs 37.1 or later. Same platform as the CI (`platforms;android-37.2`) and docs/DEV.md.
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }
    // Same NDK as the CI (NDK_VERSION) and docs/DEV.md: one NDK for libtomi_highlight (CMake) and libtomi_core (cargo-ndk).
    ndkVersion = "27.0.12077973"

    defaultConfig {
        // `eu.shaffe.tomi` is the identifier of the official build. A fork that distributes its own build must use its own
        // (and its own name and signing key): `-PtomiApplicationId=com.example.myfork`, see docs/FORKING.md and TRADEMARKS.md.
        // The Kotlin package (`namespace`) is independent of it and stays as is.
        val appId = providers.gradleProperty("tomiApplicationId").orElse("eu.shaffe.tomi").get().trim()
        require(Regex("^[a-z][a-z0-9_]*(\\.[a-z][a-z0-9_]*)+$").matches(appId)) { "tomiApplicationId must be a valid application id such as com.example.app (got '$appId')" }
        applicationId = appId
        minSdk = 33
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        // Corner radii: false = Tomi charter (cards/buttons 8, sheet/FAB 12), true = Material 3 Expressive (rounder).
        // Switch with `-PtomiExpressiveRadii=true` (or in gradle.properties). Default: charter radii.
        val expressiveRadii = providers.gradleProperty("tomiExpressiveRadii").orElse("false").get().toBoolean()
        buildConfigField("boolean", "EXPRESSIVE_RADII", expressiveRadii.toString())

        // The tree-sitter library of Enriched Markdown weighs about 8 MB per ABI (stored uncompressed in the APK), and our own
        // libtomi_highlight.so (PHP, see src/main/cpp) about 1.5 MB more. Only the ABIs of [nativeAbis] are shipped.
        ndk { abiFilters += nativeAbis }

        // Relay compiled into the app: OPTIONAL, and empty by default (no address in the sources: the relay of a release build comes from the
        // pairing link, a scanned QR code, the "Relais" field or the creation link, spec/pairing-v1.md 4.3). A relay read from a link or
        // typed is not trusted: the core shows it in full on the verification screen and keeps it only once the pairing succeeded.
        // Give your own with `-PtomiRelayUrl=https://relay.example.org` (TLS with a public certificate: the core trusts the bundled web
        // roots only). Empty = null in CoreConfig.relayUrl.
        val relayUrl = providers.gradleProperty("tomiRelayUrl").orElse("").get().trim().trimEnd('/')
        require(relayUrl.isEmpty() || Regex("^https://[A-Za-z0-9.-]+(:[0-9]{1,5})?$").matches(relayUrl)) { "tomiRelayUrl must be empty or https://host[:port] (got '$relayUrl')" }
        buildConfigField("String", "RELAY_URL", "\"$relayUrl\"")

        // libtomi_highlight.so (PHP tree-sitter), see src/main/cpp. c++_static: no libc++_shared to share with the other libraries.
        externalNativeBuild {
            cmake {
                arguments += listOf(
                    "-DTOMI_GRAMMARS_DIR=" + grammarSourcesDir.asFile.absolutePath.replace('\\', '/'),
                    "-DANDROID_STL=c++_static",
                    "-DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON",
                )
            }
        }
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    // Release signing. The key is NEVER in the repository: CI (and a maintainer building a release by hand) supplies it through
    // environment variables or Gradle properties of the same names (docs/RELEASES.md explains how to create the keystore):
    //   TOMI_RELEASE_KEYSTORE            path of the keystore file
    //   TOMI_RELEASE_KEYSTORE_PASSWORD   store password
    //   TOMI_RELEASE_KEY_ALIAS           key alias
    //   TOMI_RELEASE_KEY_PASSWORD        key password (same as the store password for a PKCS12 keystore)
    // Without TOMI_RELEASE_KEYSTORE the `release` build is left UNSIGNED (`app-release-unsigned.apk`): it builds, it does not install,
    // and it can never be mistaken for an official build. It is never silently signed with the debug key.
    signingConfigs {
        val releaseKeystorePath = releaseSigningValue("TOMI_RELEASE_KEYSTORE")
        if (releaseKeystorePath != null) {
            create("release") {
                val keystore = project.file(releaseKeystorePath)
                require(keystore.isFile) { "TOMI_RELEASE_KEYSTORE does not point to a file: $keystore" }
                storeFile = keystore
                storePassword = requireNotNull(releaseSigningValue("TOMI_RELEASE_KEYSTORE_PASSWORD")) { "TOMI_RELEASE_KEYSTORE_PASSWORD is not set" }
                keyAlias = requireNotNull(releaseSigningValue("TOMI_RELEASE_KEY_ALIAS")) { "TOMI_RELEASE_KEY_ALIAS is not set" }
                keyPassword = requireNotNull(releaseSigningValue("TOMI_RELEASE_KEY_PASSWORD")) { "TOMI_RELEASE_KEY_PASSWORD is not set" }
            }
        }
    }

    buildTypes {
        release {
            // The only variant that is published (see .github/workflows/build-apps.yml). Never debuggable.
            isDebuggable = false
            isMinifyEnabled = false
            // Not read while minification is off. JNA and the UniFFI bindings need keep rules the day it is turned on.
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Signed only when a release key is supplied (see above); unsigned otherwise.
            signingConfigs.findByName("release")?.let { signingConfig = it }
        }
        // The build to judge SPEED on (`assemblePreview`): the release code, signed with the debug key so that it installs over
        // the debug build on a developer's phone. A debug build is debuggable: ART does not optimise it and it carries no baseline
        // profile, so Compose runs several times slower there than in what ships. DEVELOPER ONLY, never distributed: it carries the
        // debug key, and CI does not build it.
        create("preview") {
            initWith(getByName("release"))
            isDebuggable = false
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += "release"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    lint {
        abortOnError = true
        warningsAsErrors = false
        checkReleaseBuilds = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
        // libtomi_core.so is built by the buildTomiCoreLibs task below. A copy that cargo-ndk dropped into src/main/jniLibs
        // (`just android-ndk-libs`, the CI pre-step) must not make the merge fail on a duplicate: any of them is the same code.
        jniLibs.pickFirsts += "**/libtomi_core.so"
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_21)
        // Floating toolbar and other Material 3 Expressive components are still marked experimental in material3 1.5 alphas.
        optIn.add("androidx.compose.material3.ExperimentalMaterial3ExpressiveApi")
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    debugImplementation(libs.androidx.compose.ui.tooling)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.fragment.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.work.runtime.ktx)
    implementation(libs.androidx.biometric)
    // EXIF orientation of the photos shown by the image viewer (the framework class is flagged by lint for its parsing bugs).
    implementation(libs.androidx.exifinterface)
    implementation(libs.kotlinx.coroutines.android)

    // QR scanning: CameraX for the preview/analysis pipeline, ZXing core (pure Java) for decoding.
    implementation(libs.androidx.camera.core)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.zxing.core)

    // Rust core (tomi-core) through UniFFI: the Kotlin bindings are generated by the build (generateUniffiKotlin) and call libtomi_core.so
    // through JNA. The AAR flavour is required on Android (it carries libjnidispatch.so).
    implementation(variantOf(libs.jna) { artifactType("aar") })

    // Markdown rendering: native text (no WebView, no HTML) with tree-sitter code colouring. Remote images are never loaded (MarkdownGuard).
    // Pinned to the EXACT version (strictly): TomiCodeHighlight relies on the internal plugin API and on the SyntaxTokenType order of
    // 0.3.0, so a transitive bump must fail the build instead of silently changing them. Bump deliberately: enrichedMarkdown in libs.versions.toml.
    implementation(libs.enriched.markdown.compose) { version { strictly(libs.versions.enrichedMarkdown.get()) } }
    implementation(libs.enriched.markdown.code.highlight) { version { strictly(libs.versions.enrichedMarkdown.get()) } }
    // HTML sanitising for the sandboxed WebView.
    implementation(libs.jsoup)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    // The plain JNA jar carries the desktop natives: RealCoreHostIntegrationTest runs the generated bindings against the host build of the
    // Rust core. (The AAR above only has the Android ones.)
    testImplementation(libs.jna)
}

// PHP grammar + tree-sitter runtime for libtomi_highlight.so (src/main/cpp). The generated PHP parser weighs 7 MB of C: it is not
// committed, but cloned once at a pinned commit (a commit hash fixes the content) into
// apps/android/.grammar-sources (ignored by git). The first build needs network and git; later builds, and the app itself, do not.
abstract class FetchGrammarSources @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    /** Folder name -> "repository url#commit". */
    @get:Input
    abstract val sources: MapProperty<String, String>

    @get:OutputDirectory
    abstract val cacheDir: DirectoryProperty

    @TaskAction
    fun fetch() {
        val root = cacheDir.get().asFile
        root.mkdirs()
        sources.get().forEach { (name, spec) ->
            val url = spec.substringBefore('#')
            val commit = spec.substringAfter('#')
            val dir = File(root, name)
            if (dir.isDirectory && git(dir, "rev-parse", "HEAD").trim() == commit) return@forEach
            logger.lifecycle("Fetching $name @ $commit")
            dir.deleteRecursively()
            dir.mkdirs()
            git(dir, "init", "-q")
            git(dir, "fetch", "-q", "--depth", "1", url, commit)
            git(dir, "checkout", "-q", "--detach", "FETCH_HEAD")
            check(git(dir, "rev-parse", "HEAD").trim() == commit) { "$name: unexpected commit after fetch" }
        }
    }

    private fun git(dir: File, vararg args: String): String {
        val out = ByteArrayOutputStream()
        val result = exec.exec {
            workingDir = dir
            commandLine(listOf("git") + args)
            standardOutput = out
            errorOutput = out
            isIgnoreExitValue = true
        }
        if (result.exitValue != 0 && args[0] != "rev-parse") error("git ${args.joinToString(" ")} failed in $dir:\n$out")
        return out.toString()
    }
}

val fetchGrammarSources = tasks.register<FetchGrammarSources>("fetchGrammarSources") {
    sources.put("tree-sitter", "https://github.com/tree-sitter/tree-sitter#da6fe9beb4f7f67beb75914ca8e0d48ae48d6406") // v0.25.10, MIT
    sources.put("tree-sitter-php", "https://github.com/tree-sitter/tree-sitter-php#58d5643086ae60e93a3746cca648363ee97c8761") // MIT
    cacheDir.set(grammarSourcesDir)
}
tasks.configureEach {
    if (name.contains("CMake") || name.startsWith("generateJsonModel") || name.startsWith("externalNativeBuild")) dependsOn(fetchGrammarSources)
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Rust core: libtomi_core.so + UniFFI Kotlin bindings (crates/core, spec/pairing-v1.md)
//
// Both are BUILT here from crates/core and never committed (they live under app/build/, ignored by git):
// - buildTomiCoreLibs    cargo-ndk  -> build/tomi-core/jniLibs/<abi>/libtomi_core.so, packaged as a generated jniLibs directory;
// - generateUniffiKotlin uniffi-bindgen (library mode) -> build/generated/uniffi/kotlin/uniffi/tomi_core/tomi_core.kt, compiled with the app.
// The bindings are generated from the HOST build of the core, not from the Android library: the release profile strips the symbols that
// library mode reads, and the generated code does not depend on the target. Compiling the Kotlin (and the unit tests) therefore needs
// cargo but not the NDK; only the APK needs libtomi_core.so.
//
// Prerequisites (docs/DEV.md, justfile): Rust with `rustup target add aarch64-linux-android`, `cargo install cargo-ndk --locked`,
// and the Android NDK 27 (ndkVersion above). cargo's own cache (target/) makes a second run a no-op.
// ---------------------------------------------------------------------------------------------------------------------------------
abstract class TomiCoreTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    /** Everything cargo reads to build tomi-core: a change re-runs the task, no change keeps its outputs. */
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val rustSources: ConfigurableFileCollection

    @get:Internal
    abstract val workspaceDir: DirectoryProperty

    protected fun cargo(env: Map<String, String>, vararg args: String) {
        val command = listOf(cargoExecutable()) + args
        val result = try {
            exec.exec {
                workingDir = workspaceDir.get().asFile
                commandLine(command)
                environment(env)
                isIgnoreExitValue = true
            }
        } catch (e: Exception) {
            throw GradleException("Cannot run cargo (${e.message}). ${prerequisites()}", e)
        }
        if (result.exitValue != 0) {
            throw GradleException("`cargo ${args.joinToString(" ")}` failed (exit code ${result.exitValue}). ${prerequisites()}")
        }
    }

    /** `cargo` from CARGO_HOME or ~/.cargo/bin when it is there (a Gradle daemon started before rustup edited the PATH does not see it). */
    private fun cargoExecutable(): String {
        val name = if (hostIsWindows()) "cargo.exe" else "cargo"
        val homes = listOfNotNull(System.getenv("CARGO_HOME"), File(System.getProperty("user.home"), ".cargo").path)
        return homes.map { File(it, "bin/$name") }.firstOrNull { it.isFile }?.absolutePath ?: "cargo"
    }

    protected fun hostIsWindows(): Boolean = System.getProperty("os.name").lowercase().contains("win")

    protected fun hostIsMac(): Boolean = System.getProperty("os.name").lowercase().contains("mac")

    private fun prerequisites() =
        "Prerequisites: Rust (rustup) with `rustup target add aarch64-linux-android`, `cargo install cargo-ndk --locked`, " +
            "and the Android NDK 27 (docs/DEV.md)."
}

/** libtomi_core.so for each ABI, with cargo-ndk, into [outputDir]/<abi>/. */
abstract class BuildTomiCoreLibs @Inject constructor(exec: ExecOperations) : TomiCoreTask(exec) {
    @get:Input
    abstract val abis: ListProperty<String>

    /** Android API level the library is linked against (minSdk). */
    @get:Input
    abstract val apiLevel: Property<Int>

    @get:Input
    abstract val release: Property<Boolean>

    /** The NDK AGP uses, so that Rust and CMake build with the same one. */
    @get:Input
    abstract val ndkDir: Property<String>

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        val args = mutableListOf("ndk", "--platform", apiLevel.get().toString())
        abis.get().forEach { args += listOf("--target", it) }
        args += listOf("--output-dir", out.absolutePath, "build", "--package", "tomi-core", "--features", "demo", "--locked")
        if (release.get()) args += "--release"
        cargo(mapOf("ANDROID_NDK_HOME" to ndkDir.get()), *args.toTypedArray())
        abis.get().forEach { abi -> check(File(out, "$abi/libtomi_core.so").isFile) { "cargo-ndk produced no $abi/libtomi_core.so" } }
    }
}

/** The Kotlin side of the UniFFI interface of crates/core/src/ffi.rs, into [outputDir]/uniffi/tomi_core/tomi_core.kt. */
abstract class GenerateUniffiKotlin @Inject constructor(exec: ExecOperations) : TomiCoreTask(exec) {
    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        // Unstripped host library (debug profile): library mode reads the UNIFFI_META symbols out of it.
        val features = arrayOf("--package", "tomi-core", "--features", "ffi-bindgen")
        cargo(emptyMap(), "build", "--locked", *features)
        val workspace = workspaceDir.get().asFile
        val targetDir = System.getenv("CARGO_TARGET_DIR")?.let { File(it).let { dir -> if (dir.isAbsolute) dir else File(workspace, it) } }
            ?: File(workspace, "target")
        val libraryName = when {
            hostIsWindows() -> "tomi_core.dll"
            hostIsMac() -> "libtomi_core.dylib"
            else -> "libtomi_core.so"
        }
        val library = File(targetDir, "debug/$libraryName")
        check(library.isFile) { "cargo did not produce ${library.absolutePath}" }

        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        cargo(
            emptyMap(), "run", "--locked", *features, "--bin", "uniffi-bindgen", "--",
            "generate", "--library", library.absolutePath, "--language", "kotlin", "--out-dir", out.absolutePath, "--no-format",
        )
        check(File(out, "uniffi/tomi_core/tomi_core.kt").isFile) { "uniffi-bindgen produced no uniffi/tomi_core/tomi_core.kt" }
    }
}

val tomiCoreSources = files(
    fileTree(repoRootDir.dir("crates/core/src")),
    repoRootDir.file("crates/core/Cargo.toml"),
    repoRootDir.file("Cargo.toml"),
    repoRootDir.file("Cargo.lock"),
)

val buildTomiCoreLibs = tasks.register<BuildTomiCoreLibs>("buildTomiCoreLibs") {
    group = "tomi"
    description = "Builds libtomi_core.so (Rust core) with cargo-ndk"
    rustSources.from(tomiCoreSources)
    workspaceDir.set(repoRootDir)
    abis.set(nativeAbis)
    apiLevel.set(android.defaultConfig.minSdk ?: 33)
    release.set(tomiCoreRelease)
    ndkDir.set(androidComponents.sdkComponents.ndkDirectory.map { it.asFile.absolutePath })
    outputDir.set(layout.buildDirectory.dir("tomi-core/jniLibs"))
}

val generateUniffiKotlin = tasks.register<GenerateUniffiKotlin>("generateUniffiKotlin") {
    group = "tomi"
    description = "Generates the UniFFI Kotlin bindings of the Rust core"
    rustSources.from(tomiCoreSources)
    workspaceDir.set(repoRootDir)
    outputDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
}

// The unit tests that run the generated bindings against the real core (RealCoreHostIntegrationTest) load the host build that
// generateUniffiKotlin makes, in the debug profile of the Cargo target directory. They skip themselves when it is not there.
tasks.withType<Test>().configureEach {
    dependsOn(generateUniffiKotlin)
    val cargoTarget = System.getenv("CARGO_TARGET_DIR")?.let { File(it).let { dir -> if (dir.isAbsolute) dir else File(repoRootDir.asFile, it) } }
        ?: File(repoRootDir.asFile, "target")
    val hostLibrary = when {
        System.getProperty("os.name").lowercase().contains("win") -> "tomi_core.dll"
        System.getProperty("os.name").lowercase().contains("mac") -> "libtomi_core.dylib"
        else -> "libtomi_core.so"
    }
    systemProperty("tomi.core.hostLibrary", File(cargoTarget, "debug/$hostLibrary").absolutePath)
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(buildTomiCoreLibs, BuildTomiCoreLibs::outputDir)
        // Registered as a generated *java* source directory on purpose: the Kotlin compiler takes the .kt files of it, and unlike
        // `sources.kotlin` (AGP 8.13 + Kotlin 2.3) it makes compileXKotlin depend on the task that generates them.
        variant.sources.java?.addGeneratedSourceDirectory(generateUniffiKotlin, GenerateUniffiKotlin::outputDir)
    }
}
