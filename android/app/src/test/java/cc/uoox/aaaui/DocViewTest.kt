package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 产物阅读与视图轮换：这两个口径两端必须一致（mac 的 `doc_view.rs` / `ui/mod.rs`
 * 里有逐条对应的测试）。不一致的话同一份文件在两台设备上会显示成不同的大小，
 * ⌘E 与顶栏那一格还会走不同的顺序。
 */
class DocViewTest {
    @Test fun sizesReadLikeAFileManager() {
        assertEquals("0 B", humanSize(0))
        assertEquals("812 B", humanSize(812))
        assertEquals("2.0 KB", humanSize(2048))
        assertEquals("88 KB", humanSize(90_000))
        assertEquals("5.0 MB", humanSize(5L * 1024 * 1024))
        assertEquals("3.0 GB", humanSize(3L * 1024 * 1024 * 1024))
    }

    /** 只剩两种看法（v1.35 删掉「浏览」）：终端 ⇄ 消息流，画不出消息流就只有终端。 */
    @Test fun viewCycleSkipsWhatCannotBeDrawn() {
        assertEquals("messages", nextView("terminal", true))
        assertEquals("terminal", nextView("messages", true))
        assertEquals("terminal", nextView("terminal", false))
        assertEquals("terminal", nextView("messages", false))
        assertEquals("消息流", viewLabelOf("messages"))
        assertEquals("终端", viewLabelOf("terminal"))
    }
}
