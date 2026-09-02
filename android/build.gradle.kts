plugins {
    // AGP 9 起 Kotlin 支持内置，不能再应用 org.jetbrains.kotlin.android（会直接报错）。
    // 升到 9.x 的直接原因是 termlib（libvterm + Compose 的终端组件）要 Kotlin 2.3 与
    // 2026 版 Compose BOM，而那一组又要 compileSdk 36。
    id("com.android.application") version "9.3.1" apply false
    id("com.android.library") version "9.3.1" apply false
    id("org.jetbrains.kotlin.plugin.serialization") version "2.3.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.3.21" apply false
}
