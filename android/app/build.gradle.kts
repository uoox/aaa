plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android { namespace = "cc.uoox.aaaui"; compileSdk = 36
    defaultConfig { applicationId = "cc.uoox.aaaui"; minSdk = 29; targetSdk = 36; versionCode = 34; versionName = "1.16.1" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
    lint { abortOnError = true; checkReleaseBuilds = false }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material3.windowsize)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.datastore.preferences)
    implementation(libs.okhttp)
    implementation(libs.serialization.json)
    implementation(libs.coroutines.android)
    implementation(libs.zxing.embedded)
    implementation(libs.commonmark)
    implementation(libs.commonmark.gfm.tables)
    implementation(libs.commonmark.gfm.strikethrough)
    // 终端：ConnectBot 的 termlib（libvterm 走 JNI 解析，Compose Canvas 渲染）。Maven 上的 AAR
    // 自带四个 ABI 的 .so，不需要 NDK。键盘与鼠标手势是我们自己的，见 TermInput.kt / TerminalHost.kt。
    implementation(libs.termlib)
    testImplementation(libs.junit)
    testImplementation(libs.coroutines.android)
}
