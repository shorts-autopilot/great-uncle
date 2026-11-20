#!/usr/bin/env python3
"""
Crawl the grandMA3 v2.2 web manual into a machine-readable spec.

Outputs (all under spec/):
  manual.json   full TOC tree + parsed content of every topic
  topics.jsonl  one flat JSON record per topic (for tooling / grep / jq)
  matrix.json   feature matrix: one row per topic and per topic section
  matrix.md     human-readable rendering of matrix.json
  REPORT.md     crawl report: coverage, counts, failures

Design notes
  * The manual is a frameset app; the real navigation lives in
    _webHelpScripts/Master/toc_nav.js as a JSON tree embedded in JS.
  * Every topic page is cached on disk, so the crawl is resumable:
    re-running only fetches what is missing or failed.
  * Extraction is deliberately lossless-ish: headings, paragraphs, nested
    lists, tables, code blocks, notes and internal links are all kept, so
    downstream work never has to re-scrape.

Usage:
  python3 tools/crawl/crawl.py                  # full crawl (resumable)
  python3 tools/crawl/crawl.py --limit 300      # fetch at most 300 new pages
  python3 tools/crawl/crawl.py --offline        # parse from cache only
  python3 tools/crawl/crawl.py --force          # ignore cache, refetch all
"""

from __future__ import annotations

import argparse
import concurrent.futures as futures
import hashlib
import json
import re
import sys
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from bs4 import BeautifulSoup, NavigableString, Tag

BASE = "https://help.malighting.com/grandMA3/2.2/"
TOC_URL = BASE + "_webHelpScripts/Master/toc_nav.js"
HTML_BASE = BASE + "HTML/"
VERSION = "2.2"
USER_AGENT = "Console-X-spec-crawler/1.0 (+internal documentation research)"

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / ".cache" / "manual"
SPEC = ROOT / "spec"


def topic_url(ext_id: str) -> str:
    return HTML_BASE + ext_id + ".html"


def cache_path(url: str) -> Path:
    """One cache file per URL: readable basename + short URL hash."""
    key = hashlib.sha1(url.encode()).hexdigest()[:16]
    name = re.sub(r"[^A-Za-z0-9._-]", "_", url.rstrip("/").split("/")[-1])[:80] or "index"
    return CACHE / f"{name}.{key}.html"


# ---------------------------------------------------------------------------
# scope mapping: manual chapter -> product module / phase / in-scope
# keys are the TOC external ids of the top-level chapters
# ---------------------------------------------------------------------------

CHAPTER_MAP: dict[str, dict] = {
    # main manual
    "news": {"module": "docs", "phase": 0, "scope": "meta"},
    "about_the_manual": {"module": "docs/help", "phase": 8, "scope": "product"},
    "device_overview": {"module": "hardware", "phase": 0, "scope": "hardware"},
    "system": {"module": "engine/parameters", "phase": 1, "scope": "product"},
    "first_steps": {"module": "hardware", "phase": 0, "scope": "hardware"},
    "onpc": {"module": "platform", "phase": 1, "scope": "product"},
    "show_file_management": {"module": "showfile", "phase": 1, "scope": "product"},
    "workspace": {"module": "ui/shell", "phase": 1, "scope": "product"},
    "command_syntax_keywords": {"module": "cmd", "phase": 2, "scope": "product"},
    "wvm": {"module": "ui/windows", "phase": 1, "scope": "product"},
    "network": {"module": "session", "phase": 5, "scope": "product"},
    "dmx": {"module": "output", "phase": 1, "scope": "product"},
    "user": {"module": "users", "phase": 5, "scope": "product"},
    "patch": {"module": "patch", "phase": 1, "scope": "product"},
    "operate_fixtures": {"module": "programmer", "phase": 2, "scope": "product"},
    "scribbles": {"module": "ui/assets", "phase": 4, "scope": "product"},
    "images": {"module": "media", "phase": 7, "scope": "product"},
    "screenshot": {"module": "system", "phase": 5, "scope": "product"},
    "file_formats": {"module": "showfile", "phase": 6, "scope": "product"},
    "videos": {"module": "media", "phase": 7, "scope": "product"},
    "meshes": {"module": "media", "phase": 7, "scope": "product"},
    "gobos": {"module": "media", "phase": 7, "scope": "product"},
    "symbols": {"module": "media", "phase": 7, "scope": "product"},
    "appear": {"module": "ui/assets", "phase": 4, "scope": "product"},
    "notes": {"module": "ui/assets", "phase": 4, "scope": "product"},
    "wvm_pool_label": {"module": "ui/windows", "phase": 1, "scope": "product"},
    "group": {"module": "pools", "phase": 2, "scope": "product"},
    "presets": {"module": "pools", "phase": 2, "scope": "product"},
    "worldfilter": {"module": "filter", "phase": 2, "scope": "product"},
    "matricks": {"module": "matrix", "phase": 3, "scope": "product"},
    "cue_sequence": {"module": "playback", "phase": 2, "scope": "product"},
    "executor": {"module": "playback/exec", "phase": 2, "scope": "product"},
    "masters": {"module": "playback/masters", "phase": 2, "scope": "product"},
    "recipes": {"module": "playback/recipes", "phase": 3, "scope": "product"},
    "phaser": {"module": "phaser", "phase": 3, "scope": "product"},
    "generator": {"module": "generator", "phase": 3, "scope": "product"},
    "bitmap": {"module": "media", "phase": 7, "scope": "product"},
    "xyz": {"module": "programmer", "phase": 4, "scope": "product"},
    "tags": {"module": "ui/assets", "phase": 4, "scope": "product"},
    "macros": {"module": "cmd/macro", "phase": 3, "scope": "product"},
    "agenda": {"module": "timecode", "phase": 3, "scope": "product"},
    "timers": {"module": "timecode", "phase": 3, "scope": "product"},
    "timecode": {"module": "timecode", "phase": 3, "scope": "product"},
    "layouts": {"module": "ui/layout", "phase": 4, "scope": "product"},
    "plugins": {"module": "script", "phase": 6, "scope": "product"},
    "quickeys": {"module": "ui/pools", "phase": 4, "scope": "product"},
    "datapool": {"module": "pools", "phase": 2, "scope": "product"},
    "system_information": {"module": "system", "phase": 5, "scope": "product"},
    "remote_inputs": {"module": "io", "phase": 5, "scope": "product"},
    "sound": {"module": "io/media", "phase": 7, "scope": "product"},
    "rdm": {"module": "io/rdm", "phase": 5, "scope": "product"},
    "local_settings": {"module": "system", "phase": 5, "scope": "product"},
    "update": {"module": "platform", "phase": 5, "scope": "product"},
    "fixture_types": {"module": "fixturetypes", "phase": 1, "scope": "product"},
    "file_management": {"module": "showfile", "phase": 5, "scope": "product"},
    "show-creator": {"module": "showtools", "phase": 6, "scope": "product"},
    "control_other_ma_devices": {"module": "session/remotedesk", "phase": 5, "scope": "product"},
    "troubleshooting": {"module": "diagnostics", "phase": 8, "scope": "product"},
    "glossary": {"module": "docs", "phase": 0, "scope": "meta"},
    "key_grandma3_listoftrademarks": {"module": "docs", "phase": 0, "scope": "legal"},
    # non-main branches
    "qsg": {"module": "docs", "phase": 0, "scope": "meta"},
    "release_notes": {"module": "docs", "phase": 0, "scope": "meta"},
}

FALLBACK_MAP = {"module": "unclassified", "phase": 0, "scope": "product"}

BOILERPLATE = [
    re.compile(r"^as master distributor ma lighting international", re.I),
    re.compile(r"^phone \+49", re.I),
    re.compile(r"terms and conditions", re.I),
    re.compile(r"^imprint$", re.I),
    re.compile(r"^privacy policy$", re.I),
    re.compile(r"^grandma3 user manual publication$", re.I),
]

NOTE_PREFIX = re.compile(
    r"^(hint|important|note|restriction|known limitation|warning|attention|tip)\s*:",
    re.I,
)

WS = re.compile(r"[\s\u00a0\u200b]+")


def norm(text: str | None) -> str:
    if not text:
        return ""
    text = text.replace("\u00a0", " ").replace("\u200b", "")
    return WS.sub(" ", text).strip()


# ---------------------------------------------------------------------------
# TOC
# ---------------------------------------------------------------------------

def fetch(url: str, force: bool = False, offline: bool = False,
          retries: int = 3, timeout: int = 30) -> bytes | None:
    path = cache_path(url)
    if path.exists() and not force:
        return path.read_bytes()
    if offline:
        return None
    last_err = None
    for attempt in range(retries):
        try:
            req = Request(url, headers={"User-Agent": USER_AGENT})
            with urlopen(req, timeout=timeout) as resp:
                data = resp.read()
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            return data
        except HTTPError as err:
            if err.code in (403, 404, 410):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"")  # negative cache
                return None
            last_err = err
        except (URLError, TimeoutError, OSError) as err:
            last_err = err
        time.sleep(0.6 * (attempt + 1))
    print(f"  !! failed {url}: {last_err}", file=sys.stderr)
    return None


def extract_toc_tree(js: str) -> list:
    anchor = js.find('"id":"357679"')
    if anchor == -1:
        raise SystemExit("TOC anchor node not found in toc_nav.js")
    start = js.rfind("[{", 0, anchor)
    if start == -1:
        raise SystemExit("TOC array start not found")
    depth = 0
    i = start
    in_str = False
    esc = False
    while i < len(js):
        ch = js[i]
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
            if depth == 0:
                i += 1
                break
        i += 1
    return json.loads(js[start:i])


def flatten(nodes: list, parent: str | None = None, depth: int = 0,
            path: tuple[str, ...] = ()) -> list[dict]:
    out = []
    for order, node in enumerate(nodes):
        ext = (node.get("vals") or {}).get("e") or None
        title = norm(node.get("t")) or "(untitled)"
        rec = {
            "toc_id": str(node.get("id")),
            "ext_id": ext,
            "title": title,
            "depth": depth,
            "order": order,
            "parent_toc_id": parent,
            "path": list(path),
        }
        out.append(rec)
        children = node.get("c") or []
        if children:
            out.extend(flatten(children, rec["toc_id"], depth + 1, path + (title,)))
    return out


# ---------------------------------------------------------------------------
# page parsing
# ---------------------------------------------------------------------------

def pick_content(soup: BeautifulSoup) -> Tag:
    for tag in soup(["script", "style", "noscript", "nav", "header", "footer", "iframe"]):
        tag.decompose()
    best = None
    best_score = len(soup.get_text(" ", strip=True))
    for el in soup.find_all(["div", "main", "article", "section"]):
        paras = el.find_all("p")
        txt = sum(len(p.get_text(" ", strip=True)) for p in paras)
        # prefer the innermost container that still holds most of the prose
        if txt > 0 and (best is None or (txt >= best_score * 0.9 and len(str(el)) < len(str(best)))):
            best, best_score = el, txt
    if best is not None and best_score > 200:
        return best
    body = soup.body or soup
    return body


def is_boilerplate(text: str) -> bool:
    return any(r.search(text) for r in BOILERPLATE)


def list_items(tag: Tag, depth: int = 0) -> list[dict]:
    items = []
    for li in tag.find_all("li", recursive=False):
        nested = [c for c in li.find_all(["ul", "ol"], recursive=False)]
        for n in nested:
            n.extract()
        text = norm(li.get_text(" ", strip=True))
        if text:
            items.append({"text": text, "depth": depth})
        for n in nested:
            items.extend(list_items(n, depth + 1))
    return items


def table_rows(tag: Tag) -> dict:
    rows = []
    for tr in tag.find_all("tr"):
        cells = [norm(c.get_text(" ", strip=True)) for c in tr.find_all(["th", "td"])]
        cells = [c for c in cells if c != ""]
        if cells:
            rows.append(cells)
    headers = []
    head = tag.find("thead")
    if head:
        first = head.find("tr")
        if first:
            headers = [norm(c.get_text(" ", strip=True)) for c in first.find_all(["th", "td"])]
    return {"headers": headers, "rows": rows}


def parse_topic(html: bytes, ext_id: str, url: str) -> dict:
    soup = BeautifulSoup(html.decode("utf-8", "replace"), "html.parser")
    doc_title = norm(soup.title.get_text()) if soup.title else ""
    container = pick_content(soup)

    blocks: list[dict] = []
    sections: list[dict] = []
    links: list[dict] = []
    current = {"heading": None, "level": 0, "items": [], "notes": []}

    def close_section():
        if current["heading"] is not None or current["items"] or current["notes"]:
            sections.append({
                "heading": current["heading"],
                "level": current["level"],
                "items": current["items"],
                "notes": current["notes"],
            })

    def add_item(text: str):
        if not text or len(text) > 400:
            return
        # drop breadcrumb tables / version stamps wherever they appear
        if "\u00bb" in text and re.search(r"version\s*\d", text, re.I):
            return
        if re.fullmatch(r"version\s*[\d.]+(\s*\|.*)?", text, re.I):
            return
        if text not in current["items"]:
            current["items"].append(text)

    def walk(el: Tag):
        nonlocal current
        for child in el.children:
            if isinstance(child, NavigableString):
                continue
            if not isinstance(child, Tag):
                continue
            name = child.name
            if name in ("h1", "h2", "h3", "h4", "h5", "h6"):
                text = norm(child.get_text("", strip=True))
                if not text:
                    continue
                lv = int(name[1])
                blocks.append({"type": "heading", "level": lv, "text": text})
                close_section()
                current = {"heading": text, "level": lv, "items": [], "notes": []}
            elif name == "p":
                text = norm(child.get_text(" ", strip=True))
                if not text or is_boilerplate(text):
                    continue
                if text == "..." or set(text) == {"."}:
                    continue
                # drop the page breadcrumb / version stamp
                if "\u00bb" in text and re.search(r"version\s*\d", text, re.I):
                    continue
                if re.fullmatch(r"version\s*\d+(\.\d+)*", text, re.I):
                    continue
                kind = "note" if NOTE_PREFIX.match(text) else "para"
                blocks.append({"type": kind, "text": text})
                if kind == "note":
                    current["notes"].append(text)
                elif len(text) <= 90 and not text.endswith("."):
                    # short, label-like paragraphs are setting names, keep as options
                    add_item(text)
            elif name in ("ul", "ol"):
                items = list_items(child)
                if items:
                    blocks.append({"type": "list", "items": items})
                    for it in items:
                        if it["depth"] == 0:
                            add_item(it["text"])
            elif name == "table":
                data = table_rows(child)
                if data["rows"]:
                    blocks.append({"type": "table", **data})
                    for row in data["rows"]:
                        add_item(" | ".join(row))
            elif name == "pre":
                text = child.get_text("\n", strip=True)
                if text:
                    blocks.append({"type": "code", "text": text})
            elif name == "img":
                blocks.append({
                    "type": "image",
                    "src": child.get("src", ""),
                    "alt": norm(child.get("alt", "")),
                })
            elif name in ("div", "section", "article", "main", "blockquote", "span", "font", "b", "strong", "em", "i", "a", "tbody"):
                walk(child)
            else:
                # unknown wrappers: recurse, but keep their direct prose
                if child.find(["h1", "h2", "h3", "h4", "h5", "h6", "p", "ul", "ol", "table", "div"]):
                    walk(child)
                else:
                    text = norm(child.get_text(" ", strip=True))
                    if text and not is_boilerplate(text) and len(text) > 2:
                        blocks.append({"type": "para", "text": text})
                        add_item(text)

    walk(container)
    close_section()

    for a in container.find_all("a", href=True):
        href = a["href"]
        if href.endswith(".html") and "://" not in href:
            links.append({"text": norm(a.get_text()), "ext_id": href[:-5]})

    seen = set()
    uniq_links = []
    for link in links:
        if link["ext_id"] not in seen:
            seen.add(link["ext_id"])
            uniq_links.append(link)

    return {
        "ext_id": ext_id,
        "url": url,
        "doc_title": doc_title,
        "blocks": blocks,
        "sections": sections,
        "links": uniq_links,
        "counts": {
            "blocks": len(blocks),
            "headings": sum(1 for b in blocks if b["type"] == "heading"),
            "tables": sum(1 for b in blocks if b["type"] == "table"),
            "notes": sum(1 for b in blocks if b["type"] == "note"),
            "links": len(uniq_links),
        },
    }


# ---------------------------------------------------------------------------
# matrix
# ---------------------------------------------------------------------------

KIND_BY_PARENT = {
    "csk_general_keywords": "keyword",
    "option_keywords": "option-keyword",
    "lua_objectfree": "lua-function",
    "lua_object": "lua-function",
}


def slug(text: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return s[:40] or "misc"


def build_matrix(nodes: list[dict], topics: dict[str, dict]) -> dict:
    rows = []
    counters: dict[str, int] = {}
    chapters: dict[str, dict] = {}
    by_toc = {r["toc_id"]: r for r in nodes}

    for rec in nodes:
        path = rec["path"] + [rec["title"]]
        # chapter = depth-1 ancestor inside the main manual; for the satellite
        # documents (Quick Start Guide, quick manuals, release notes) the
        # depth-0 root is its own chapter.
        anc = rec
        while anc["parent_toc_id"] and by_toc[anc["parent_toc_id"]]["depth"] > 0:
            anc = by_toc[anc["parent_toc_id"]]
        doc = by_toc.get(anc["parent_toc_id"]) if anc["parent_toc_id"] else anc
        if doc is not None and doc["title"].startswith("grandMA3 User Manual"):
            top, top_ext = anc["title"], anc["ext_id"]
        else:
            root = doc if doc is not None else anc
            top, top_ext = root["title"], root["ext_id"]
        mapping = CHAPTER_MAP.get(top_ext or "", FALLBACK_MAP)
        if mapping is FALLBACK_MAP:
            if top.startswith("grandMA3 Quick Manual"):
                mapping = {"module": "hardware", "phase": 0, "scope": "hardware"}
            elif top.startswith("grandMA3 Release Notes"):
                mapping = {"module": "docs", "phase": 0, "scope": "meta"}
            elif top.startswith("grandMA3 User Manual"):
                mapping = {"module": "docs", "phase": 0, "scope": "meta"}

        slug_key = (top_ext or slug(top))[:24]
        counters[slug_key] = counters.get(slug_key, 0) + 1
        rid = f"CX-{slug_key.upper()}-{counters[slug_key]:04d}"

        topic = topics.get(rec["ext_id"] or "")
        parent = by_toc.get(rec["parent_toc_id"] or "")
        kind = KIND_BY_PARENT.get(parent["ext_id"] if parent else None, "topic")

        row = {
            "id": rid,
            "kind": kind,
            "name": rec["title"],
            "path": path,
            "chapter": top,
            "module": mapping["module"],
            "phase": mapping["phase"],
            "scope": mapping["scope"],
            "status": "not-started",
            "source": {
                "ext_id": rec["ext_id"],
                "url": topic_url(rec["ext_id"]) if rec["ext_id"] else None,
                "toc_id": rec["toc_id"],
            },
            "children": [],
        }
        if kind == "keyword":
            row["token"] = rec["title"].split("(")[0].strip()
        elif kind == "option-keyword":
            row["token"] = "/" + rec["title"].lstrip("/")
        elif kind == "lua-function":
            row["signature"] = rec["title"]

        subs = []
        if topic:
            for idx, section in enumerate(topic["sections"], start=1):
                if section["heading"] is None and not section["items"]:
                    continue
                subs.append({
                    "id": f"{rid}.s{idx:02d}",
                    "kind": "section",
                    "name": section["heading"] or "Introduction",
                    "level": section["level"],
                    "items": section["items"][:80],
                    "notes": section["notes"][:20],
                    "truncated": len(section["items"]) > 80,
                })
        row["children"] = subs
        rows.append(row)

        ch = chapters.setdefault(top, {
            "chapter": top,
            "chapter_ext_id": top_ext,
            "module": mapping["module"],
            "phase": mapping["phase"],
            "scope": mapping["scope"],
            "topics": 0,
            "sections": 0,
            "items": 0,
        })
        ch["topics"] += 1
        ch["sections"] += len(subs)
        ch["items"] += sum(len(s["items"]) for s in subs)

    return {
        "meta": {
            "product": "Console-X",
            "source": TOC_URL,
            "manual_version": VERSION,
            "row_count": len(rows),
            "section_count": sum(len(r["children"]) for r in rows),
            "status_vocabulary": ["not-started", "in-progress", "implemented", "verified", "out-of-scope"],
        },
        "chapters": list(chapters.values()),
        "rows": rows,
    }


def render_matrix_md(matrix: dict) -> str:
    lines = [
        "# Console-X feature matrix",
        "",
        f"Generated from the grandMA3 v{VERSION} web manual. "
        f"{matrix['meta']['row_count']} topics, {matrix['meta']['section_count']} sections.",
        "",
        "Status vocabulary: " + ", ".join(f"`{s}`" for s in matrix["meta"]["status_vocabulary"]) + ".",
        "",
        "Scope reasons and chapter dispositions: [`docs/COMPLETENESS.md`](../docs/COMPLETENESS.md).",
        "",
    ]
    by_chapter: dict[str, list[dict]] = {}
    for row in matrix["rows"]:
        by_chapter.setdefault(row["chapter"], []).append(row)

    for ch in matrix["chapters"]:
        rows = by_chapter[ch["chapter"]]
        lines.append(f"## {ch['chapter']}")
        lines.append("")
        lines.append(
            f"`{ch['module']}` · phase {ch['phase']} · {ch['topics']} topics · "
            f"{ch['sections']} sections · {ch['items']} extracted options"
        )
        lines.append("")
        lines.append("| ID | Feature | Sections | Phase | Status |")
        lines.append("|---|---|---|---|---|")
        for row in rows:
            name = row["name"].replace("|", "\\|")
            lines.append(
                f"| `{row['id']}` | {name} | {len(row['children'])} | {row['phase']} | {row['status']} |"
            )
        lines.append("")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=0, help="max new pages to fetch this run")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    CACHE.mkdir(parents=True, exist_ok=True)
    SPEC.mkdir(parents=True, exist_ok=True)

    print(f"fetching TOC: {TOC_URL}")
    toc_bytes = fetch(TOC_URL, force=args.force, offline=args.offline)
    if not toc_bytes:
        raise SystemExit("could not load toc_nav.js (try without --offline)")
    toc_js = toc_bytes.decode("utf-8", "replace")
    tree = extract_toc_tree(toc_js)
    nodes = flatten(tree)
    print(f"TOC: {len(nodes)} nodes")

    wanted: list[str] = []
    seen = set()
    for rec in nodes:
        if rec["ext_id"] and rec["ext_id"] not in seen:
            seen.add(rec["ext_id"])
            wanted.append(rec["ext_id"])
    print(f"unique topics to parse: {len(wanted)}")

    def work(ext_id: str):
        url = topic_url(ext_id)
        data = fetch(url, force=args.force, offline=args.offline)
        if not data:
            return ext_id, None, "missing"
        try:
            return ext_id, parse_topic(data, ext_id, url), "ok"
        except Exception as err:  # noqa: BLE001 - keep crawling, report later
            return ext_id, None, f"parse-error: {err}"

    topics: dict[str, dict] = {}
    failures: list[dict] = []
    started = time.time()

    cached = [e for e in wanted if cache_path(topic_url(e)).exists()]
    pending = [e for e in wanted if not cache_path(topic_url(e)).exists()]
    deferred: list[str] = []
    if args.limit and not args.offline:
        deferred = pending[args.limit:]
        pending = pending[: args.limit]
    todo = cached + pending
    print(f"cached: {len(cached)} | fetching: {len(pending)} | deferred: {len(deferred)}")

    with futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        futs = [pool.submit(work, ext) for ext in todo]
        done = 0
        for fut in futures.as_completed(futs):
            ext_id, parsed, status = fut.result()
            done += 1
            if parsed:
                topics[ext_id] = parsed
            else:
                failures.append({"ext_id": ext_id, "status": status})
            if done % 100 == 0:
                print(f"  {done}/{len(todo)} pages ({time.time() - started:.0f}s)")

    # thread completion order is not stable; emit topics in document order
    topics = {ext: topics[ext] for ext in wanted if ext in topics}

    parsed_ok = len(topics)
    print(f"parsed {parsed_ok}/{len(wanted)} topics in {time.time() - started:.0f}s")

    manual = {
        "meta": {
            "product": "Console-X",
            "source": BASE,
            "toc_url": TOC_URL,
            "manual_version": VERSION,
            "toc_nodes": len(nodes),
            "unique_topics": len(wanted),
            "parsed_topics": parsed_ok,
            "failed_topics": len(failures),
        },
        "tree": tree,
        "nodes": nodes,
        "topics": topics,
    }
    (SPEC / "manual.json").write_text(json.dumps(manual, ensure_ascii=False, indent=1))

    with (SPEC / "topics.jsonl").open("w") as fh:
        for rec in nodes:
            topic = topics.get(rec["ext_id"] or "")
            line = {
                "toc_id": rec["toc_id"],
                "ext_id": rec["ext_id"],
                "title": rec["title"],
                "path": rec["path"] + [rec["title"]],
                "depth": rec["depth"],
                "url": topic_url(rec["ext_id"]) if rec["ext_id"] else None,
                "headings": [b["text"] for b in (topic or {}).get("blocks", []) if b["type"] == "heading"],
                "counts": (topic or {}).get("counts", {}),
            }
            fh.write(json.dumps(line, ensure_ascii=False) + "\n")

    matrix = build_matrix(nodes, topics)
    (SPEC / "matrix.json").write_text(json.dumps(matrix, ensure_ascii=False, indent=1))
    (SPEC / "matrix.md").write_text(render_matrix_md(matrix))

    phase_totals: dict[int, int] = {}
    module_totals: dict[str, int] = {}
    scope_totals: dict[str, int] = {}
    for row in matrix["rows"]:
        phase_totals[row["phase"]] = phase_totals.get(row["phase"], 0) + 1
        module_totals[row["module"]] = module_totals.get(row["module"], 0) + 1
        scope_totals[row["scope"]] = scope_totals.get(row["scope"], 0) + 1

    report = [
        "# Phase 0 crawl report",
        "",
        f"- Source: {BASE}",
        f"- Manual version: {VERSION}",
        f"- TOC nodes: {len(nodes)}",
        f"- Unique topics: {len(wanted)}",
        f"- Parsed: {parsed_ok}",
        f"- Deferred by --limit: {len(deferred)}",
        f"- Failed/missing: {len(failures)}",
        f"- Topics with content: {sum(1 for t in topics.values() if t['blocks'])}",
        f"- Matrix rows (topics): {matrix['meta']['row_count']}",
        f"- Matrix sections: {matrix['meta']['section_count']}",
        "",
        "## Topics per phase",
        "",
        "| Phase | Topics |",
        "|---|---|",
    ]
    for phase in sorted(phase_totals):
        report.append(f"| {phase} | {phase_totals[phase]} |")
    kind_totals: dict[str, int] = {}
    item_total = 0
    for row in matrix["rows"]:
        kind_totals[row["kind"]] = kind_totals.get(row["kind"], 0) + 1
        item_total += sum(len(c["items"]) for c in row["children"])
    report += [
        "",
        "## Topics per kind",
        "",
        "| Kind | Rows |",
        "|---|---|",
    ]
    for kind in sorted(kind_totals, key=lambda k: -kind_totals[k]):
        report.append(f"| `{kind}` | {kind_totals[kind]} |")
    report.append("")
    report.append(f"Extracted option/setting items: **{item_total}**")
    report += ["", "## Topics per module", "", "| Module | Topics |", "|---|---|"]
    for module in sorted(module_totals, key=lambda m: -module_totals[m]):
        report.append(f"| `{module}` | {module_totals[module]} |")
    report += ["", "## Topics per scope", "", "| Scope | Topics |", "|---|---|"]
    for scope in sorted(scope_totals):
        report.append(f"| {scope} | {scope_totals[scope]} |")
    if failures:
        report += ["", "## Missing or failed pages", "", "| ext_id | status |", "|---|---|"]
        for fail in failures[:200]:
            report.append(f"| `{fail['ext_id']}` | {fail['status']} |")
        if len(failures) > 200:
            report.append(f"| … | {len(failures) - 200} more |")
    report += [
        "",
        "## Files",
        "",
        "- `spec/manual.json` — TOC tree + every parsed topic (headings, paragraphs, lists, tables, notes, links).",
        "- `spec/topics.jsonl` — flat one-line-per-topic index for `jq`/grep tooling.",
        "- `spec/matrix.json` — feature matrix rows (topic level + section level) with module, phase, status.",
        "- `spec/matrix.md` — readable rendering of the matrix.",
        "- `spec/REPORT.md` — this file.",
        "- `.cache/manual/` — raw HTML cache (untracked), makes re-runs resumable.",
    ]
    (SPEC / "REPORT.md").write_text("\n".join(report) + "\n")

    print(f"wrote spec/ ({parsed_ok} topics, {matrix['meta']['section_count']} sections)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
