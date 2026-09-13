---
category: Fixed
---
- `Session::run` no longer wedges indefinitely when the runtime dies mid-turn (stdout EOF after `session/prompt` succeeds but before the inbox receipt or root-idle notification arrives). The high-level API surfaces `Error::TransportClosed` with the exit code and stderr tail, as its doc contract always promised ("once the channel (or the client) is closed, `Error::TransportClosed` is returned").
- `NotificationSubscription::recv` parked in `broadcast::Receiver::recv().await` now wakes with `TransportClosed` when the runtime dies spontaneously (stdout EOF without an explicit `close`), instead of hanging forever. The fix shares the original broadcast `Sender` between `HarnessClient` and the read loop and drops it on the read loop's EOF path, so the channel closes and a parked `recv` resolves with `RecvError::Closed`. Subscriptions created after runtime death remain born-failed.
