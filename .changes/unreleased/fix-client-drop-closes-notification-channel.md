---
category: Fixed
---
- A `HarnessClient` dropped without an explicit `close()` now closes its notification channel, so a subscription awaiting the next notification returns `Error::TransportClosed` promptly instead of only when the aborted background reader task happens to be torn down. That error reports the closed reason, plus the runtime's exit code and captured stderr tail when they were observed.
