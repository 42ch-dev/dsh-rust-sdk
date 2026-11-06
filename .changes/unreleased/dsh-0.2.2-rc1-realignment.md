---
category: Changed
---

- The verified runtime baseline moved to `@deepseek-ai/dsh@0.2.2-rc.1`, and both frozen contracts — the wire-parity surface and the launch contract — were re-baselined against it. The wire delta is additive: upstream now declares eight request methods while this crate still sends the same six, and `session/query`, `approval/request`, and `user-question/ask` stay deliberately unwrapped, so a leading `/` in run input is still sent to the model as prompt text rather than executed as a native command.
- Two documented facts were corrected, with no public-API change: upstream declares six server-to-client notifications while the crate still models four, `subagent.started` is emitted only for subagent-origin sessions (an ordinary fork therefore adds no tree edge), and a resumed session takes the initialized provider/model route instead of its stored selection, where a stored reasoning effort survives only when both match. The two added notification names reach the crate only through its unknown-method path.
- The runtime's install floor is documented as Node `>= 24.9.0`. The crate still installs, downloads, and versions nothing, so a caller below that bound sees the runtime's own engine failure rather than a crate error.
