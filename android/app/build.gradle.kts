plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android { namespace = "cc.uoox.aaaui"; compileSdk = 36
    defaultConfig { applicationId = "cc.uoox.aaaui"; minSdk = 29; targetSdk = 36; versionCode = 5; versionName = "1.2.0" }
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
    implementation(project(":terminal-view"))
    // 第二套终端：ConnectBot 的 termlib（libvterm 走 JNI 解析，Compose Canvas 渲染）。
    // 与 termux 那套并存，设置里可切，看过效果再决定去留。
    implementation(libs.termlib)
    testImplementation(libs.junit)
    testImplementation(libs.coroutines.android)
}
