---
category: Fixed
---
- A `HarnessClient` dropped without an explicit `close()` now closes its notification channel, so a subscription awaiting the next notification returns `Error::TransportClosed` promptly instead of only when the aborted background reader task happens to be torn down (the error reports whatever diagnostics had been observed by then).
- The parked-recv-on-spontaneous-death regression tests no longer report a false regression on a healthy build: the runtime exit code on that path is best-effort (the process may already have been reaped), so they assert only the diagnostics the path always produces — the closed reason and the captured stderr tail — and additionally cover a subscription created after the death and the second `close()` being a no-op.
