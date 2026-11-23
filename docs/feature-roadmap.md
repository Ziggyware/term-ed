| # | Feature | Why | First milestone |
|---|---|---|---|
| 1 | Language-server integration | Completion, diagnostics, symbol navigation → practical IDE | Per-language LSP config, diagnostics list, go-to-def/rename with preview |
| 2 | AI change-review queue | Inline diffs need granular review/rollback | Accept/reject hunks, inspect all files in turn, revert entire AI task |
| 3 | Workspace trust & plugin capabilities | Executable plugins have full filesystem/process access | Untrusted-workspace mode, declared read/write/shell permissions; sandbox later |
| 4 | Background, cancellable workspace search | Large trees shouldn't stall typing/rendering | Debounced workers, cancel stale queries, streaming results, configurable limits |
| 5 | Persistent AI conversations | Don't lose investigations on editor close | Local chat history, named sessions, resume/export/delete, retention/redaction |
| 6 | Integrated terminal & task runner | Build/test without leaving editor | PTY-backed split terminal, named tasks, exit status, clickable errors |
| 7 | Git source-control panel | git-info plugin only reports status | Changed files, side-by-side diff, stage/unstage hunks, commit editing |
| 8 | User-configurable MCP connections | Internal bridge doesn't manage external tool servers | Add/remove stdio/remote MCP, env-ref credentials, tool allowlists, diagnostics |
| 9 | Debug-adapter support | Breakpoints/stepping for bugs static analysis can't catch | DAP launch configs, breakpoint gutter, stack frames, locals, debug console |
| 10 | Custom keymaps & session restore | Terminal/mobile users need different shortcuts, layout persistence | Editable keybindings with conflict detection, restore tabs/cursors/splits, portable profiles |
