# Ten additional feature proposals

These are **recommendations, not implemented features**. They are separate from
this change's search fixes, menus, settings, AI plugin tools, sub-agents and skills.

| # | Feature | Why it matters | First useful milestone |
| --- | --- | --- | --- |
| 1 | Language-server integration | Completion, diagnostics and symbol navigation turn a text editor into a practical IDE. | Per-language LSP configuration, diagnostics list, go-to-definition and rename with preview. |
| 2 | AI change-review queue | Inline diffs are helpful, but automatic edits need more granular review and rollback. | Accept/reject individual hunks, inspect all files in a turn, and revert an entire AI task atomically. |
| 3 | Workspace trust and plugin capabilities | Enabling an executable plugin currently trusts its local filesystem/process access. | Untrusted-workspace mode plus declared read/write/shell permissions; investigate an actual process sandbox separately. |
| 4 | Background, cancellable workspace search | Large trees should never stall typing or rendering. | Debounced worker searches, cancellation of stale queries, streaming results, progress and configurable limits. |
| 5 | Persistent AI conversations | Closing the editor should not lose a useful investigation or task plan. | Local chat history, named sessions, resume/export/delete and configurable retention/redaction. |
| 6 | Integrated terminal and task runner | Build/test workflows should not require leaving the editor. | PTY-backed split terminal, named workspace tasks, exit status and clickable compiler errors. |
| 7 | Git source-control panel | The git-info plugin reports status but does not provide a full review workflow. | Changed-file list, side-by-side diff, stage/unstage hunks and commit-message editing. |
| 8 | User-configurable MCP connections | The internal editor bridge does not manage arbitrary external tool servers. | Add/remove stdio or remote MCP servers, credentials by environment reference, tool allowlists and connection diagnostics. |
| 9 | Debug-adapter support | Breakpoints and stepping are essential for bugs that cannot be understood from static code alone. | DAP launch configurations, breakpoint gutter, stack frames, locals and a debug console. |
| 10 | Custom keymaps and workspace session restore | Terminal/mobile users need different shortcuts and expect their working layout back. | Editable keybindings with conflict detection, restore tabs/cursors/splits and portable named workspace profiles. |

Suggested order: trust/review and responsive search first; LSP and terminal/tasks
next; history, Git and workflow customization after those foundations. Add MCP
connection management and debugging once their permission and lifecycle models
are solid. Each milestone should include desktop and narrow-terminal tests.
