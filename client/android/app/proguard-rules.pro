-keep class app.turattext.mobile.core.NativeCore { *; }
-keepclasseswithmembernames class * { native <methods>; }

# Имя темы сохраняется в настройках устройства: константы перечисления нельзя переименовывать.
-keepclassmembers enum app.turattext.mobile.ui.AppTheme { *; }
