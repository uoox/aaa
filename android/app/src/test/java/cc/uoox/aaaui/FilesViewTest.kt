package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 目录浏览与视图轮换：这三个口径两端必须一致（mac 的 `files_view.rs` / `ui/mod.rs`
 * 里有逐条对应的测试）。不一致的话同一个目录在两台设备上会显示成不同的大小、
 * 不同的路径，⌘E 与顶栏那一格还会走不同的顺序。
 */
class FilesViewTest {
    @Test fun sizesReadLikeAFileManager() {
        assertEquals("0 B", humanSize(0))
        assertEquals("812 B", humanSize(812))
        assertEquals("2.0 KB", humanSize(2048))
        assertEquals("88 KB", humanSize(90_000))
        assertEquals("5.0 MB", humanSize(5L * 1024 * 1024))
        assertEquals("3.0 GB", humanSize(3L * 1024 * 1024 * 1024))
    }

    @Test fun crumbsAreRelativeToTheProject() {
        assertEquals("aaa", fileCrumb("/p/aaa", "/p/aaa"))
        assertEquals("aaa/src/ui", fileCrumb("/p/aaa", "/p/aaa/src/ui"))
        assertEquals("/elsewhere", fileCrumb("/p/aaa", "/elsewhere"))
    }

    @Test fun viewCycleSkipsWhatCannotBeDrawn() {
        assertEquals("messages", nextView("terminal", true))
        assertEquals("files", nextView("messages", true))
        assertEquals("terminal", nextView("files", true))
        assertEquals("files", nextView("terminal", false))
        assertEquals("terminal", nextView("files", false))
        assertEquals("files", nextView("messages", false))
    }
}
