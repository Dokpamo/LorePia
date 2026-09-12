//! Evaluate durable limits once per distinct queued task revision.
pub(super) const SQL: &str = r"WITH requested_tasks AS MATERIALIZED (
    SELECT DISTINCT task_profile_revision_id
    FROM memory_jobs
    WHERE state = 'queued' AND attempts < 32
      AND julianday(available_at) <= julianday(?1)
      AND task_profile_revision_id IS NOT NULL
), eligible_tasks AS MATERIALIZED (
    SELECT task.revision_id
    FROM requested_tasks AS requested
    JOIN task_profile_revisions AS task
      ON task.revision_id = requested.task_profile_revision_id
    WHERE task.revision_id IS NOT NULL
    AND (
        SELECT COUNT(*)
        FROM memory_jobs AS running
        WHERE running.state = 'running'
          AND running.task_profile_revision_id =
              task.revision_id
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
                  task.revision_id
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
                  task.revision_id
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
SELECT job.id FROM memory_jobs AS job
WHERE job.state = 'queued' AND job.attempts < 32
  AND julianday(job.available_at) <= julianday(?1)
  AND (
    (job.task_profile_revision_id IS NULL AND (
      SELECT COUNT(*) FROM memory_jobs AS running
      WHERE running.state = 'running' AND running.task_profile_revision_id IS NULL
    ) < 1)
    OR job.task_profile_revision_id IN (SELECT revision_id FROM eligible_tasks)
  )
ORDER BY julianday(job.available_at), julianday(job.created_at), job.id
LIMIT 1";

#[cfg(test)]
mod tests;
