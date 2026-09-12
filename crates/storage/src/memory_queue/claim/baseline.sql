SELECT job.id
             FROM memory_jobs AS job
             LEFT JOIN task_profile_revisions AS task
               ON task.revision_id = job.task_profile_revision_id
             WHERE job.state = 'queued'
               AND job.attempts < 32
               AND julianday(job.available_at) <= julianday(?1)
               AND (
                   (
                       job.task_profile_revision_id IS NULL
                       AND (
                           SELECT COUNT(*)
                           FROM memory_jobs AS running
                           WHERE running.state = 'running'
                             AND running.task_profile_revision_id IS NULL
                       ) < 1
                   )
                   OR (
                       job.task_profile_revision_id IS NOT NULL
                       AND task.revision_id IS NOT NULL
                       AND (
                           SELECT COUNT(*)
                           FROM memory_jobs AS running
                           WHERE running.state = 'running'
                             AND running.task_profile_revision_id =
                                 job.task_profile_revision_id
                       ) < task.concurrency_limit
                       AND (
                           (
                               SELECT COUNT(*)
                               FROM memory_jobs AS recent
                               JOIN json_each(
                                   recent.payload_json,
                                   '$.attempt_started_at'
                               ) AS attempt
                               WHERE recent.task_profile_revision_id =
                                     job.task_profile_revision_id
                                 AND json_type(
                                     recent.payload_json,
                                     '$.attempt_started_at'
                                 ) = 'array'
                                 AND julianday(attempt.value) > julianday(?1)
                                     - (task.rate_limit_per_seconds / 86400.0)
                           )
                           + (
                               SELECT COUNT(*)
                               FROM memory_jobs AS recent
                               WHERE recent.task_profile_revision_id =
                                     job.task_profile_revision_id
                                 AND json_type(
                                     recent.payload_json,
                                     '$.attempt_started_at'
                                 ) IS NULL
                                 AND recent.started_at IS NOT NULL
                                 AND julianday(recent.started_at) > julianday(?1)
                                     - (task.rate_limit_per_seconds / 86400.0)
                           )
                       ) < task.rate_limit_requests
                   )
               )
             ORDER BY julianday(job.available_at), julianday(job.created_at), job.id
             LIMIT 1