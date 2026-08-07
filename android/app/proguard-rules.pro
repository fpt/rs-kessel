# The JNI entry points in libkessel_ffi.so bind to `dev.kessel.vm.KesselNative`
# by name at first call. R8 has no way to see that reference, so without this
# a release build shrinks the class away and the app dies on
# UnsatisfiedLinkError -- at runtime, not at build time.
-keep class dev.kessel.vm.KesselNative { *; }
-keepclasseswithmembernames class * { native <methods>; }
