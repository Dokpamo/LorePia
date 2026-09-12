# Native lifecycle inspection

Task DEEP-EFFICIENCY-20260912; read-only slice on the existing dirty worktree
`/Users/codexer/.codex/worktrees/lorepia-ux-20260912`, baseline
`aac3b16e0697c710b1209f498caba71821746140` and the deep-efficiency snapshot.
Root, frontend and Core AGENTS and lifecycle ownership guidance were read.
No code edits, symbol movement or tests were planned until a candidate was
validated and file ownership agreed. Priority changed to portable history
compatibility before exhaustive lifecycle investigation completed.

Inspected native chat stream/runtime generation registries, supervisors in
state.rs, active discovery request registry and Core runtime control. Chat
subscriptions are bounded at 32 with Drop removal; runtime registrations and
cancellation tombstones are bounded at 16 with terminal acknowledgment and
Drop removal. Discovery request registrations remove their exact request ID
on Drop. These are not evidence of permanent map leaks.

Native memory/interaction/lifecycle supervisors each retain a single shutdown
watch subscription; startup guards avoid duplicate loops. They poll every
500 ms, which is ongoing idle work, but changing to event-driven wakeups would
require verifying durable-job wake sources and is not an authorized quick
lifecycle fix. Headless lifecycle startup explicitly avoids a detached Core
owner. Core runtime uses two workers and bounded runtime shutdown.

No confirmed actionable leak found in this bounded reading. This is not a
complete shutdown/long-session memory proof; no benchmark or tests were run.
Existing asset CRC changes remain separate and untouched.
