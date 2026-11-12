# 拾语 Android 混淆规则占位。
# kotlinx.serialization 的 @Serializable 类在开启混淆后需要保留序列化器：
-keepattributes *Annotation*, InnerClasses
-keep,includedescriptorclasses class com.bnagent.shiyu.**$$serializer { *; }
-keepclassmembers class com.bnagent.shiyu.** {
    *** Companion;
}
-keepclasseswithmembers class com.bnagent.shiyu.** {
    kotlinx.serialization.KSerializer serializer(...);
}
