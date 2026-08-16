package dev.lorepia.tauri.platform

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import kotlin.io.path.createTempDirectory
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class BoundCredentialFilePublisherTest {
    @Test
    fun firstBoundDirectoryCreationSyncsDirectoryThenParent() {
        val root = createTempDirectory("lorepia-bound-directory").toFile()
        try {
            val directory = root.resolve("provider-credentials")
            val events = mutableListOf<String>()

            BoundCredentialDirectory.ensureDurable(
                directory = directory,
                createDirectory = {
                    events += "create"
                    Files.createDirectory(it.toPath())
                },
                syncDirectory = {
                    events += if (it == directory) "sync-directory" else "sync-parent"
                },
            )

            assertEquals(listOf("create", "sync-directory", "sync-parent"), events)
            assertTrue(directory.isDirectory)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun firstBoundDirectorySyncFailureKeepsRecoveryEvidence() {
        val root = createTempDirectory("lorepia-bound-directory-failure").toFile()
        try {
            val directory = root.resolve("provider-credentials")

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialDirectory.ensureDurable(
                    directory = directory,
                    createDirectory = {
                        Files.createDirectory(it.toPath())
                    },
                    syncDirectory = { throw IOException("synthetic directory sync failure") },
                )
            }

            assertTrue(directory.isDirectory)
            assertTrue(directory.listFiles()?.isEmpty() == true)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun missingBaseNeverRecoversAtomicFileSidecars() {
        withTemporaryTarget { target ->
            val backup = File("${target.path}.bak")
            val pending = File("${target.path}.new")
            val backupBytes = "stale-backup".toByteArray()
            val pendingBytes = "stale-new-write".toByteArray()
            backup.writeBytes(backupBytes)
            pending.writeBytes(pendingBytes)

            val observed = BoundCredentialFileReader.readBaseOnly(
                target = target,
                maximumBytes = 1024,
            )

            assertEquals(null, observed)
            assertTrue(!Files.exists(target.toPath(), LinkOption.NOFOLLOW_LINKS))
            assertArrayEquals(backupBytes, backup.readBytes())
            assertArrayEquals(pendingBytes, pending.readBytes())
        }
    }

    @Test
    fun sidecarAmbiguityPreservesBaseAndEverySidecarExactly() {
        withTemporaryTarget { target ->
            val backup = File("${target.path}.bak")
            val pending = File("${target.path}.new")
            val winnerBytes = "current-base-winner".toByteArray()
            val backupBytes = "stale-backup-winner".toByteArray()
            val pendingBytes = "stale-new-winner".toByteArray()
            target.writeBytes(winnerBytes)
            backup.writeBytes(backupBytes)
            pending.writeBytes(pendingBytes)

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileReader.readBaseOnly(
                    target = target,
                    maximumBytes = 1024,
                )
            }

            assertArrayEquals(winnerBytes, target.readBytes())
            assertArrayEquals(backupBytes, backup.readBytes())
            assertArrayEquals(pendingBytes, pending.readBytes())
        }
    }

    @Test
    fun staleBackupMakesPostPublishVerificationFailWithoutRollback() {
        withTemporaryTarget { target ->
            val backup = File("${target.path}.bak")
            val attempted = "newly-published-bound-record".toByteArray()
            val backupBytes = "stale-generic-atomic-backup".toByteArray()
            backup.writeBytes(backupBytes)

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = attempted,
                    syncDirectory = {},
                    verify = {
                        BoundCredentialFileReader.readBaseOnly(
                            target = target,
                            maximumBytes = 1024,
                        )?.contentEquals(attempted) == true
                    },
                )
            }

            assertArrayEquals(attempted, target.readBytes())
            assertArrayEquals(backupBytes, backup.readBytes())
        }
    }

    @Test
    fun freshBaseIsReadWithoutMutation() {
        withTemporaryTarget { target ->
            val winnerBytes = "current-base-winner".toByteArray()
            target.writeBytes(winnerBytes)

            val observed = BoundCredentialFileReader.readBaseOnly(
                target = target,
                maximumBytes = 1024,
            )

            assertArrayEquals(winnerBytes, observed)
            observed?.fill(0)
            assertArrayEquals(winnerBytes, target.readBytes())
        }
    }

    @Test
    fun existingRaceWinnerIsNeverOverwrittenOrDeleted() {
        withTemporaryTarget { target ->
            val winner = "competing-bound-envelope".toByteArray()
            val attempted = "attempted-bound-envelope".toByteArray()
            target.writeBytes(winner)

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = attempted,
                    syncDirectory = { error("existing target must fail before directory sync") },
                    verify = { error("existing target must fail before verification") },
                )
            }

            assertArrayEquals(winner, target.readBytes())
        }
    }

    @Test
    fun partialExistingTargetRemainsExactRecoveryEvidence() {
        withTemporaryTarget { target ->
            val partial = byteArrayOf(0x01, 0x02, 0x03)
            target.writeBytes(partial)

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = "complete-envelope".toByteArray(),
                    syncDirectory = {},
                    verify = { true },
                )
            }

            assertArrayEquals(partial, target.readBytes())
        }
    }

    @Test
    fun existingSymlinkIsNeverFollowedOrReplaced() {
        val root = createTempDirectory("lorepia-bound-symlink").toFile()
        try {
            val winner = root.resolve("winner")
            val target = root.resolve("synthetic-bound.credential")
            val winnerBytes = "symlink-winner".toByteArray()
            winner.writeBytes(winnerBytes)
            Files.createSymbolicLink(target.toPath(), winner.toPath())

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = "attempted-bound-envelope".toByteArray(),
                    syncDirectory = { error("symlink target must fail before directory sync") },
                    verify = { error("symlink target must fail before verification") },
                )
            }

            assertTrue(Files.isSymbolicLink(target.toPath()))
            assertArrayEquals(winnerBytes, winner.readBytes())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun postPublishVerificationMismatchNeverRollsBackPossibleWinner() {
        withTemporaryTarget { target ->
            val attempted = "attempted-bound-envelope".toByteArray()
            val winner = "post-publish-race-winner".toByteArray()

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = attempted,
                    syncDirectory = {},
                    verify = {
                        target.writeBytes(winner)
                        false
                    },
                )
            }

            assertTrue(target.isFile)
            assertArrayEquals(winner, target.readBytes())
        }
    }

    @Test
    fun newTargetIsForcedSyncedAndVerifiedExactlyOnce() {
        withTemporaryTarget { target ->
            val encoded = "new-bound-envelope".toByteArray()
            var directorySyncs = 0
            var verifications = 0

            BoundCredentialFilePublisher.publishAddOnly(
                target = target,
                encoded = encoded,
                syncDirectory = { directory ->
                    assertEquals(target.parentFile, directory)
                    directorySyncs += 1
                },
                verify = {
                    verifications += 1
                    target.readBytes().contentEquals(encoded)
                },
            )

            assertArrayEquals(encoded, target.readBytes())
            assertEquals(1, directorySyncs)
            assertEquals(1, verifications)
        }
    }

    @Test
    fun parentDirectorySyncFailureKeepsPublishedBytesForRecovery() {
        withTemporaryTarget { target ->
            val encoded = "published-before-directory-sync-failure".toByteArray()

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFilePublisher.publishAddOnly(
                    target = target,
                    encoded = encoded,
                    syncDirectory = { throw IOException("synthetic directory sync failure") },
                    verify = { error("failed directory sync must stop before verification") },
                )
            }

            assertTrue(target.isFile)
            assertArrayEquals(encoded, target.readBytes())
        }
    }

    @Test
    fun boundDeleteMissingCredentialDirectorySyncsGrandparentWithoutCreatingAnything() {
        val root = createTempDirectory("lorepia-bound-delete-missing-directory").toFile()
        try {
            val directory = root.resolve("provider-credentials")
            val target = directory.resolve("synthetic-bound.credential")
            val syncedDirectories = mutableListOf<File>()
            var unlinks = 0

            BoundCredentialFileDeleter.deleteBaseOnly(
                target = target,
                unlink = {
                    unlinks += 1
                    true
                },
                syncDirectory = { syncedDirectories += it },
            )

            assertEquals(0, unlinks)
            assertEquals(listOf(root), syncedDirectories)
            assertTrue(!Files.exists(directory.toPath(), LinkOption.NOFOLLOW_LINKS))
            assertTrue(!Files.exists(target.toPath(), LinkOption.NOFOLLOW_LINKS))
            assertTrue(!Files.exists(File("${target.path}.bak").toPath(), LinkOption.NOFOLLOW_LINKS))
            assertTrue(!Files.exists(File("${target.path}.new").toPath(), LinkOption.NOFOLLOW_LINKS))
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun boundDeleteMissingCredentialDirectorySyncFailureStaysRecoveryRequired() {
        val root = createTempDirectory("lorepia-bound-delete-missing-directory-sync").toFile()
        try {
            val directory = root.resolve("provider-credentials")
            val target = directory.resolve("synthetic-bound.credential")
            var directorySyncs = 0
            var unlinks = 0

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = {
                        unlinks += 1
                        true
                    },
                    syncDirectory = {
                        directorySyncs += 1
                        throw IOException("synthetic grandparent sync failure")
                    },
                )
            }

            assertEquals(0, unlinks)
            assertEquals(1, directorySyncs)
            assertTrue(!Files.exists(directory.toPath(), LinkOption.NOFOLLOW_LINKS))
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun boundDeleteMissingCredentialDirectoryRacePreservesEveryNewWinner() {
        val root = createTempDirectory("lorepia-bound-delete-missing-directory-race").toFile()
        try {
            val directory = root.resolve("provider-credentials")
            val target = directory.resolve("synthetic-bound.credential")
            val winner = "new-bound-winner".toByteArray()
            val sidecarWinner = "new-sidecar-winner".toByteArray()
            var unlinks = 0

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = {
                        unlinks += 1
                        true
                    },
                    syncDirectory = {
                        assertEquals(root, it)
                        Files.createDirectory(directory.toPath())
                        target.writeBytes(winner)
                        File("${target.path}.new").writeBytes(sidecarWinner)
                    },
                )
            }

            assertEquals(0, unlinks)
            assertArrayEquals(winner, target.readBytes())
            assertArrayEquals(sidecarWinner, File("${target.path}.new").readBytes())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun boundDeleteRejectsSidecarAmbiguityBeforeAnyNativeMutation() {
        withTemporaryTarget { target ->
            val backup = File("${target.path}.bak")
            val pending = File("${target.path}.new")
            val winnerBytes = "bound-delete-winner".toByteArray()
            val backupBytes = "bound-delete-backup".toByteArray()
            val pendingBytes = "bound-delete-new".toByteArray()
            target.writeBytes(winnerBytes)
            backup.writeBytes(backupBytes)
            pending.writeBytes(pendingBytes)
            var unlinks = 0
            var directorySyncs = 0

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = {
                        unlinks += 1
                        true
                    },
                    syncDirectory = { directorySyncs += 1 },
                )
            }

            assertEquals(0, unlinks)
            assertEquals(0, directorySyncs)
            assertArrayEquals(winnerBytes, target.readBytes())
            assertArrayEquals(backupBytes, backup.readBytes())
            assertArrayEquals(pendingBytes, pending.readBytes())
        }
    }

    @Test
    fun boundDeleteFailurePreservesExactBase() {
        withTemporaryTarget { target ->
            val winnerBytes = "bound-delete-winner".toByteArray()
            target.writeBytes(winnerBytes)

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = { throw IOException("synthetic unlink failure") },
                    syncDirectory = { error("failed unlink must stop before directory sync") },
                )
            }

            assertArrayEquals(winnerBytes, target.readBytes())
        }
    }

    @Test
    fun boundDeleteDirectorySyncFailurePreservesCompetingWinner() {
        withTemporaryTarget { target ->
            target.writeBytes("deleted-bound-record".toByteArray())
            val competingWinner = "post-unlink-winner".toByteArray()

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = { Files.deleteIfExists(it.toPath()) },
                    syncDirectory = {
                        target.writeBytes(competingWinner)
                        throw IOException("synthetic directory sync failure")
                    },
                )
            }

            assertArrayEquals(competingWinner, target.readBytes())
        }
    }

    @Test
    fun boundDeletePostconditionPreservesCompetingBaseAndSidecar() {
        withTemporaryTarget { target ->
            target.writeBytes("deleted-bound-record".toByteArray())
            val competingWinner = "post-unlink-winner".toByteArray()
            val competingSidecar = "post-unlink-sidecar".toByteArray()

            assertThrows(CredentialRecoveryRequiredException::class.java) {
                BoundCredentialFileDeleter.deleteBaseOnly(
                    target = target,
                    unlink = { Files.deleteIfExists(it.toPath()) },
                    syncDirectory = {
                        target.writeBytes(competingWinner)
                        File("${target.path}.new").writeBytes(competingSidecar)
                    },
                )
            }

            assertArrayEquals(competingWinner, target.readBytes())
            assertArrayEquals(competingSidecar, File("${target.path}.new").readBytes())
        }
    }

    @Test
    fun boundDeleteUnlinksFinalBaseAndSyncsParentExactlyOnce() {
        withTemporaryTarget { target ->
            target.writeBytes("deleted-bound-record".toByteArray())
            var directorySyncs = 0

            BoundCredentialFileDeleter.deleteBaseOnly(
                target = target,
                syncDirectory = { directory ->
                    assertEquals(target.parentFile, directory)
                    directorySyncs += 1
                },
            )

            assertTrue(!Files.exists(target.toPath(), LinkOption.NOFOLLOW_LINKS))
            assertEquals(1, directorySyncs)
        }
    }

    private fun withTemporaryTarget(test: (File) -> Unit) {
        val root = createTempDirectory("lorepia-bound-publisher").toFile()
        try {
            test(root.resolve("synthetic-bound.credential"))
        } finally {
            root.deleteRecursively()
        }
    }
}
