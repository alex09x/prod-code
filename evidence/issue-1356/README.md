# Workspace resource retirement (#1356)

Current delivery evidence is in `parent-final/verification.json` and its full logs.
The earlier artifacts at this directory root and under `repair-4280/` are retained
historical states, not claims that their source hashes describe the final code.

Idle session and watcher entries are bounded; active and queued query owners remain
pinned. Root identity and process-unique watch tokens prevent stale observations
or old sync acknowledgments from transferring to a replacement. Retirement observes
weak references to exact watcher incarnations and drops resources after unlocking.
No blocking sleep remains in watcher acquisition.

The final Linux gate passed 16 session and 8 watcher tests at the inherited 1024
soft descriptor limit. No whole-target serialization or limit increase was used.
The socket check asserts zero surviving descriptors for the 20 retired fixture
clients; whole-process samples include unrelated fixtures and are not a zero-FD claim.

All intermediate failures are retained. One exposed a test setup race and was fixed.
Another observed unavailable native watchers; its underlying error was not captured.
Test-only setup diagnostics now report errors and local descriptor/thread counts.
Later diagnostics and the exact final gate passed, but the earlier availability
failure's cause remains unproven. No shared service was restarted or reconfigured.
This gate does not replace the separately pending constructor acceptance.
