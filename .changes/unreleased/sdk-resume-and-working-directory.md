---
category: Added
---
- `DeepSeekHarness::resume_session` restores a persisted session and returns a handle bound to its id: the stored identity and directory are kept, an interrupted final turn is repaired without sending input, and no replacement session is created. A missing, unreadable, or already-open/opening session is an error.
- `Session::get_working_directory` returns the session's effective working directory as a validated absolute path. An unknown session id creates the session, and a missing directory is recovered to its origin.
- `Session::set_working_directory` changes the session's working directory and returns the validated absolute path; a relative path resolves against the session's current directory, and the session origin and permissions are unchanged.
- `HarnessClient` carries the matching `resume_session`, `get_working_directory`, and `set_working_directory` methods for direct wire-level use.
