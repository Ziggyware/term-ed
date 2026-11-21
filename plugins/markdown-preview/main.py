#!/usr/bin/env python3
"""markdown-preview: render the current Markdown buffer as HTML.

Two commands:

* ``preview``          write HTML next to the buffer (or to a temp file when
                       the buffer is unsaved) and open it in the browser.
* ``preview-terminal`` print an ANSI-styled rendering into a terminal preview.

Both read the *live* buffer through ``editor.text``, so unsaved edits show up.
Standard library only: no Markdown package, no network.
"""

import html
import json
import os
import re
import sys
import tempfile
import webbrowser


class Host:
    def __init__(self):
        self.next_id = 1

    def send(self, message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    def call(self, method, params=None):
        rid = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        while True:
            line = sys.stdin.readline()
            if not line:
                raise SystemExit(0)
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("id") == rid and ("result" in msg or "error" in msg):
                return msg.get("result")

    def toast(self, message, kind="info"):
        self.send({"jsonrpc": "2.0", "method": "ui.toast", "params": {"message": message, "kind": kind}})


host = Host()


# ---------------------------------------------------------------- rendering

INLINE_CODE = re.compile(r"`([^`]+)`")
BOLD = re.compile(r"\*\*([^*]+)\*\*|__([^_]+)__")
ITALIC = re.compile(r"(?<!\*)\*([^*]+)\*(?!\*)|(?<!_)_([^_]+)_(?!_)")
LINK = re.compile(r"\[([^\]]*)\]\(([^)\s]+)\)")


def inline(text):
    """Escape, then apply inline Markdown. Code spans are protected first so
    their contents are never treated as emphasis."""
    spans = []

    def stash(m):
        spans.append(html.escape(m.group(1)))
        return "\x00%d\x00" % (len(spans) - 1)

    text = INLINE_CODE.sub(stash, text)
    text = html.escape(text, quote=False)
    text = BOLD.sub(lambda m: "<strong>%s</strong>" % (m.group(1) or m.group(2)), text)
    text = ITALIC.sub(lambda m: "<em>%s</em>" % (m.group(1) or m.group(2)), text)
    text = LINK.sub(
        lambda m: '<a href="%s">%s</a>' % (html.escape(m.group(2), quote=True), m.group(1)),
        text,
    )
    return re.sub(r"\x00(\d+)\x00", lambda m: "<code>%s</code>" % spans[int(m.group(1))], text)


def slug(text):
    keep = re.sub(r"[^a-z0-9 -]", "", text.lower()).strip().replace(" ", "-")
    return keep or "section"


def render(text):
    """Return HTML for the document. Deliberately small: headings, fenced and
    indented code, lists, quotes, rules, tables, links and emphasis."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    out = []
    para = []
    i = 0
    in_list = None  # "ul" or "ol"
    while i < len(lines):
        line = lines[i]

        fence = re.match(r"^\s*(```|~~~)\s*(\S*)\s*$", line)
        if fence:
            if in_list:
                out.append("</%s>" % in_list)
                in_list = None
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            marker, lang = fence.group(1), fence.group(2)
            code = []
            i += 1
            while i < len(lines) and not re.match(r"^\s*%s\s*$" % marker, lines[i]):
                code.append(lines[i])
                i += 1
            i += 1
            out.append(
                '<pre><code class="language-%s">%s</code></pre>'
                % (html.escape(lang, quote=True), html.escape("\n".join(code), quote=False))
            )
            continue

        if not line.strip():
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            if in_list:
                out.append("</%s>" % in_list)
                in_list = None
            i += 1
            continue

        heading = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
        if heading:
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            if in_list:
                out.append("</%s>" % in_list)
                in_list = None
            level = len(heading.group(1))
            title = heading.group(2)
            out.append(
                '<h%d id="%s">%s</h%d>' % (level, slug(title), inline(title), level)
            )
            i += 1
            continue

        # --- horizontal rule
        if re.match(r"^\s*([-*_])\s*(\1\s*){2,}$", line):
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            out.append("<hr>")
            i += 1
            continue

        # --- table: a header row followed by a separator row
        if (
            "|" in line
            and i + 1 < len(lines)
            and re.match(r"^\s*\|?\s*:?[-=]+:?\s*(\|\s*:?[-=]+:?\s*)+\|?\s*$", lines[i + 1])
        ):
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []

            def cells(row):
                row = row.strip()
                if row.startswith("|"):
                    row = row[1:]
                if row.endswith("|"):
                    row = row[:-1]
                return [c.strip() for c in row.split("|")]

            out.append("<table><thead><tr>")
            out.extend("<th>%s</th>" % inline(c) for c in cells(line))
            out.append("</tr></thead><tbody>")
            i += 2
            while i < len(lines) and "|" in lines[i] and lines[i].strip():
                out.append("<tr>")
                out.extend("<td>%s</td>" % inline(c) for c in cells(lines[i]))
                out.append("</tr>")
                i += 1
            out.append("</tbody></table>")
            continue

        # --- block quote (consecutive lines joined)
        if line.lstrip().startswith(">"):
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            quoted = []
            while i < len(lines) and lines[i].lstrip().startswith(">"):
                quoted.append(re.sub(r"^\s*>\s?", "", lines[i]))
                i += 1
            out.append("<blockquote><p>%s</p></blockquote>" % inline(" ".join(quoted)))
            continue

        # --- list item
        item = re.match(r"^\s*([-*+]|\d+[.)])\s+(.*)$", line)
        if item:
            if para:
                out.append("<p>%s</p>" % inline(" ".join(para)))
                para = []
            kind = "ol" if item.group(1)[0].isdigit() else "ul"
            if in_list != kind:
                if in_list:
                    out.append("</%s>" % in_list)
                out.append("<%s>" % kind)
                in_list = kind
            out.append("<li>%s</li>" % inline(item.group(2)))
            i += 1
            continue

        # --- indented code block
        if line.startswith("    ") and not para:
            block = []
            while i < len(lines) and (lines[i].startswith("    ") or not lines[i].strip()):
                block.append(lines[i][4:])
                i += 1
            out.append("<pre><code>%s</code></pre>" % html.escape("\n".join(block).rstrip(), quote=False))
            continue

        para.append(line.strip())
        i += 1

    if para:
        out.append("<p>%s</p>" % inline(" ".join(para)))
    if in_list:
        out.append("</%s>" % in_list)
    return "\n".join(out)


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>%s</title>
<style>
:root { color-scheme: light dark; }
body { margin: 0 auto; padding: 2rem 1.25rem 6rem; max-width: 52rem;
  font: 16px/1.65 -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background: #ffffff; color: #1f2328; }
@media (prefers-color-scheme: dark) {
  body { background: #0d1117; color: #e6edf3; }
  a { color: #4493f8; }
  code, pre { background: #161b22 !important; }
  th, td { border-color: #30363d !important; }
  blockquote { border-color: #30363d !important; color: #9198a1; }
  hr { border-color: #30363d !important; }
}
h1,h2 { border-bottom: 1px solid #d1d9e0; padding-bottom: .3em; }
h1,h2,h3,h4,h5,h6 { line-height: 1.25; margin: 1.6em 0 .6em; }
a { color: #0969da; }
code { background: #f0f1f3; padding: .18em .38em; border-radius: 4px;
  font: .875em/1.4 ui-monospace, "Cascadia Code", Consolas, monospace; }
pre { background: #f0f1f3; padding: .9rem 1rem; border-radius: 8px; overflow-x: auto; }
pre code { background: none; padding: 0; }
blockquote { margin: 1rem 0; padding: 0 1rem; border-left: .25em solid #d1d9e0; color: #59636e; }
table { border-collapse: collapse; width: 100%%; margin: 1rem 0; }
th, td { border: 1px solid #d1d9e0; padding: .45rem .7rem; text-align: left; }
th { font-weight: 600; }
img { max-width: 100%%; }
hr { border: 0; border-top: 1px solid #d1d9e0; margin: 2rem 0; }
</style></head><body>
%s
</body></html>
"""


def document_title(text, path):
    for line in text.split("\n"):
        heading = re.match(r"^#\s+(.*)", line)
        if heading:
            return heading.group(1).strip()
    return os.path.basename(path) if path else "Markdown preview"


def browser_preview(text, path):
    title = document_title(text, path)
    page = PAGE % (html.escape(title, quote=False), render(text))
    # A file next to the document keeps relative images and links working;
    # an unsaved buffer gets a temp file instead.
    if path:
        target = os.path.splitext(path)[0] + ".html"
    else:
        target = os.path.join(
            tempfile.mkdtemp(prefix="termed-md-"), (slug(title) or "preview") + ".html"
        )
    with open(target, "w", encoding="utf-8") as f:
        f.write(page)
    try:
        opened = webbrowser.open("file://" + os.path.abspath(target))
    except Exception:
        opened = False
    if opened:
        host.toast("Preview opened: %s" % target, "success")
    else:
        host.toast("Wrote %s but could not open a browser; open it manually" % target, "info")


# One alternation, applied in a single pass: styling in several passes would
# let a later rule match inside text an earlier rule just produced.
TERMINAL_INLINE = re.compile(
    r"`([^`]+)`"                        # 1: code
    r"|\*\*([^*]+)\*\*"                # 2: bold
    r"|__([^_]+)__"                       # 3: bold
    r"|\*([^*]+)\*"                      # 4: italic
    r"|\[([^\]]*)\]\(([^)\s]+)\)"        # 5: link text, 6: url
)


def style_inline(text):
    """Inline Markdown to plain text.

    The terminal preview is a read-only *buffer* in the editor, drawn with
    syntax highlighting — ANSI escapes would render as literal `^[[1m`
    junk, so styling is expressed with characters instead.
    """
    def repl(m):
        if m.group(1) is not None:            # `code`
            return m.group(1)
        if m.group(2) is not None:            # **bold**
            return m.group(2)
        if m.group(3) is not None:            # __bold__
            return m.group(3)
        if m.group(4) is not None:            # *italic*
            return m.group(4)
        body, url = m.group(5), m.group(6)    # [text](url)
        return body if url == body else "%s (%s)" % (body, url)

    return TERMINAL_INLINE.sub(repl, text)


def wrap_text(text, width):
    """Hard-wrap to `width` columns on word boundaries."""
    out = []
    for line in text.split("\n"):
        if len(line) <= width:
            out.append(line)
            continue
        current = ""
        for word in line.split(" "):
            while len(word) > width:
                if current:
                    out.append(current)
                    current = ""
                out.append(word[:width])
                word = word[width:]
            if not current:
                current = word
            elif len(current) + 1 + len(word) <= width:
                current += " " + word
            else:
                out.append(current)
                current = word
        out.append(current)
    return "\n".join(out)


def terminal_preview(text, width=96):
    out = []
    fenced = False
    for raw in text.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        if re.match(r"^\s*(```|~~~)", raw):
            fenced = not fenced
            out.append("─" * min(width, 40))
            continue
        if fenced:
            out.append("  │ " + raw)
            continue
        heading = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", raw)
        if heading:
            level = len(heading.group(1))
            title = heading.group(2)
            if level == 1:
                out.append(title)
                out.append("═" * min(len(title), width))
            elif level == 2:
                out.append(title)
                out.append("─" * min(len(title), width))
            else:
                out.append("▸ " + title)
            continue
        if re.match(r"^\s*([-*_])\s*(\1\s*){2,}$", raw):
            out.append("─" * min(width, 60))
            continue
        item = re.match(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$", raw)
        if item:
            bullet = "•" if item.group(2)[0] in "-*+" else item.group(2)
            body = wrap_text(item.group(3), max(20, width - len(item.group(1)) - 2))
            pad = " " * (len(item.group(1)) + 2)
            lines = style_inline(body).split("\n")
            out.append(item.group(1) + bullet + " " + lines[0])
            out.extend(pad + l for l in lines[1:])
            continue
        if raw.lstrip().startswith(">"):
            quoted = style_inline(raw.lstrip()[1:].strip())
            out.append("  │ " + quoted)
            continue
        if raw.startswith("    "):
            out.append("  │ " + raw[4:])
            continue
        out.append(style_inline(raw))
    # Collapse runs of three or more blank lines left by block spacing.
    result, blanks = [], 0
    for line in out:
        if line.strip():
            blanks = 0
            result.append(line)
        else:
            blanks += 1
            if blanks <= 1:
                result.append("")
    return "\n".join(result).rstrip() + "\n"


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        rid = msg.get("id")
        if msg.get("method") != "command":
            if rid is not None:
                host.send({"jsonrpc": "2.0", "id": rid, "result": None})
            continue

        want = (msg.get("params") or {}).get("id")
        try:
            text = host.call("editor.text")
            path = host.call("editor.path")
            if not isinstance(text, str):
                host.toast("No Markdown buffer is open", "error")
            elif want == "preview-terminal":
                host.call("editor.preview", {"kind": "text", "text": terminal_preview(text)})
                host.toast("Markdown rendered in the terminal preview", "success")
            else:
                browser_preview(text, path if isinstance(path, str) else None)
        except Exception as exc:  # never leave the editor waiting
            host.toast("Markdown preview failed: %s" % exc, "error")

        if rid is not None:
            host.send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True}})


if __name__ == "__main__":
    main()
