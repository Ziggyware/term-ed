| # | Feature | Why | First milestone |
|---|---|---|---|
| 1 | Language-server integration | Practical IDE support | Per-language LSP config, diagnostics, go-to-def/rename |
| 2 | AI change-review queue | Granular diff review/rollback | Accept/reject hunks, revert entire AI task |
| 3 | Workspace trust & plugin capabilities | Plugins have full FS/process access | Untrusted mode, declared permissions, sandbox later |
| 4 | Background, cancellable search | Large trees stall UI | Debounced workers, cancel stale, streaming results |
| 5 | Persistent AI conversations | Don't lose context on close | Local history, named sessions, resume/export |
| 6 | Integrated terminal & task runner | Build/test without leaving editor | PTY split terminal, named tasks, clickable errors |
| 7 | Git source-control panel | Plugin only shows status | Changed files, side-by-side diff, stage/unstage, commit |
| 8 | User-configurable MCP connections | Bridge doesn't manage external servers | Add/remove stdio/remote MCP, env credentials, allowlists |
| 9 | Debug-adapter support | Breakpoints for bugs static analysis misses | DAP launch configs, breakpoints, stack frames, locals |
| 10 | Custom keymaps & session restore | Terminal/mobile need different shortcuts | Editable bindings, conflict detection, restore tabs/splits |