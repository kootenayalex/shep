package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TaskParseTest {
    @Test
    fun `parses the exact assigned agent pane`() {
        val tasks = parseTasks(
            JSONObject(
                """{"tasks":[
                    {"id":7,"prompt":"run tests","repo":"/repo","runtime":"claude",
                     "use_worktree":false,"state":"running","workspace_id":"w1",
                     "assigned_pane_id":"w1:p2"}
                ]}"""
            )
        )

        assertEquals("w1:p2", tasks.single().assignedPaneId)
    }

    @Test
    fun `old task rows have no assigned pane`() {
        val tasks = parseTasks(
            JSONObject("""{"tasks":[{"id":1,"prompt":"old","repo":"/repo"}]}""")
        )

        assertNull(tasks.single().assignedPaneId)
    }
}
