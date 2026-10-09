# gomobile bindings are reached through JNI by name.
-keep class go.** { *; }
-keep class libXray.** { *; }
# Instantiated by Tauri's plugin manager through reflection.
-keep class dev.uniray.app.VpnPlugin { *; }
-keep class dev.uniray.app.StartArgs { *; }
-keep class dev.uniray.app.DelayArgs { *; }
-keep class dev.uniray.app.NotificationArgs { *; }
-keep class dev.uniray.app.StealthArgs { *; }
-keep class dev.uniray.app.ProbeArgs { *; }
-keepclassmembers enum dev.uniray.app.Privilege { *; }
-keep class dev.uniray.app.Privilege { *; }
-keep class dev.uniray.app.PrivilegeState { *; }
-keep class dev.uniray.app.PrivilegedShell { *; }
# Shizuku's process API (newProcess) is @hide and reached by reflection.
-keep class rikka.shizuku.** { *; }
-keep class moe.shizuku.** { *; }
# libsu runs shell and background binder services
-keep class com.topjohnwu.superuser.** { *; }
