package com.bnagent.shiyu.data.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** 后端错误响应解析（backend-multisession 真实错误体）与 4xx/5xx 映射。 */
class AgentErrorParsingTest {

    @Test
    fun `parses error string field`() {
        assertEquals(
            "LLM not configured",
            parseErrorMessage("""{"error":"LLM not configured"}""", 503),
        )
    }

    @Test
    fun `parses nested error message`() {
        assertEquals(
            "boom",
            parseErrorMessage("""{"error":{"message":"boom","code":42}}""", 500),
        )
    }

    @Test
    fun `parses detail field`() {
        assertEquals("Not Found", parseErrorMessage("""{"detail":"Not Found"}""", 404))
    }

    @Test
    fun `parses plain text body`() {
        assertEquals(
            "missing or invalid API key",
            parseErrorMessage("missing or invalid API key", 401),
        )
    }

    @Test
    fun `ignores html error pages`() {
        assertEquals(
            "服务器内部错误",
            parseErrorMessage("<html><body>502 Bad Gateway</body></html>", 502),
        )
    }

    @Test
    fun `falls back by status code for empty body`() {
        assertEquals("API Key 无效或缺失", parseErrorMessage("", 401))
        assertEquals("接口不存在，请检查服务器地址与版本", parseErrorMessage(null, 404))
        assertEquals("请求过于频繁，请稍后重试", parseErrorMessage("   ", 429))
    }

    @Test
    fun `maps 401 to unauthorized`() {
        val error = agentExceptionForHttp(401, "missing or invalid API key")
        assertTrue(error is AgentException.Unauthorized)
        assertEquals(401, (error as AgentException.Unauthorized).code)
        assertEquals("API Key 无效或缺失，请在服务器设置中检查", error.userMessage)
    }

    @Test
    fun `maps 4xx to client error with server message`() {
        val error = agentExceptionForHttp(400, """{"error":"message is required"}""")
        assertTrue(error is AgentException.ClientError)
        error as AgentException.ClientError
        assertEquals(400, error.code)
        assertEquals("message is required", error.serverMessage)
        assertEquals("请求失败（HTTP 400）：message is required", error.userMessage)
    }

    @Test
    fun `maps 5xx to server error with parsed body`() {
        val error = agentExceptionForHttp(502, """{"error":"LLM not configured"}""")
        assertTrue(error is AgentException.ServerError)
        error as AgentException.ServerError
        assertEquals(502, error.code)
        assertEquals("LLM not configured", error.serverMessage)
    }
}
