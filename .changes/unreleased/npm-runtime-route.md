---
category: Changed
---
- The npm-published `dsh` CLI (`@deepseek-ai/dsh`) is now documented as a runtime-acquisition route: install the pinned `npm install -g @deepseek-ai/dsh@0.1.3-alpha.2` and point `DSH_RUNTIME_BIN` at it; the bin is a Node.js script, so Node.js must be on `PATH`.
- The `RuntimeNotFound` hint now names the npm route alongside the bring-your-own and build-from-source routes.
