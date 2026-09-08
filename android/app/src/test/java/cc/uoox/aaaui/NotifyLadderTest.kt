package cc.uoox.aaaui

import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 一次会话更新最多响一声（PROTOCOL「通知策略」）。v1.22 之前这里是四个平铺的 `if`，
 * 同一拍里 asking 翻 true 又 running→waiting 会弹两条说同一件事；mac 侧一直是阶梯。
 */
class NotifyLadderTest {
    private fun sess(
        id: String = "s1",
        agent: String = "claude",
        state: String = "waiting",
        asking: Boolean = false,
        error: String? = null,
        exit: Int? = null,
    ) = Session(
        id = id, title = "t", project_path = "/p/a", project_name = "a", agent = agent,
        state = state, asking = asking, error = error, exit_code = exit,
    )

    @Test
    fun 一次更新只响一声() {
        // 这一拍里两件事同时发生：跑完了，而且弹出了表单。只报「待回复」——它更要紧，
        // 而且说的是同一轮的同一件事。
        val ev = notifyEventFor(sess(asking = true), sess(state = "running"), prev = "running", killedHere = false)
        assertTrue("$ev", ev is NotifyEvent.Asking)
    }

    @Test
    fun 跑完一轮报运行结束() {
        val ev = notifyEventFor(sess(state = "waiting"), sess(state = "running"), prev = "running", killedHere = false)
        assertTrue("$ev", ev is NotifyEvent.Done && !ev.exited)
    }

    @Test
    fun 出错优先于跑完() {
        val ev = notifyEventFor(sess(error = "rate_limit"), sess(state = "running"), prev = "running", killedHere = false)
        assertTrue("$ev", ev is NotifyEvent.Error)
    }

    @Test
    fun 同一个错误不重复报() {
        val old = sess(error = "rate_limit", state = "waiting")
        assertNull(notifyEventFor(sess(error = "rate_limit"), old, prev = "waiting", killedHere = false))
    }

    @Test
    fun 正常退出与自己动手终止都不响() {
        val old = sess(state = "running")
        assertNull("退出码 0 是正常收工", notifyEventFor(sess(state = "exited", exit = 0), old, "running", false))
        assertNull("自己按的结束不用报告", notifyEventFor(sess(state = "exited", exit = 1), old, "running", true))
        val ev = notifyEventFor(sess(state = "exited", exit = 1), old, "running", false)
        assertTrue("$ev", ev is NotifyEvent.Done && ev.exited)
    }

    @Test
    fun 终端不响() {
        assertNull(notifyEventFor(sess(agent = "shell", state = "waiting"), sess(agent = "shell", state = "running"), "running", false))
    }
}
