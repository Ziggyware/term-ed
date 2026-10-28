# Images, ZIP workspaces and the Studio interface

![A PNG inside an expanded ZIP, displayed in the Termed Studio theme](studio-assets.png)

*Actual headless application frame at 140 × 42 terminal cells, not a design mockup. The sample landscape is a generated test illustration; it demonstrates the portable half-block renderer, including transparent corners.*

## Images

Existing PNG, JPEG, GIF, WebP, BMP and ICO files open in a **read-only image canvas**, including image members inside ZIP files. The loader detects image signatures as well as extensions; binary image bytes are no longer converted into lossy text.

- Aspect-preserving fit, centered image, transparency checkerboard and format/dimension/file-size metadata.
- JPEG EXIF orientation is applied to the preview without changing the original bytes.
- Viewport-sized rendering cache: zooming does not allocate an enormous zoomed bitmap.
- Text editing, replacement and editor plugins cannot modify the preview. **Save As** exports the original encoded bytes, not a re-encoded approximation or the empty text buffer.

| Control | Action |
| --- | --- |
| `+` or `=` / `-` | Zoom in / out |
| Mouse wheel over the viewer | Zoom in / out |
| Arrow keys | Pan a zoomed image |
| `0` or `Home` | Fit and reset pan |
| **Fit**, **−**, **+** | Click/tap viewer controls |
| **Export…** or `Ctrl+Alt+S` | Export the original image to a regular file |

The command palette also includes **Image: Zoom In**, **Image: Zoom Out** and **Image: Fit to View**. Select/open the same image again to refresh a cached preview after an external change; merely switching tabs preserves its current view.

### Format and display limits

- The canvas uses Unicode half-blocks and terminal colors. It works without Kitty or Sixel support, but its resolution is limited by the terminal grid. Truecolor gives the best result; `TERMED_TRUECOLOR=1` can force it when detection misses your terminal.
- Animation is not played; GIF/animated image previews display a static frame.
- SVG remains editable XML source. TIFF, AVIF and other image codecs are not enabled; unsupported or corrupt images report an error instead of opening as garbage text.
- Input is capped at **32 MiB**, decoded dimensions at **16 megapixels**, and the image decoder's allocation budget at **128 MiB**. This is a decoder budget, not a process-wide memory limit. Ordinary text opens also have a 32 MiB input budget and require valid UTF-8.

## ZIP files: folder view and file view

ZIPs are virtual folders, **not extracted directories**. An actual filesystem directory named `something.zip` still behaves as an ordinary directory.

### Browse as a folder

1. Select a `.zip` in Explorer; its row has an archive icon and **ZIP** badge.
2. Press `Enter` or `Right`, or click/tap it, to expand it. `Left` collapses it or selects the parent.
3. Open a member just like a normal file. Intermediate folders appear even if the ZIP omitted explicit directory entries.

Virtual paths use the form:

```text
release.zip/src/main.rs
release.zip/assets/dusk.png
```

Text members support syntax highlighting, normal editing and undo/redo. Image members use the same image canvas as regular image files. Virtual paths can also be passed to the CLI:

```sh
termed -t "Termed Studio" release.zip/src/main.rs
```

### Open as a file

Select the ZIP row and press **Shift+Enter**, or run **Archive: Open Selected ZIP as File**. This opens a **read-only archive overview** with member count, expanded sizes and a listing of the first 200 entries. Explorer remains available for the full directory.

**Save As** on the overview copies the original ZIP bytes, **not the text listing**. Reopen the overview with Shift+Enter to refresh it after the archive changes. A stale overview cannot export an archive version it has not inspected. **Explorer: Refresh** clears cached folder listings after external changes.

### Edit, save and export members

- `Ctrl+S` on a text member builds a temporary sibling ZIP, raw-copies untouched compressed members, writes the changed member, flushes the temporary file and replaces the original by rename. Failed saves keep the edited buffer dirty and clean up the temporary file.
- Archive-level comments, ordinary member Unix permissions and untouched member data are retained. CRLF text members retain CRLF on disk.
- Separate tabs can save different members: saves rebase on the current archive. If the **same member's CRC changes**, saving is refused and your edits are kept. Export them with Save As or reopen the member after closing its edited buffer. CRC/metadata checks detect ordinary conflicts; they are not a cryptographic integrity guarantee or a cross-process lock.
- **Save As** from a text member writes a regular text file. To create another member, use a destination such as `release.zip/new/folder/notes.txt`; its container ZIP must already exist. Save As changes a text buffer's path only if saving succeeds.
- **Save As** from an image member exports its original image bytes to a regular file. Exporting images into ZIPs is not implemented.
- Export refuses the source file/container itself, including symlink aliases and, on Unix, hard-link aliases. Text-only save and agent write APIs also reject image/ZIP signatures rather than overwriting them as text. A missing container is never silently recreated as a directory.

### Archive safety and boundaries

| Limit or boundary | Behavior |
| --- | --- |
| Container size | At most 256 MiB, including the rewritten output before replacement |
| Entry count | At most 10,000; ordinary and ZIP64 footer counts checked before indexing |
| Expanded member | At most 32 MiB per opened/written member |
| Unsafe paths | Traversal, absolute/drive paths, backslashes, control characters and file/folder conflicts are refused |
| ZIP symlinks | Never followed; archives containing them cannot be rewritten |
| Encryption | No password/decryption support; archives containing encrypted members cannot be rewritten |
| Compression | Stored and Deflate members can be opened; unsupported compression reports an error |
| Nested archives | ZIPs inside ZIPs are not recursively mounted |
| Archive editing scope | Content editing, not a lossless ZIP-metadata editor: per-member comments/extra fields and signatures are not guaranteed to survive a rewrite. Keep a backup or export the member when that metadata matters. Do not use this to maintain signed/package archives. |

Quick Open and disk-based project search still enumerate the real filesystem, not every member in every archive. **Already-open text members** participate in Find in Files through their live editor buffers, including unsaved changes; image previews and archive overview listings do not become fake text search results. External commands and plugins that open paths themselves are not automatically ZIP-aware. Editor-buffer plugins can work on an open text member; binary previews cannot be editor-plugin targets.

These protections are editor safeguards, not an OS sandbox for trusted plugins, external agents or approved commands.

## Professional UI/UX direction

The goal is a focused terminal workbench, not a web-style skin pasted over the existing keyboard workflow. The opt-in **Termed Studio** theme adds navy surface layers, a restrained cool accent, stronger secondary text and clearer selected/active surfaces. Existing themes and the default theme remain unchanged.

### Research and application

| Reference principle | Applied here |
| --- | --- |
| VS Code recommends contextually appropriate editor actions and secondary overflow, rather than a noisy permanent toolbar. [3](https://code.visualstudio.com/api/ux-guidelines/editor-actions) | Fit/zoom/export controls live inside the image viewer. Archive-as-file is a named palette command/Explorer shortcut, not another global toolbar icon. |
| Zed exposes breadcrumbs and quick actions as configurable toolbar elements. [18](https://zed.dev/docs/reference/all-settings) | Preserve the existing tabs, breadcrumbs and palette. A virtual ZIP path remains visible in breadcrumbs, while a ZIP badge and read-only/member labels explain what kind of resource is open. Compact layouts retain their existing reduced chrome. |
| VS Code's accessibility guidance calls for keyboard alternatives and at least 4.5:1 contrast outside the editor. [1](https://github.com/microsoft/vscode/wiki/Accessibility-Guidelines) | Every image action has a keyboard/palette alternative. Automated checks enforce at least 4.5:1 for Studio's tested main, secondary, selected, input, popup and status text pairs in truecolor. This is not a claim of full application accessibility certification. |
| VS Code's theme model separates background, foreground, selection and active/contrast-border roles. [22](https://code.visualstudio.com/api/references/theme-color) | Reuse the editor's semantic palette instead of hardcoding canvas colors. Image metadata, transparency stage and controls adapt to other themes too. |

### Try Studio

```sh
termed -t "Termed Studio" .
termed -t "Termed Studio" pictures/example.png
```

Or choose **Termed Studio** in the theme picker through Settings or **Preferences: Color Theme**. Theme selection through the UI persists with the existing settings system.

Further polish worth pursuing separately: optional native Kitty/Sixel graphics for sharper images, asynchronous archive/search work for very large projects, and a dedicated high-contrast/focus-navigation audit. These are recommendations, not implemented features.

## Verification

`tests/assets.rs` covers real image codecs, orientation, original-byte exports, resize/zoom/pan controls, corrupt files, resource limits, ZIP traversal/CRC/conflict safety, folder and file views, member saves/undo, aliases, read-only mutation guards, live search and Studio contrast. The automation and built-in agent unit tests also exercise binary-preview protections.

The combined **121-test suite passed** against the real application/library sources in the separate offline harness, and the all-targets check passed in that harness. A pseudo-terminal smoke run opened a ZIP-member image and exited cleanly after zoom/pan/fit input. The headless snapshot example rendered the frame above. Desktop/phone image layouts are exercised at 140 × 42, 80 × 24, 46 × 30 and 28 × 18 cells.

The offline harness uses alternate GitHub-sourced dependency versions and reduced features; it is **not** a locked-dependency or HTTPS-transport verification. The production manifest/lockfile remain registry-based. Normal `cargo test --locked` and `cargo clippy --all-targets --locked` still need registry access. Native-terminal graphics behavior has not been manually verified on each supported terminal/platform.
