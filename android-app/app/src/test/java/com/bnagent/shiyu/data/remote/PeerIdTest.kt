package com.bnagent.shiyu.data.remote

import org.junit.Assert.assertEquals
import org.junit.Test

/** peer_id 生成规则：每个会话固定 `android:<convId>`（team-lead 契约）。 */
class PeerIdTest {

    @Test
    fun `peer id prefixes android`() {
        assertEquals("android:lin", AgentClient.peerIdFor("lin"))
        assertEquals("android:team", AgentClient.peerIdFor("team"))
    }

    @Test
    fun `peer id keeps conversation id verbatim`() {
        assertEquals("android:group_2026-11", AgentClient.peerIdFor("group_2026-11"))
    }
}
