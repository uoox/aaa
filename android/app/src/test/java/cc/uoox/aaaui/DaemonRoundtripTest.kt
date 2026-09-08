package cc.uoox.aaaui

import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.junit.Assert.assertTrue
import org.junit.Assume
import org.junit.Test
import java.io.File
import java.net.ServerSocket
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * 与真 aaa-daemon 的端到端回路：隔离环境（AAA_HOME/AAA_DAEMON_CONFIG 指向 tempdir，
 * project_root 为 tempdir，绝不触碰真实 HOME/项目根）起 daemon → REST 建 shell 会话 →
 * attach WS → 发 echo → 断言 vendor 模拟器屏幕包含输出。
 *
 * 找不到 daemon 二进制时跳过（Assume）。可用 AAA_DAEMON_BIN 环境变量指定路径。
 */
class DaemonRoundtripTest {

    private class Env(val tmp: File, val projects: File, val log: File, val proc: Process, val api: DaemonClient)

    /** 起隔离 daemon；二进制缺失时 Assume-skip。 */
    private fun launchDaemon(): Env {
        val bin = File(
            System.getenv("AAA_DAEMON_BIN") ?: "../../daemon/target/release/aaa-daemon",
        ).absoluteFile.normalize()
        Assume.assumeTrue("aaa-daemon binary not found at $bin — skipping roundtrip test", bin.canExecute())

        val tmp = Files.createTempDirectory("aaa-roundtrip").toFile()
        val home = File(tmp, "home").apply { mkdirs() }
        val projects = File(tmp, "projects").apply { mkdirs() }
        val port = ServerSocket(0).use { it.localPort }
        val token = "aaa_tk_roundtrip_test"
        val cfgFile = File(tmp, "config.toml").apply {
            writeText(
                """
                port = $port
                token = "$token"
                project_root = "${projects.absolutePath}"
                namer = false
                """.trimIndent() + "\n",
            )
        }
        val log = File(tmp, "daemon.log")
        val proc = ProcessBuilder(bin.absolutePath, "run")
            .redirectErrorStream(true)
            .redirectOutput(log)
            .apply {
                environment()["AAA_HOME"] = home.absolutePath
                environment()["AAA_DAEMON_CONFIG"] = cfgFile.absolutePath
                environment()["HOME"] = home.absolutePath
            }
            .start()
        val api = DaemonClient("http://127.0.0.1:$port", token)
        runBlocking {
            var up = false
            for (i in 0 until 60) {
                try { api.health(); up = true; break } catch (_: Exception) { delay(250) }
            }
            assertTrue("daemon did not come up on :$port; log:\n${log.readTextSafe()}", up)
        }
        return Env(tmp, projects, log, proc, api)
    }

    private fun teardown(env: Env) {
        env.proc.destroy()
        if (!env.proc.waitFor(3, TimeUnit.SECONDS)) env.proc.destroyForcibly()
        env.tmp.deleteRecursively()
    }

    @Test fun shellEchoRoundtripThroughAttachWs() {
        val env = launchDaemon()
        val projects = env.projects
        val log = env.log

        try {
            val api = env.api
            runBlocking {

                val sess = api.createSession(File(projects, "rt-demo").absolutePath, "shell", resume = false)
                assertTrue(sess.id.isNotBlank())

                val frames = LinkedBlockingQueue<ByteArray>()
                val opened = CountDownLatch(1)
                var wsRef: WebSocket? = null
                wsRef = api.attachSocket(sess.id, object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) { opened.countDown() }
                    override fun onMessage(webSocket: WebSocket, bytes: ByteString) { frames.add(bytes.toByteArray()) }
                })
                assertTrue("attach WS did not open", opened.await(10, TimeUnit.SECONDS))
                wsRef.send("""{"t":"resize","cols":120,"rows":30}""") // hello 后宣告网格 → {"t":"resize"} 文本帧

                // 单线程泵：WS 二进制帧攒成原始字节流。终端模拟器（libvterm）是 JNI，JVM 单测里
                // 不能起，所以这里只看字节里有没有期望的文本——shell 的输出是明文行。
                val raw = StringBuilder()
                fun transcript(): String = raw.toString()
                fun pump(timeoutMs: Long, until: () -> Boolean): Boolean {
                    val deadline = System.currentTimeMillis() + timeoutMs
                    while (System.currentTimeMillis() < deadline) {
                        frames.poll(200, TimeUnit.MILLISECONDS)?.let { raw.append(it.toString(Charsets.UTF_8)) }
                        if (until()) return true
                    }
                    return until()
                }

                assertTrue("no replay/prompt output arrived; log:\n${log.readTextSafe()}", pump(15_000) { transcript().isNotBlank() })

                // 用户输入路径：二进制 WS 帧 → PTY
                wsRef.send("echo aaa-roundtrip-\$((6*7))\r".toByteArray().toByteString())
                assertTrue(
                    "echo output missing; screen:\n${transcript()}\nlog:\n${log.readTextSafe()}",
                    pump(20_000) { transcript().contains("aaa-roundtrip-42") },
                )

                // composer 路径：POST /sessions/:id/input → PTY，输出同样回流到 attach WS
                api.input(sess.id, "echo via-rest-\$((5*5))", enter = true)
                assertTrue(
                    "REST input output missing; screen:\n${transcript()}",
                    pump(20_000) { transcript().contains("via-rest-25") },
                )

                wsRef.cancel()
                api.kill(sess.id)
                api.deleteSession(sess.id)
            }
        } finally {
            teardown(env)
        }
    }

    /** projects / sessions 管理端点全链路（app 各屏依赖的 REST 面）。 */
    @Test fun projectsAndSessionLifecycleThroughRest() {
        val env = launchDaemon()
        try {
            val api = env.api
            runBlocking {
                val health = api.health()
                assertTrue(health.ssd_mounted)
                assertTrue(health.version.isNotBlank())

                // 新建屏：POST /projects → 完整项目对象
                val proj = api.createProject("Demo Project", agent = "claude")
                assertTrue("unexpected slug: ${proj.path}", proj.path.endsWith("Demo-Project")) // slugify 保大小写、空格→-
                assertTrue(File(proj.path).isDirectory)

                // 项目屏：列表含它
                val listed = api.projects()
                assertTrue(listed.any { it.path == proj.path && it.agent == "claude" })

                // 会话：shell 会话 + 重命名 + 详情 + 列表
                val sess = api.createSession(proj.path, "shell", resume = false)
                api.rename(sess.id, "重命名测试")
                assertTrue(api.sessions().first { it.id == sess.id }.title == "重命名测试")
                // v1.17 详情：终端会话四样都空，但形状要能解析
                val detail = api.detail(sess.id)
                assertTrue(detail.subagents.isEmpty() && detail.background_tasks.isEmpty())
                assertTrue(detail.uploads.isEmpty() && detail.skills.isEmpty())

                // 删除项目 → purge 报告
                api.kill(sess.id)
                api.deleteSession(sess.id)
                val results = api.deleteProjects(listOf(proj.path))
                assertTrue(results.single().ok)
                assertTrue(!File(proj.path).exists())
                assertTrue(api.projects().none { it.path == proj.path })
            }
        } finally {
            teardown(env)
        }
    }

    private fun File.readTextSafe(): String = try { readText().takeLast(4000) } catch (_: Exception) { "<no log>" }
}
