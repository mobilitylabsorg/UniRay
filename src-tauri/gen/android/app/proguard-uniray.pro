# gomobile bindings are reached through JNI by name.
-keep class go.** { *; }
-keep class libXray.** { *; }
# Instantiated by Tauri's plugin manager through reflection.
-keep class dev.uniray.app.VpnPlugin { *; }
-keep class dev.uniray.app.StartArgs { *; }
-keep class dev.uniray.app.DelayArgs { *; }
-keep class dev.uniray.app.NotificationArgs { *; }
-keep class dev.uniray.app.StealthArgs { *; }
# Shizuku's process API (newProcess) is @hide and reached by reflection.
-keep class rikka.shizuku.Shizuku { *; }
