package dev.lorepia.tauri.platform

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class PlatformWorkQueuesTest {
    @Test(timeout = 10_000)
    fun blockedStagingWorkDoesNotDelayCredentialsAndDomainOrderIsPreserved() {
        val queues = PlatformWorkQueues()
        val stagingStarted = CountDownLatch(1)
        val releaseStaging = CountDownLatch(1)
        val stagingFinished = CountDownLatch(2)
        val credentialFinished = CountDownLatch(2)
        val completionOrder = CopyOnWriteArrayList<String>()

        try {
            queues.executeStaging {
                stagingStarted.countDown()
                releaseStaging.await()
                completionOrder.add("staging-first")
                stagingFinished.countDown()
            }
            assertTrue(stagingStarted.await(2, TimeUnit.SECONDS))

            queues.executeStaging {
                completionOrder.add("staging-second")
                stagingFinished.countDown()
            }
            queues.executeCredential {
                completionOrder.add("credential-first")
                credentialFinished.countDown()
            }
            queues.executeCredential {
                completionOrder.add("credential-second")
                credentialFinished.countDown()
            }

            assertTrue(credentialFinished.await(2, TimeUnit.SECONDS))
            assertEquals(
                listOf("credential-first", "credential-second"),
                completionOrder,
            )

            releaseStaging.countDown()
            assertTrue(stagingFinished.await(2, TimeUnit.SECONDS))
            assertEquals(
                listOf(
                    "credential-first",
                    "credential-second",
                    "staging-first",
                    "staging-second",
                ),
                completionOrder,
            )
        } finally {
            releaseStaging.countDown()
            queues.shutdownNow()
        }
    }

    @Test
    fun shutdownStopsBothWorkDomains() {
        val queues = PlatformWorkQueues()
        queues.shutdownNow()

        assertThrows(RejectedExecutionException::class.java) {
            queues.executeCredential {}
        }
        assertThrows(RejectedExecutionException::class.java) {
            queues.executeStaging {}
        }
    }
}
