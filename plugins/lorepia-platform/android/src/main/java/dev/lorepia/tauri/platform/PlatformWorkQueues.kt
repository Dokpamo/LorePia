package dev.lorepia.tauri.platform

import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

internal class PlatformWorkQueues(
    private val credentialExecutor: ExecutorService = Executors.newSingleThreadExecutor(),
    private val stagingExecutor: ExecutorService = Executors.newSingleThreadExecutor(),
) {
    fun executeCredential(task: Runnable) {
        credentialExecutor.execute(task)
    }

    fun executeStaging(task: Runnable) {
        stagingExecutor.execute(task)
    }

    fun shutdownNow() {
        credentialExecutor.shutdownNow()
        stagingExecutor.shutdownNow()
    }
}
