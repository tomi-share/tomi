# Not applied while isMinifyEnabled is false (build.gradle.kts). Kept ready for the release build with R8.

# JNA calls the UniFFI native library by reflection and fills Structure subclasses field by field: keep its classes whole.
-dontwarn java.awt.*
-keep class com.sun.jna.* { *; }
-keepclassmembers class * extends com.sun.jna.* { public *; }

# The UniFFI Kotlin bindings of the Rust core (uniffi.tomi_core): JNA reflects on the FFI structures and callback interfaces.
-keep class uniffi.tomi_core.** { *; }
