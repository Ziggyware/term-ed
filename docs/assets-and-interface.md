# Images, ZIPs & Studio Theme

## Images

| Control | Action |
|---|---|
| `+`/`=` / `-` | Zoom in / out |
| Mouse wheel | Zoom in / out |
| Arrow keys | Pan zoomed image |
| `0` / `Home` | Fit & reset pan |
| `1` | Actual pixels (1:1) |
| **Fit**, **1:1**, **−**, **+** | Click/tap viewer controls |
| **Export…** / `Ctrl+Alt+S` | Export original image to file |

## ZIP Files

### Browse as folder

1. Select `.zip` in Explorer (archive icon + **ZIP** badge)
2. `Enter`/`Right`/click to expand; `Left` to collapse
3. Open members like normal files

### Safety & boundaries

| Limit | Behavior |
|---|---|
| Container size | ≤ 256 MiB |
| Entry count | ≤ 10,000 |
| Expanded member | ≤ 32 MiB per opened/written member |
| Unsafe paths | Traversal, absolute/drive paths, backslashes, control chars, conflicts refused |
| ZIP symlinks | Never followed; archives with them cannot be rewritten |
| Encryption | No decryption; encrypted members → cannot rewrite |
| Compression | Stored & Deflate only; unsupported → error |
| Nested archives | ZIPs in ZIPs not recursively mounted |
| Editing scope | Content editing only — not lossless metadata editor |
