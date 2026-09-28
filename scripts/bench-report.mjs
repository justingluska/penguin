// Turns the benchmark results in docs/perf/*.json into docs/PERFORMANCE.md
// and the Performance block in README.md (between the perf:start/perf:end
// markers). Run by scripts/bench.sh; run it alone after editing the prose:
//
//   node scripts/bench-report.mjs
//
// Results files: <os>-<arch>-backend-<corpus>.json (crates/penguin-core
// examples/bench.rs) and <os>-<arch>-ui.json (scripts/bench-ui). Each
// <os>-<arch> is one machine section. The README quotes the macOS machine
// when there is one, else the first.

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const perfDir = join(root, "docs/perf");

// ----- load -----
const machines = new Map(); // tag → { backend: Map(size → json), ui }
for (const f of readdirSync(perfDir).filter((f) => f.endsWith(".json")).sort()) {
  const m = f.match(/^(.+?)-(backend-(.+)|ui)\.json$/);
  if (!m) continue;
  const [, tag, , size] = m;
  if (!machines.has(tag)) machines.set(tag, { backend: new Map(), ui: null });
  const data = JSON.parse(readFileSync(join(perfDir, f), "utf8"));
  if (size) machines.get(tag).backend.set(size, data);
  else machines.get(tag).ui = data;
}
if (!machines.size) throw new Error("no results in docs/perf; run scripts/bench.sh first");
const sizeKey = (s) => parseFloat(s) * (/k$/i.test(s) ? 1e3 : /m$/i.test(s) ? 1e6 : 1);
for (const m of machines.values()) m.backend = new Map([...m.backend].sort((a, b) => sizeKey(a[0]) - sizeKey(b[0])));
const tags = [...machines.keys()].sort((a, b) => (b.startsWith("macos") ? 1 : 0) - (a.startsWith("macos") ? 1 : 0));
const primaryTag = tags[0];
// "Known slow paths" was investigated on the Linux VM; its inline numbers come from that machine.
const slowPathTag = tags.find((t) => t.startsWith("linux")) ?? primaryTag;

// ----- formatting -----
const ms = (x) => (x == null ? "–" : x < 1 ? x.toFixed(2) : x < 10 ? x.toFixed(1) : Math.round(x).toString());
const int = (x) => (x == null ? "–" : Math.round(x).toLocaleString("en-US"));
const mib = (x) => (x == null ? "–" : x < 10 ? x.toFixed(1) : Math.round(x).toLocaleString("en-US"));
const p = (s, k = "p95") => ms(s?.[k]);
const trio = (s) => (s?.n ? `${ms(s.p50)} / ${ms(s.p95)} / ${ms(s.p99)}` : "–");
const sizeLabel = (s) => int(sizeKey(s));
const esc = (s) => String(s).replace(/\|/g, "\\|");
const table = (head, rows, align) =>
  [
    `| ${head.join(" | ")} |`,
    `|${head.map((_, i) => (align?.[i] === "l" || (!align && i === 0) ? "---" : "---:")).join("|")}|`,
    ...rows.map((r) => `| ${r.join(" | ")} |`),
  ].join("\n");

function machineShort(m) {
  return m.virtualized
    ? `a shared ${m.os.split(" ")[0]} cloud VM (${m.cpu}, ${m.logicalCores} vCPUs)`
    : `${/^Apple /.test(m.cpu) ? "an" : "a"} ${m.cpu} (${m.os}, ${m.logicalCores} cores, ${m.ramGiB} GiB RAM)`;
}

function machineLine(b) {
  const m = b.machine;
  const virt = m.virtualized ? ", virtual machine" : "";
  return `${m.cpu ?? "unknown CPU"}, ${m.logicalCores} logical cores, ${m.ramGiB} GiB RAM, ${m.os} (${m.arch}${virt})`;
}

// ----- sections -----
function headline(tag) {
  const { backend, ui } = machines.get(tag);
  const sizes = [...backend.keys()];
  const col = (f) => sizes.map((s) => f(backend.get(s)));
  const rows = [
    ["Search, all 27 query shapes pooled (p95)", ...col((b) => `${p(b.search.pooledMs)} ms`)],
    ["Search as you type, per keystroke (p95)", ...col((b) => `${p(b.asYouType.pooledMs)} ms`)],
    ["Open a thread: read + sanitize + render (p95)", ...col((b) => `${p(b.thread.totalMs)} ms`)],
    ["Inbox, first 50 conversations (p95)", ...col((b) => `${p(b.list.views[0].ms)} ms`)],
    ["Next page while scrolling (p95)", ...col((b) => `${p(b.list.scroll.pageMs)} ms`)],
    ["Ask your inbox (p95)", ...col((b) => `${p(b.ask.pooledMs)} ms`)],
    ["Launch: open database → first Inbox page, warm (p50)", ...col((b) => `${p(b.open.warm.timeToFirstListMs, "p50")} ms`)],
    ["Launch, OS file cache evicted (p50)", ...col((b) => (b.open.cold.pageCacheEvicted ? `${p(b.open.cold.timeToFirstListMs, "p50")} ms` : "n/a"))],
    ["Ingest into the local index", ...col((b) => (b.ingest ? `${int(b.ingest.messagesPerSec)} msg/s` : "–"))],
    ["Database size per 100k messages", ...col((b) => `${mib(b.disk.mibPer100k)} MiB`)],
  ];
  let out = table(["", ...sizes.map((s) => `${sizeLabel(s)} messages`)], rows);
  if (ui) {
    out += "\n\n" + table(
      ["Web layer (headless Chromium, mock backend)", ""],
      [
        ["`j`/`k` → selection painted (p95)", `${p(ui.keyboard.selectionPaintMs)} ms`],
        ["`j`/`k` → reading pane with the message rendered (p95)", `${p(ui.keyboard.readingPaneMs)} ms`],
        ["Keystroke → search results rendered (p95)", `${p(ui.search.keystrokeToResultsMs)} ms`],
        ["Scrolling a 5,000-conversation list: frame time (p50 / p99)", `${ms(ui.scroll.frameMs.p50)} / ${ms(ui.scroll.frameMs.p99)} ms`],
        ["First conversation rows painted after load (p50)", `${p(ui.startup.firstRowPaint, "p50")} ms`],
      ],
      ["l", "r"],
    );
  }
  return out;
}

function machineSection(tag) {
  const { backend, ui } = machines.get(tag);
  const any = [...backend.values()][0] ?? ui;
  const lines = [];
  const b0 = [...backend.values()][0];
  if (b0) {
    lines.push(`- **Machine:** ${machineLine(b0)}.`);
    const c = (x) => (x ? `load ${x.loadAvg1m ?? "–"}${x.cpuPressureSomeAvg60Pct != null ? `, CPU wait ${x.cpuPressureSomeAvg60Pct}%` : ""}` : "–");
    const runs = [...backend.entries()].map(([s, b]) => `${sizeLabel(s)}: ${c(b.contention?.start)} → ${c(b.contention?.end)}`);
    if (ui?.contention) runs.push(`UI: ${c(ui.contention.start)} → ${c(ui.contention.end)}`);
    lines.push(`- **How busy the machine was** (1-minute load average; CPU wait = share of the last minute runnable tasks waited for a CPU), start → end of each run: ${runs.join("; ")}. The machine is shared with other work, so tails (p99, max) are noisier than medians.`);
    lines.push(`- **Build:** Rust release profile (\`lto = true\`, \`codegen-units = 1\`, \`opt-level = 3\`), bundled SQLite from rusqlite 0.37.`);
    const commits = [...new Set([...backend.values()].map((b) => b.git?.commit))].join(", ");
    lines.push(`- **Commit:** ${commits}${[...backend.values()].some((b) => b.git?.dirtyCrates) ? " (with uncommitted changes under crates/)" : ""}; run ${[...backend.values()].map((b) => b.date.slice(0, 10))[0]}.`);
  }
  if (ui) lines.push(`- **Browser:** ${ui.browser}, viewport ${ui.viewport}.`);
  if (!any) return "";
  return lines.join("\n");
}

function corpusSection(tag) {
  const { backend } = machines.get(tag);
  const rows = [];
  const sizes = [...backend.keys()].filter((s) => backend.get(s).corpus);
  if (!sizes.length) return "";
  const c = (f) => sizes.map((s) => f(backend.get(s).corpus));
  const pct = (x) => `${Math.round(x * 100)}%`;
  rows.push(["Messages", ...c((x) => int(x.messages))]);
  rows.push(["Conversations", ...c((x) => int(x.threads))]);
  rows.push(["Accounts (60 / 30 / 10% of mail)", ...c((x) => x.accounts)]);
  rows.push(["Conversation length 1 / 2–3 / 4–8", ...c((x) => `${int(x.threadLength["1"])} / ${int(x.threadLength["2-3"])} / ${int(x.threadLength["4-8"])}`)]);
  rows.push(["Messages with an HTML body", ...c((x) => pct(x.htmlShare))]);
  rows.push(["Newsletters (List-Unsubscribe)", ...c((x) => pct(x.newsletterShare))]);
  rows.push(["Average plain text per message", ...c((x) => `${(x.avgTextBytes / 1024).toFixed(1)} KB`)]);
  rows.push(["Average HTML body / largest", ...c((x) => `${(x.avgHtmlBytes / 1024).toFixed(1)} KB / ${Math.round(x.maxHtmlBytes / 1024)} KB`)]);
  rows.push(["Raw text + HTML", ...c((x) => `${mib(x.plainTextMiB + x.rawHtmlMiB)} MiB`)]);
  rows.push(["Attachments (metadata)", ...c((x) => int(x.attachments))]);
  rows.push(["In the Inbox / unread", ...c((x) => `${int(x.inboxMessages)} / ${int(x.unreadMessages)}`)]);
  rows.push(["With a user label (20 per account)", ...c((x) => int(x.userLabelledMessages))]);
  return table(["", ...sizes.map((s) => `${sizeLabel(s)}`)], rows);
}

function backendSections(tag) {
  const { backend } = machines.get(tag);
  const sizes = [...backend.keys()];
  const B = (s) => backend.get(s);
  const hdr = (first) => [first, ...sizes.map((s) => `${sizeLabel(s)}`)];
  const out = [];

  // Ingest
  const ing = sizes.filter((s) => B(s).ingest);
  if (ing.length) {
    out.push("### Ingest: the local half of sync\n");
    out.push(
      "Messages written with `Store::upsert_messages` in 100-message transactions, the same chunk size sync uses (`FETCH_CHUNK` in penguin-gmail): message rows, zstd-compressed bodies, attachments, labels, people, thread aggregates and view rows, and the FTS5 index, all in one transaction per chunk. Generating the synthetic mail is excluded.\n",
    );
    out.push(
      table(
        ["", ...ing.map(sizeLabel)],
        [
          ["Messages per second, whole run", ...ing.map((s) => int(B(s).ingest.messagesPerSec))],
          ["… first tenth of the run", ...ing.map((s) => int(B(s).ingest.firstTenthMessagesPerSec))],
          ["… last tenth of the run", ...ing.map((s) => int(B(s).ingest.lastTenthMessagesPerSec))],
          ["CPU per message (the writing thread; checkpoints run on another)", ...ing.map((s) => (B(s).ingest.cpuMsPerMessage != null ? `${ms(B(s).ingest.cpuMsPerMessage)} ms` : "–"))],
          ["Plain text indexed per second", ...ing.map((s) => `${mib(B(s).ingest.plainTextMiBPerSec)} MiB`)],
          ["Index merge after the backfill (`Store::optimize`)", ...ing.map((s) => `${(B(s).ingest.ftsOptimizeMs / 1000).toFixed(1)} s`)],
          ["… a write issued meanwhile waits (p50 / p95 / max)", ...ing.map((s) => {
            const w = B(s).ingest.writeWaitDuringOptimizeMs;
            return w?.n ? `${ms(w.p50)} / ${ms(w.p95)} / ${ms(w.max)} ms` : "–";
          })],
        ],
      ),
    );
    const side = ing.map((s) => B(s).ingest.orderSideTest).filter(Boolean);
    if (side.length) {
      const avg = (f) => side.reduce((a, x) => a + f(x), 0) / side.length;
      out.push(
        `\n**Arrival order.** The corpus is written in the generator's order, random by date. A Gmail backfill arrives newest first. A side test in each run ingests the same ${int(side[0].messages)} messages both ways into fresh databases: generator order ${int(avg((x) => x.generatorOrder.messagesPerSec))} messages/s (${ms(avg((x) => x.generatorOrder.cpuMsPerMessage))} ms CPU each), newest first ${int(avg((x) => x.newestFirst.messagesPerSec))} messages/s (${ms(avg((x) => x.newestFirst.cpuMsPerMessage))} ms), averaged over the ${side.length} runs. Each batch is written oldest first whatever order it arrives in, so newest first is no slower.\n`,
      );
    }
    const slowest = Math.min(...ing.map((s) => B(s).ingest.lastTenthMessagesPerSec ?? B(s).ingest.messagesPerSec));
    out.push(
      `\n**Sync is limited by Gmail, not by this.** Google's documented per-user quota is 15,000 quota units a minute (250 a second) and \`messages.get\` costs 5 units, so fetching full messages tops out around 50 messages a second per account. The slowest stretch measured here, the last tenth of the largest backfill, still stores ${int(slowest)} messages a second, about ${Math.floor(slowest / 50)}× that ceiling, so the index is not what a first sync waits on. Ingest does slow down as the database grows: each message touches a leaf page in a dozen B-trees (rows, bodies, indexes, thread views, people) at a position set by its date, so a batch dirties pages all over a growing file, and FTS5 has more segments to merge. See [Known slow paths](#known-slow-paths).\n`,
    );
  }

  // Disk
  out.push("### Size on disk\n");
  out.push(
    table(
      hdr(""),
      [
        ["Database (after FTS optimize)", ...sizes.map((s) => `${mib(B(s).disk.totalMiB)} MiB`)],
        ["Per 100k messages", ...sizes.map((s) => `${mib(B(s).disk.mibPer100k)} MiB`)],
        ["Bytes per message", ...sizes.map((s) => int(B(s).disk.bytesPerMessage))],
        ["… of which full-text index", ...sizes.map((s) => `${mib(B(s).disk.breakdownMiB.fullTextIndex)} MiB`)],
        ["… of which bodies (zstd)", ...sizes.map((s) => `${mib(B(s).disk.breakdownMiB["bodies (zstd)"])} MiB`)],
        ["… of which message rows, threads, views", ...sizes.map((s) => `${mib((B(s).disk.breakdownMiB.messageRows ?? 0) + (B(s).disk.breakdownMiB.threadsAndViews ?? 0))} MiB`)],
      ],
    ),
  );
  const c0 = B(sizes[sizes.length - 1]).corpus;
  if (c0) out.push(`\nThe raw text and HTML of the ${sizeLabel(sizes[sizes.length - 1])}-message corpus is ${mib(c0.plainTextMiB + c0.rawHtmlMiB)} MiB; the database holds all of it, compressed, plus the index. Attachment contents are not downloaded or stored, only their metadata.\n`);

  // Open
  out.push("### Opening the database (app launch)\n");
  out.push(
    "`Store::open` (connection, WAL, migrations check), then the first page of the unified Inbox, then the label list with unread counts, then one search. *Warm* reopens a database the OS has cached (the usual case: relaunching the app). *Cold* first drops the database files from the OS page cache with `posix_fadvise(DONTNEED)` (Linux only), as after a reboot. Times are from the start of `Store::open`; p50 / p95 / p99 in ms.\n",
  );
  out.push(
    table(
      hdr(""),
      [
        ["Warm: `Store::open`", ...sizes.map((s) => trio(B(s).open.warm.openMs))],
        ["Warm: → first Inbox page", ...sizes.map((s) => trio(B(s).open.warm.timeToFirstListMs))],
        ["Warm: → + labels with unread counts", ...sizes.map((s) => trio(B(s).open.warm.plusLabelsMs))],
        ["Warm: first search", ...sizes.map((s) => trio(B(s).open.warm.firstSearchMs))],
        ["Cold: `Store::open`", ...sizes.map((s) => (B(s).open.cold.pageCacheEvicted ? trio(B(s).open.cold.openMs) : "n/a"))],
        ["Cold: → first Inbox page", ...sizes.map((s) => (B(s).open.cold.pageCacheEvicted ? trio(B(s).open.cold.timeToFirstListMs) : "n/a"))],
        ["Cold: → + labels with unread counts", ...sizes.map((s) => (B(s).open.cold.pageCacheEvicted ? trio(B(s).open.cold.plusLabelsMs) : "n/a"))],
        ["Cold: first search", ...sizes.map((s) => (B(s).open.cold.pageCacheEvicted ? trio(B(s).open.cold.firstSearchMs) : "n/a"))],
      ],
    ),
  );
  const rr = sizes.map((s) => B(s).open.diskRandomRead4kMs).find((x) => x?.n);
  out.push(
    `\nCold numbers are disk-bound and much larger than the page count explains.${rr ? ` One uncached 4 KiB read at a random offset of the database takes ${ms(rr.p50)} ms here (p50; ${ms(rr.p95)} ms p95, ${rr.n} reads), and the first Inbox page needs about 80 of them (336 KiB, measured with the map off).` : ""} The time goes to readahead through SQLite's memory map (334 MiB read to show 50 rows of the 100k database on this machine): see [Known slow paths](#known-slow-paths).\n`,
  );

  // Lists
  out.push("### Mailbox lists\n");
  out.push("`Store::list_threads`, 50 conversations per page (the UI's page size), all accounts unless noted. p50 / p95 / p99 in ms.\n");
  const views = B(sizes[0]).list.views.map((v) => v.name);
  out.push(
    table(
      hdr("View"),
      [
        ...views.map((name) => [name, ...sizes.map((s) => trio(B(s).list.views.find((v) => v.name === name)?.ms))]),
        ["Scrolling: each of 40 consecutive pages (All mail)", ...sizes.map((s) => trio(B(s).list.scroll.pageMs))],
        ["Scrolling: all 40 pages, 2,000 conversations", ...sizes.map((s) => trio(B(s).list.scroll.all40PagesMs))],
        ["Sidebar labels with unread counts", ...sizes.map((s) => trio(B(s).list.labelsWithUnreadCounts.ms))],
      ],
    ),
  );

  const coldScroll = sizes.filter((s) => B(s).list.scroll.all40PagesMs.max > 5 * B(s).list.scroll.all40PagesMs.p50);
  if (coldScroll.length)
    out.push(
      `\nThe scrolling tails at ${coldScroll.map(sizeLabel).join(" and ")} messages come from the first of the ten passes, which reaches rows deep in All mail that are not in memory (the cold-open runs just before evicted the file from the OS cache); later passes take the p50.`,
    );

  // Thread
  out.push("\n### Opening a conversation\n");
  out.push(
    "What `get_thread` does, minus the IPC hop: read the thread's messages from SQLite and decompress the bodies, then for each message sanitize and render the HTML with penguin-render (remote images and trackers blocked) or render the plain text, detect a one-time code, and work out the unsubscribe offer. 500 random conversations plus the longest one, each opened once (no warm-up, so SQLite's cache isn't primed for it). p50 / p95 / p99 in ms.\n",
  );
  out.push(
    table(
      hdr(""),
      [
        ["Read from the store", ...sizes.map((s) => trio(B(s).thread.storeReadMs))],
        ["Sanitize + render", ...sizes.map((s) => trio(B(s).thread.renderMs))],
        ["**Total**", ...sizes.map((s) => trio(B(s).thread.totalMs))],
        ["Total, conversations", ...sizes.map((s) => trio(B(s).thread.conversationTotalMs))],
        ["Total, newsletters", ...sizes.map((s) => trio(B(s).thread.newsletterTotalMs))],
        ["Average rendered HTML per thread", ...sizes.map((s) => `${mib(B(s).thread.avgRenderedKiB)} KiB`)],
      ],
    ),
  );
  const r = B(sizes[sizes.length - 1]).render;
  out.push("\n### Rendering email HTML\n");
  out.push("penguin-render alone on synthetic ESP-style newsletters (nested tables, inline styles, a head stylesheet with media queries, remote images, tracked links, an open pixel), 20 runs each. The UI caches the last 60 rendered threads and prefetches the neighbours of the selected one (and the row under the pointer), so this cost is usually paid before you press `j` or click.\n");
  out.push(
    table(
      ["Input", "Size", "p50 ms", "p95 ms", "Remote images blocked", "Trackers removed"],
      r.map((x) => [x.name, `${Math.round(x.inputBytes / 1024)} KB`, ms(x.ms.p50), ms(x.ms.p95), x.blockedRemoteImages ?? "–", x.trackersRemoved ?? "–"]),
    ),
  );

  // Search
  out.push("\n### Search\n");
  out.push("`Store::search` with the UI's limit of 50 conversations, warm, 30 runs per query. The last word of a free-text query is prefix-matched, as while typing. Planted known items (a lease PDF from Mike, invoice INV-20417, a wifi password) check that the right message is on top, not just that something came back.\n");
  const qs = B(sizes[0]).search.queries;
  const largest = sizes[sizes.length - 1];
  out.push(
    table(
      ["Query", "Kind", ...sizes.map((s) => `${sizeLabel(s)} p50 / p95`), `Conversations (${sizeLabel(largest)})`, "Known item on top"],
      qs.map((q, i) => [
        "`" + esc(q.query.replace(/after:\S+ before:\S+/, "after:<1y ago> before:<6mo ago>")) + "`",
        q.kind,
        ...sizes.map((s) => {
          const x = B(s).search.queries[i]?.ms;
          return x ? `${ms(x.p50)} / ${ms(x.p95)}` : "–";
        }),
        B(largest).search.queries[i]?.threads ?? "–",
        B(largest).search.queries[i]?.topHitIsPlanted ? "yes" : "",
      ]),
      ["l", "l"],
    ),
  );
  const kinds = Object.keys(B(largest).search.byKind);
  out.push("\nBy kind (p50 / p95 / p99 ms):\n");
  out.push(table(hdr("Kind"), [...kinds.map((k) => [k === "none" ? "no match" : k, ...sizes.map((s) => trio(B(s).search.byKind[k]))]), ["**all queries**", ...sizes.map((s) => trio(B(s).search.pooledMs))]]));

  out.push("\n### Search as you type\n");
  out.push("Every prefix of each string is its own search, as each keystroke is in the app (the UI coalesces keystrokes to one search per frame and drops stale responses). 10 passes. p50 / p95 / p99 ms per keystroke.\n");
  const strs = B(sizes[0]).asYouType.strings.map((x) => x.typed);
  out.push(
    table(hdr("Typed"), [
      ...strs.map((t, i) => ["`" + esc(t) + "`", ...sizes.map((s) => trio(B(s).asYouType.strings[i]?.ms))]),
      ["**every keystroke**", ...sizes.map((s) => trio(B(s).asYouType.pooledMs))],
      ["worst keystroke", ...sizes.map((s) => `${ms(B(s).asYouType.pooledMs.max)}`)],
    ]),
  );

  out.push("\n### Ask your inbox\n");
  out.push("`Store::ask`: question grammar, person and company resolution, indexed queries and answer extraction, no model. 20 runs per question. p50 / p95 ms.\n");
  const asks = B(sizes[0]).ask.questions;
  out.push(
    table(hdr("Question"), [
      ...asks.map((q, i) => [esc(q.template ?? q.question), ...sizes.map((s) => {
        const x = B(s).ask.questions[i]?.ms;
        return x ? `${ms(x.p50)} / ${ms(x.p95)}` : "–";
      })]),
      ["**all questions** (p50 / p95 / p99)", ...sizes.map((s) => trio(B(s).ask.pooledMs))],
    ]),
  );
  return out.join("\n");
}

function uiSection(tag) {
  const { ui } = machines.get(tag);
  if (!ui) return "";
  const out = [];
  out.push(`Penguin's production bundle (\`vite build\` with \`VITE_MOCK=1\`) served locally and driven by Playwright in ${ui.browser}. The backend is the in-browser mock (\`src/lib/mock\`), which answers every command after a simulated 4 ms IPC hop and doesn't run SQLite, so these numbers are the web layer's own cost: React, the virtualized list, keyboard handling, the sandboxed message iframe. Add the backend numbers above for the whole path.\n`);
  out.push(`**This is Chromium on ${machines.get(tag).backend.values().next().value?.machine.os ?? tag}, not the real app.** Penguin ships in WKWebView (Safari's engine) on macOS; layout, iframe and paint costs differ between engines and machines. Use these as a regression baseline for the web layer, not as the app's latency on a Mac.\n`);
  out.push("Timings are taken inside the page, from the input event's own timestamp (when the browser received it) to the paint after the DOM changed (`requestAnimationFrame`, then a task). Frame-based measurements are quantized to the 16.7 ms frame. p50 / p95 / p99 in ms.\n");
  out.push(
    table(
      ["", "p50 / p95 / p99"],
      [
        [`Load → first contentful paint (${ui.startup.fcp.n} fresh loads, 300-conversation mock)`, trio(ui.startup.fcp)],
        ["Load → first conversation rows painted", trio(ui.startup.firstRowPaint)],
        [`\`j\`/\`k\` → row selected in the DOM (${ui.keyboard.selectionDomMs.n} presses, 5,000 conversations)`, trio(ui.keyboard.selectionDomMs)],
        ["`j`/`k` → selection painted", trio(ui.keyboard.selectionPaintMs)],
        ["`j`/`k` → reading pane shows the conversation's subject", trio(ui.keyboard.paneSubjectMs)],
        ["`j`/`k` → message body loaded in its sandboxed iframe, painted", trio(ui.keyboard.readingPaneMs)],
        [`Click a conversation further down → message body painted (${ui.clickOpen.readingPaneMs.n} clicks)`, trio(ui.clickOpen.readingPaneMs)],
        [`Search keystroke → results for it rendered (${ui.search.keystrokeToResultsMs.n} keystrokes)`, trio(ui.search.keystrokeToResultsMs)],
        ["Search keystroke → last DOM change before the next key", trio(ui.search.keystrokeToSettledMs)],
        [`Scrolling the list at ~3,600 px/s for 5 s: frame interval (${ui.scroll.frames} frames)`, trio(ui.scroll.frameMs)],
      ],
      ["l", "r"],
    ),
  );
  out.push(
    `\nWhile scrolling ${int(ui.scroll.scrolledPx)} px through ${int(ui.mockThreads)} conversations, ${ui.scroll.framesOver20ms} of ${ui.scroll.frames} frames took longer than 20 ms, ${ui.scroll.framesOver33ms} longer than 33 ms, with ${ui.scroll.longTasks} long tasks (>50 ms); the list kept ${ui.scroll.rowsInDom} rows in the DOM. Page load: ${mib(ui.bundle.jsKiB)} KiB of JavaScript (${mib(ui.bundle.jsGzipKiB)} KiB gzipped) and ${mib(ui.bundle.cssKiB)} KiB of CSS; the app loads them from disk, not the network. JS heap after load: ${ms(ui.startup.heapMiB.p50)} MiB.${ui.startup.launchJsKiB?.n ? ` Of that, launch fetches ${mib(ui.startup.launchJsKiB.p50)} KiB of JavaScript and ${mib(ui.startup.launchCssKiB.p50)} KiB of CSS before the first rows paint (the mock backend's own chunk left out); the rest loads afterwards, in idle moments.` : ""}\n`,
  );
  if (ui.storm?.events)
    out.push(
      `A backfill's event storm (\`mail-changed\` every 100 ms for 8 s, ${int(ui.storm.events)} events, about 1,000 rows loaded) caused ${ui.storm.listCalls} list re-reads (${int(ui.storm.rowsAsked)} rows); \`j\` pressed every 200 ms during it painted its selection in ${trio(ui.storm.selectionPaintMs)} ms (p50 / p95 / p99).\n`,
    );
  return out.join("\n");
}

const HOW = `## Why it's fast

Every interaction reads local state. Nothing on the path from a key press to pixels waits on the network; Gmail is only talked to in the background (sync, sending, and the actions you take, which are applied locally first and pushed afterwards).

**One SQLite database, shaped for the reads the UI makes** (\`crates/penguin-core/src/store.rs\`).
- **Precomputed views.** Each conversation has one row per mailbox it appears in (\`thread_views\`: Inbox, each tab, Sent, each label…) with its latest date and an unread flag, kept current on every write. A mailbox page is one range scan of \`(view, last_date)\` or \`(account_id, view, last_date)\` joined to 50 thread rows, and the next page is keyed by the last date on screen, not an OFFSET. That is why page 40 costs what page 1 costs, and why list times barely move between 10k and 300k messages. Several accounts are merged from one index scan per account, and the Unread filter walks partial indexes that contain only unread rows.
- **Aggregates written once.** Subject, snippet, participants, counts, flags and labels of a conversation live in its \`threads\` row, recomputed only when one of its messages changes; message counts per account live in a counter row. Nothing is counted or grouped while you wait.
- **Date-ordered row ids.** Message rowids are allocated from the message date, so "newest first" is the physical order of the table and of the full-text index. Search can walk candidates newest first and stop early.
- **WAL, one writer, pooled readers.** All writes (sync and your actions) go through one writer connection; reads go to a pool of read-only connections (32 MiB page cache and a 1 GiB memory map each), so a backfill never blocks the list or search. Statements are prepared once and cached (\`prepare_cached\`). Copying the WAL back into the database (checkpoints) happens on its own thread, and index maintenance after a backfill runs in short steps, so neither holds up your actions.
- **Bodies compressed, index contentless.** Bodies are stored zstd-compressed (around 4–6× on mail) and decompressed only for the thread you open. The FTS5 table is contentless (\`content=''\`): it holds the inverted index only, and snippets are cut in Rust from the one row being shown.

**Search is an index lookup, then a bounded rank** (\`search.rs\`, \`query.rs\`).
- The query is parsed once into SQL filters plus one FTS5 \`MATCH\`. Operators become indexed conditions: \`from:\`/\`to:\`/\`cc:\` match their own columns of the full-text index, \`has:pdf\`/\`is:unread\`/\`in:sent\` are bits in a flags column, \`label:\` hits a \`(label, message)\` table, and dates are ranges on the date-ordered rowid.
- FTS5 keeps a 3-character prefix index (\`prefix='3'\`), so the half-typed last word is an index lookup, not a scan of the vocabulary. Filenames have their own trigram index for \`filename:\` substrings.
- Where the full-text index drives a search, set filters (\`label:\`, \`is:new-sender\`, \`filename:\`, and \`is:unread\` through a partial index of unread mail) are checked on the index row before the message row is read, so a selective filter costs almost nothing per rejected match.
- A query that matches half the mailbox ("the") scores at most the newest 5,000 candidates (\`MAX_CANDIDATES\`) with BM25 plus recency and sender boosts, so the worst case is bounded instead of growing with the mailbox.
- The UI sends one search per animation frame while you type and ignores responses older than what's on screen, so fast typing never queues work.

**Rendering happens off the UI thread and is cached** (\`crates/penguin-render\`, \`apps/desktop/src/app/store.ts\`).
- Email HTML is sanitized in Rust (ammonia/html5ever) on a blocking worker when a thread is opened, with remote images and trackers stripped in the same pass, then shown in a sandboxed iframe without scripts.
- The UI keeps the last 60 opened conversations (least recently used go first) and prefetches the next three in the direction the cursor is moving and one behind whenever the selection moves, plus the row the pointer rests on, so \`j\`/\`k\` and clicks normally show an already-rendered thread in the same frame as the selection. A selection asks for its thread before React renders it, and until the thread arrives the pane shows its subject from the list row.
- A refetch that changed nothing (a mail-changed after your own action) keeps the cached objects, so the open thread doesn't re-render; a list refresh reuses unchanged row objects, so only rows that changed re-render; a mailbox you switch back to shows its remembered first page at once while the backend confirms it.

**Actions don't wait for anything** (\`src/app/store.ts\`, \`src/app/optimistic.ts\`, \`src/app/actions.ts\`).
- Read, unread, star, labels, archive, trash, move, snooze and Reply Later change the rows, the cached thread and every unread count (sidebar, accounts, profiles, inbox tabs, Reply Later, Snoozed) in the same frame as the key; the backend call runs behind it. Opening an unread conversation marks it read in that same frame (holding \`j\`/\`k\` skims: only the one you stop on is marked).
- Until a call settles, anything read back from the backend (a list page, a thread, the label counts) gets the change re-applied, so a read that raced the call can't flash the old state back. A refused call puts the rows and counts back and shows why.

**The list stays small** (\`src/features/inbox/ThreadList.tsx\`).
- The conversation list is virtualized (\`@tanstack/react-virtual\`): only the rows in view plus a small overscan exist in the DOM however long the list is, and the next page is fetched when you get within 25 rows of the end.
- Moving the selection updates a small external store and scrolls the virtualizer; it doesn't re-render the list.

**The web layer does the mail view's work and little else** (\`apps/desktop/src\`).
- Launch parses and runs only what the mail view needs. Settings, Add account, first-run setup and the calendar are separate chunks (\`src/lib/lazy.ts\`), fetched one at a time in idle moments once the first rows have painted, together with the compose editor, so opening any of them later still renders in the same frame as the key.
- Components subscribe to the narrowest slice of state they draw (\`useSyncExternalStore\` selectors). A row re-renders when its own selection, its row object or the look of a label changes, not when an unread count moves: moving onto unread mail re-renders the two rows whose selection changed, not every row on screen.
- Backend events are coalesced (\`src/app/coalesce.ts\`): a burst of \`mail-changed\` from a backfill becomes at most one list re-read in flight and at most four a second; labels and counts refresh at most once a second during a storm instead of waiting for it to end.`;

// The web layer's efficiency pass: measured on the shared Linux VM (8 cores,
// load 4 to 9 while it ran) against the build before it, in headless Chromium.
const WEB = `## The web layer: what was measured and changed

The frontend was measured as a whole before anything was changed: launch (bundle parse, module evaluation, first render, first layout), \`j\`/\`k\`, search and compose keystrokes, scrolling, re-renders per action, forced layouts, style recalculation, IPC volume under event storms, and memory over a long session. Fixes went to the largest measured costs. Everything is Chromium on the Linux VM against the build before the changes, with the mock backend; see *Not verified in WKWebView* below.

**Tools** (\`scripts/bench-ui\`): \`bench.mjs\` (latencies; \`--dist\` runs a prebuilt bundle and \`--only\` picks sections, for before/after runs; it now also records the JavaScript and CSS fetched before the first rows paint, and a \`mail-changed\` storm), \`renders.mjs\` (which components render for each action, counted through React's DevTools hook the way React DevTools counts them), \`soak.mjs\` (heap, DOM nodes, documents and listeners after each round of a scripted session, optionally a heap-snapshot diff). Main-thread CPU per action came from Chrome traces (thread time, so a loaded machine inflates it less than wall time).

| | Before | After |
|---|---:|---:|
| JavaScript fetched before the first rows (mock chunk excluded) | 870–1,470 KiB | 829 KiB |
| CSS fetched before the first rows | 243 KiB | 200 KiB |
| Main chunk, minified (gzip) | 1,042 KiB (316) | 771 KiB (240) |
| Main module evaluation at launch (median of 4) | 90 ms | 77 ms |
| Load → first rows in the DOM, p50 (3 × 20 alternating launches, load ≈ 4) | 363 ms | 336 ms |
| Components rendered: one \`j\` onto unread mail | 585 | 321 |
| Components rendered: ten \`j\` | 4,408 | 2,145 |
| Components rendered: archive with \`e\` | 1,515 | 804 |
| Main-thread CPU per \`j\` (6 × 30 presses) | 53 ms | 50 ms |
| … of it JavaScript | 25.9 ms | 22.9 ms |
| List re-reads during an 8 s \`mail-changed\` storm (event every 100 ms, ~1,000 rows loaded) | 77–81 | 32–33 |
| Rows re-read during that storm (a 1,000-row page is 433 KiB of JSON) | 77,000–81,000 | 32,000–33,000 |

What changed, and why:
- **Rows re-rendered on every unread-count change.** Every \`ThreadRow\` (and the reading pane) subscribed to \`meta.labels\` to draw label chips, and marking a conversation read replaces that array, so each \`j\` onto unread mail re-rendered every row on screen. They now subscribe to \`meta.labelLook\`, which changes only when a label is added, removed, renamed, recolored or hidden. \`useSyncExternalStore\` re-renders when the selected snapshot changes by \`Object.is\` [1], and \`memo\` only helps when props and subscribed state stay the same [2]; sidebar label rows are memoized too.
- **Launch parsed code for screens most sessions never open.** Settings (with its pages, rules and diagnostics), Add account, first-run setup and the calendar left the main chunk; Vite splits the CSS of each chunk with it and loads it before the chunk resolves, so nothing renders unstyled [3]. They load with a small helper instead of \`React.lazy\`, which suspends on first render while the code loads [4] and so would cost a frame even after a prefetch; once fetched, a screen renders synchronously. The fetches wait until the first rows have painted and run one at a time in idle callbacks, so none becomes a long task in the way of input [5][6]. WebKit has no \`requestIdleCallback\` [7]; there a short timeout stands in. The compose editor, previously fetched by \`requestIdleCallback\` during launch's IPC waits, before the first rows in Chromium, now joins the same queue. JavaScriptCore, like V8, has to parse every function and starts running it in its interpreter [8], so code that isn't loaded costs nothing. Search, compose and the command palette stay in the main chunk: \`/\`, \`c\` and ⌘K work from the first frame.
- **Backfill event storms re-read the list once per event.** Each \`mail-changed\` scheduled a list re-read 60 ms later (up to 1,000 rows, parsed and diffed on the main thread), and re-reads could overlap; labels were debounced, so a steady storm starved the unread counts until it ended. Tauri events are JSON messages not meant for high throughput [9][10]; \`src/app/coalesce.ts\` keeps one re-read in flight and starts them at least 250 ms apart (the first after a quiet spell still at 60 ms), and counts at most once a second.

Measured and left alone:
- **Compose typing** paints each character in the frame after the key (16 ms p50, 27 ms p95; the editor DOM changes 2.5 ms after the key), about 3 ms of JavaScript per key.
- **Search typing** re-renders the results panel for each response, about 2.5 ms of React work per key; two layout reads per key (keeping the query's highlight overlay scrolled with the input, and keeping the selected result in view) force layout the frame needs anyway [12].
- **CSS.** \`will-change\` appears once (toasts), \`:has()\` six times on small subtrees, no \`backdrop-filter\` on the desktop. Style recalculation per \`j\` touches at most ~130 elements. The list is virtualized, so \`content-visibility\` [11] has nothing off screen to skip there.
- **Memory.** Twenty rounds of scripted use (about five minutes: triage, open and close, searches, a reply, view switches, scrolling): JS heap 8.5 → 15.8 MiB, rising 2.2 MiB after round 3, of which 1.9 MiB is compiled code; DOM nodes (~3,200), documents (20) and event listeners (~470) stay flat. No leak found.
- **Re-renders elsewhere.** Idle with sync ticking re-renders two components per tick; scrolling renders only rows entering the viewport; search and compose render only their own panels.

Not verified in WKWebView: every number here is Chromium's. Parse and evaluation cost, the idle timeout that stands in for \`requestIdleCallback\`, frame timing and the iframe cost differ in WebKit; the byte counts, render counts and IPC counts carry over.

Sources: [1] [react.dev: useSyncExternalStore](https://react.dev/reference/react/useSyncExternalStore) · [2] [react.dev: memo](https://react.dev/reference/react/memo) · [3] [Vite: CSS code splitting and async chunk loading](https://vite.dev/guide/features) · [4] [react.dev: lazy](https://react.dev/reference/react/lazy) · [5] [web.dev: Optimize long tasks](https://web.dev/articles/optimize-long-tasks) · [6] [web.dev: Interaction to Next Paint](https://web.dev/articles/inp) · [7] [caniuse: requestIdleCallback](https://caniuse.com/requestidlecallback) · [8] [WebKit: Speculation in JavaScriptCore](https://webkit.org/blog/10308/speculation-in-javascriptcore/) · [9] [Tauri: Inter-Process Communication](https://v2.tauri.app/concept/inter-process-communication/) · [10] [Tauri: Calling the frontend from Rust (events vs channels)](https://v2.tauri.app/develop/calling-frontend/) · [11] [web.dev: content-visibility](https://web.dev/articles/content-visibility) · [12] [web.dev: Avoid large, complex layouts and layout thrashing](https://web.dev/articles/avoid-large-complex-layouts-and-layout-thrashing).`;

const REPRO = `## Reproduce

\`\`\`sh
scripts/bench.sh                  # 10k, 100k and 300k corpora + UI, then regenerate this file and README
scripts/bench.sh --corpus 100k    # one corpus
scripts/bench.sh --no-ui          # backend only
node scripts/bench-report.mjs     # rebuild the docs from docs/perf/*.json
\`\`\`

Or one corpus directly: \`cargo run --release -p penguin-core --example bench -- --corpus 100k\` (JSON to \`target/bench/\`). The UI part needs \`npm ci\` in \`apps/desktop\` and Playwright's headless shell (\`cd scripts/bench-ui && npm ci && npx playwright-core install chromium-headless-shell\`).

The corpus is generated from a fixed seed (fictional people and companies at \`.example\` domains, Zipf-distributed vocabulary, quoted replies, ESP-style newsletters, attachment metadata, three accounts, eight years of mail growing toward the present), so two runs on the same commit index the same mail. Dates are relative to the time of the run. The 300k run needs about 3 GB of free disk while it runs and deletes the database afterwards.

**On a Mac.** Run the same \`scripts/bench.sh\`. Results go to \`docs/perf/macos-<arch>-*.json\` next to the Linux ones, this file gets a section per machine, and the README quotes the Mac. The cold-open rows need Linux's \`posix_fadvise\` and read "n/a" on macOS; a cold number there needs \`sudo purge\` right before opening the database, which the script doesn't do. The UI numbers stay Chromium; timing the real WKWebView app needs a signed build and is not automated here. Close other heavy apps first and plug in the laptop.

**Comparing with other clients.** These numbers are Penguin's own and aren't comparable to figures published for other apps, which measure different things on different machines. A fair comparison indexes the same mail in each client on the same machine. The synthetic corpus only exists inside Penguin's store (there is no export to mbox or IMAP yet), so the fair comparison available today is timing the same tasks (search a term, open the 50th conversation, archive ten in a row) in each app on the same real account and the same Mac, for example from a screen recording.`;

// The database layer's efficiency pass: examples/bench_sql.rs on the 300k
// corpus, before/after on the shared Linux VM (load 6 to 9), medians of 3.
const DB = `## The database layer: what was measured and changed

Every query path in penguin-core was timed on the 300,000-message corpus, then profiled statement by statement: \`examples/bench_sql.rs\` runs each list view (mailboxes, Reply Later and Follow up, every smart view, a saved search, 40-page scrolls), the sidebar counts and badges, thread open, keyword search and every keystroke of typed strings, Ask, the actions' writes, sync's write path into the corpus and into a new database, and table sizes. Built with \`--features sql-profile\`, every connection reports each finished statement through SQLite's \`SQLITE_TRACE_PROFILE\` hook [12] with its full-scan, sort and automatic-index counters and its \`EXPLAIN QUERY PLAN\` [4], so a table scan or temporary B-tree shows up next to the time it costs. Fixes went to the paths whose statements scanned, ran once per row, or ran in a loop. Before and after are the same corpus copied fresh for each run, three runs each, alternating builds, on the shared Linux VM (load 6–9 throughout); the numbers are the median of the three runs' p50 / p95, in ms.

| Path (300,000 messages) | Before | After |
|---|---:|---:|
| Snoozed list (nothing snoozed) | 27.7 / 52.1 | 0.05 / 0.09 |
| \`has:invite\` (no invitations in the corpus) | 127 / 175 | 0.02 / 0.04 |
| Follow up list (986 conversations) | 60.9 / 98.4 | 20.6 / 34.5 |
| Follow up, unread only | 65.2 / 92.5 | 20.5 / 28.7 |
| Reply Later and Follow up badges (\`triage_counts\`) | 55.1 / 81.6 | 15.3 / 22.9 |
| Smart view badges, all views (\`smart_counts\`) | 17.5 / 31.5 | 12.4 / 16.9 |
| Files (300 rows) | 3.65 / 5.02 | 2.23 / 2.84 |
| Files: spreadsheets | 5.69 / 7.21 | 2.69 / 5.26 |
| Receipts | 3.81 / 5.31 | 2.52 / 3.23 |
| Search with no match (\`zzqxjv\`) | 2.09 / 3.01 | 0.39 / 0.46 |
| Search \`INV-20417\` | 2.99 / 4.16 | 1.32 / 1.83 |
| Search \`the pdf mike sent about the lease date:"last spring"\` | 8.97 / 10.1 | 3.29 / 5.02 |
| Typing \`priya budget\`, per keystroke | 14.2 / 29.2 | 9.5 / 21.4 |
| Search, all 33 shapes and typing pooled | 21.1 / 85.5 | 15.3 / 74.5 |
| Ask, all 20 questions pooled | 34.3 / 70.2 | 24.8 / 59.1 |
| Ask "who is <name>" | 32.3 / 55.6 | 25.6 / 31.2 |
| Sync write path into a new database, CPU per message | 0.71 | 0.65 |

Search relevance did not move: \`penguin-eval\` gives identical nDCG@10, MRR and recall on all 200 queries before and after, in keyword and Ask mode. (Hybrid with the hash embedder varies by ±0.001 between two runs of the same build, and the before/after difference is within that.)

What changed, and why:
- **A join order the planner got wrong.** The Snoozed view and \`is:snoozed\` joined \`snoozes\` to \`threads\`. An empty table gets no \`sqlite_stat1\` row, so next to 140,000 analyzed threads the planner scanned every thread and probed snoozes for each. \`CROSS JOIN\` is SQLite's documented way to fix the loop order [1]; \`is:snoozed\`'s rowid list went from 233 to 0.08 ms.
- **A filter that walked the mailbox.** \`has:invite\` was an \`EXISTS\` per message, so a filter-only search probed attachments for every message newest first. As \`rowid IN (SELECT message_rowid FROM attachments WHERE kind = 6)\` it matches the WHERE of the partial index \`attachments_calendar\`, which SQLite requires before it will use one [3], and reads only calendar parts.
- **N+1 queries in Follow up.** Each thread you sent to in the last 60 days cost up to six statements (latest message, dismissal, snooze, invitation, recipients, bulk count). Now one statement per account reads the candidates joined to their messages in the index's own order, so nothing is sorted; dismissals, snoozes and invitation messages are read once as sets; and a thread whose newest live message is more than \`ROWID_SPILL_LIMIT\` past your newest sent one (no draft) is ruled out from its thread row alone. A randomized test compares the result with the per-thread rules on same-millisecond replies, trash, spam, drafts and newsletters, and fails when the shortcut is loosened.
- **One query per row for lists built in Rust.** Follow up and the smart views read each row's thread summary with its own statement (up to 500). \`summaries_by_rowid\` reads them all with one statement over a JSON rowid list.
- **A covering index where the Files walk read rows.** \`messages_attach\` is now \`(date, flags, thread_rowid, account_id) WHERE flags & 512 != 0\`, so the newest-first walk over up to 3,000 attachment messages never visits the message rows [2]. Measured alone on the same build: Files 3.4 → 2.2 ms, spreadsheets 5.6 → 2.4 ms. It costs 0.9 MiB at 300,000 messages (0.8 → 1.7 MiB) and nothing measurable on ingest (17% of messages have attachments).
- **Badges that built the list to count it.** The Files and People-you-know counts built every row (file names, participants, snooze state) and then took the length. The Files count now walks the same index without names and counts unread threads; People you know is a \`count(*)\` over the list's own WHERE.
- **People lookups that scanned every correspondent.** Search's People group, \`from:\` suggestions and chips, the boost for people you write to, hybrid search's "is every word a name?" and Ask's person resolution each ran \`search_key LIKE '% word%'\`. A leading wildcard can't use an index [13], so each was a scan of \`people\`, several times per keystroke and per question. \`people_words(word, email)\` (\`WITHOUT ROWID\`, plus an email index) holds each person's words; a prefix is the range \`word BETWEEN tok AND tok || U+10FFFF\`, so the matches are the same. At 4,007 people one word went from 0.9 to 0.06 ms and the People panel query from 1.0–1.6 to 0.2 ms; real address books are larger and the scan grew with them. The writer that sets \`search_key\` keeps the words in step; triggers were tried first and dropped, because any trigger on \`people\` made sync's write path ~40% slower (0.74 → 1.1 ms CPU per message into a new database). The migration fills the table in 0.1 s at 300,000 messages (1.5 MiB).
- The two migrations together take 0.26–0.37 s on first launch at 300,000 messages (most of it rebuilding \`messages_attach\`); later launches open in 3–6 ms as before.

Measured and not changed:
- **\`PRAGMA optimize\` on every open.** SQLite recommends \`PRAGMA optimize=0x10002\` when a long-lived connection opens [5]. Measured here it hurt twice. That mask leaves out bit 0x10, the one that applies an analysis limit, so it ran a full \`ANALYZE\` (1.0–2.4 s added to the first open at 300,000 messages) and wrote \`sqlite_stat4\` samples; with STAT4 compiled in, the planner re-plans for the values bound to each statement [6], and those samples made Ask's "what am I waiting on" 2.4× slower (45 → 111 ms) and "what do I owe replies to" 1.7× slower until the samples were deleted. On a new, empty database it analyzed empty tables and the first sync then ingested 11× slower (0.68 → 7.8 ms CPU per message). \`Store::optimize\` already runs the default, limited \`PRAGMA optimize\` after a backfill, when the tables are full, and that stays the only place statistics are gathered.
- **A larger prepared-statement cache.** rusqlite keeps 16 statements per connection by default [11]. Raising it to 256 made no measurable difference, either to search (15.1 vs 15.4 ms pooled p50, two runs each) or to sync's write path (0.63 vs 0.63 ms CPU per message, three runs each), so the default stays.
- **The connection setup.** WAL with one writer and pooled read-only readers is SQLite's own concurrency model: readers don't block the writer and the writer doesn't block readers [7], so a backfill never stalls a list or a search; checkpoints run on their own thread (\`store_checkpoint.rs\`). \`synchronous = NORMAL\` is safe from corruption under WAL (a power cut can roll back the last transactions) [8]; \`temp_store = MEMORY\` keeps sorter spills off disk; the 1 GiB memory map [9] and 32–64 MiB page caches were measured before (see *Known slow paths*). Nothing here was changed.
- **FTS5.** The index uses \`detail=full\` (phrases and \`NEAR\` need positions; \`column\` or \`none\` would shrink it 2–5× and drop them [10]), \`prefix='3'\`, \`automerge=8\`, and \`Store::optimize\` merges it in steps with FTS5's \`merge\` command [10]. BM25 reads each candidate's \`_docsize\` row, about 5,000 lookups for a broad query; the profile puts them at a few milliseconds per search, the price of ranking and bounded by \`MAX_CANDIDATES\`.
- **Ask's waiting-on and top-sender questions** scan 60 days or a year of message rows and decompress the bodies of candidate threads (~45 ms each). A covering index for them would add bytes to every message insert for two questions.

Sources: [1] [SQLite: Manual control of join order with CROSS JOIN](https://www.sqlite.org/optoverview.html#manual_control_of_query_plans_using_cross_join) · [2] [SQLite: Covering indexes](https://www.sqlite.org/optoverview.html#covering_indexes) · [3] [SQLite: Partial indexes, when the planner can use one](https://www.sqlite.org/partialindex.html) · [4] [SQLite: EXPLAIN QUERY PLAN](https://www.sqlite.org/eqp.html) · [5] [SQLite: PRAGMA optimize](https://www.sqlite.org/pragma.html#pragma_optimize) and [analysis_limit](https://www.sqlite.org/pragma.html#pragma_analysis_limit) · [6] [SQLite: sqlite3_prepare_v2 re-plans for bound values with STAT4](https://www.sqlite.org/c3ref/prepare.html) and [the sqlite_stat4 table](https://www.sqlite.org/fileformat2.html#the_sqlite_stat4_table) · [7] [SQLite: Write-Ahead Logging](https://www.sqlite.org/wal.html) · [8] [SQLite: PRAGMA synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous) · [9] [SQLite: Memory-mapped I/O](https://www.sqlite.org/mmap.html) · [10] [SQLite: FTS5 (detail, prefix, automerge, merge, optimize, bm25)](https://www.sqlite.org/fts5.html) · [11] [rusqlite: Connection::prepare_cached and set_prepared_statement_cache_capacity](https://docs.rs/rusqlite/0.37.0/rusqlite/struct.Connection.html#method.prepare_cached) · [12] [SQLite: sqlite3_trace_v2 and SQLITE_TRACE_PROFILE](https://www.sqlite.org/c3ref/c_trace.html) · [13] [SQLite: The LIKE optimization](https://www.sqlite.org/optoverview.html#the_like_optimization).`;

const SYNC = `## Sync: what was measured and changed

The network half of sync, measured against in-process servers with the real sync engines: the scripted IMAP server (\`crates/penguin-imap/src/testserver.rs\`) over loopback TCP, holding each reply for an emulated round trip (\`PENGUIN_BENCH_RTT_MS\`) and pacing it to an emulated link (\`PENGUIN_BENCH_MBPS\`); the in-memory Graph (\`fake_graph.rs\`) behind a fixed response time; an in-process Gmail. The mail is the benchmark corpus encoded the way each server sends it (RFC 5322 with real base64 attachments up to 256 KB each, Gmail \`format=full\` JSON, Graph JSON). Medians of three runs on the shared Linux VM; before is the sync code as it was, after is this branch.

| IMAP backfill, 2,000 messages (server offers COMPRESS=DEFLATE) | Before | After |
|---|---:|---:|
| Bytes on the wire per message | 77,103 | 4,288 |
| 50 Mbit/s link: messages/s | 78.5 | 451 |
| 30 ms RTT and 50 Mbit/s: messages/s | 74.4 | 347 |
| 30 ms RTT, unlimited link: messages/s | 480 | 445 (607 without COMPRESS) |
| Loopback, unlimited: messages/s | 665 | 619 (1,057 without COMPRESS) |

| IMAP poll of 37 folders, nothing changed | Before | After |
|---|---:|---:|
| 30 ms RTT, QRESYNC server | 2,374 ms | 128 ms |
| 30 ms RTT, CONDSTORE only (Gmail's IMAP) | 2,366 ms | 130 ms |
| 30 ms RTT, neither | 2,369 ms | 161 ms |
| Commands per poll | 75 | 39 (in 2–3 round trips) |

| Other paths | Before | After |
|---|---:|---:|
| Graph backfill, 2,000 messages at 50 ms per request: requests | 2,441 | 461 |
| Graph backfill: messages/s | 50.5 | 187 |
| IMAP MIME parse, µs per message | 425 | 362 |
| IMAP MIME parse, allocations per message | 208 | 131 |

What changed, and why:
- **IMAP: big mail without its attachments.** A message over 128 KB is almost always big because of attachments, which sync never needs (Gmail's API path never downloads them either; they come on demand). Backfill asks for BODYSTRUCTURE with the ids, so such a message comes as its header and text parts, grouped by MIME shape, every group's FETCH written in one go: pipelining, which RFC 3501 §5.5 allows for commands that don't depend on each other [1]. Text of mail up to 2 MB comes uncapped, so the stored message is the same as a whole download would give (a test compares them field by field). This is most of the byte saving and all of the gain on a slow link.
- **IMAP: COMPRESS=DEFLATE.** RFC 4978 [2] puts one DEFLATE context per direction under TLS for the connection's life; its authors measured typical responses at 25–40% of their size, and the corpus's text parts shrink ~3×. Penguin turns it on after login when the server offers it, as Thunderbird does by default [11], and not for servers on this Mac or the local network, where bandwidth is plentiful and inflating (~4 µs per KB, measured) is pure cost. The unlimited-link rows are slower with it because the in-process test server deflates on the same CPU; on a real link the server does that work and the link is the limit.
- **IMAP polls: STATUS instead of EXAMINE.** A full poll (every 5 minutes) opened every folder twice. Now one pipelined round trip asks every folder's STATUS (MESSAGES, UIDNEXT, UIDVALIDITY and, with CONDSTORE, HIGHESTMODSEQ) and a folder whose numbers match what is stored is skipped: an unchanged HIGHESTMODSEQ means nothing in it changed (RFC 7162 §3.1.2.1 [3]; QRESYNC servers also bump it on expunge, §3.2). Without CONDSTORE flag changes don't show in STATUS, so those folders are only skipped between their flag scans, as before. The selected folder still gets EXAMINE, as RFC 3501 §6.3.10 asks [1]. New mail in INBOX still arrives through IDLE [4] without waiting for a poll.
- **Graph: bodies by the page, not by the message.** Outlook limits each app to 10,000 requests per 10 minutes and 4 at a time per mailbox, whatever they return [5], so one GET per message capped backfill at ~14 messages/s. The first delta round is ordered newest first (delta supports \`$select\`, \`$filter\` and \`$orderby\` on \`receivedDateTime\` [6]), so the bodies a page needs are a run of one folder by date: they come from one folder listing of that range with the same \`$select\`/\`$expand\` as the single GET. At the request limit the ceiling goes from ~14 to ~72 messages/s. Anything the listing misses, a sparse range, a plain delta, incremental rounds or a refused listing go one by one as before; a test checks both paths store identical messages.
- **MIME: headers parsed once, snippets read only what they keep.** IMAP's parser parsed a whole message, then its header block again, and unfolded the header fields twice; the whole-message parse now hands its headers over. Every provider cut its 200-character snippet by splitting and re-joining the whole body (megabytes for a big newsletter); one shared \`snippet\` stops at 200 characters.

Measured and not changed:
- **Gmail batching.** A batch of n calls counts as n against the quota [7], and Penguin's measured cost of \`messages.get(format=full)\` (~60 units) holds a mailbox near 100 messages per minute, which a few concurrent requests over one HTTP/2 connection already reach. Batching would save round trips the quota doesn't let us use.
- **Gmail wire format.** Responses are already gzip (reqwest's \`gzip\` feature sends \`Accept-Encoding\` and decodes [8]; Google asks for "gzip" in the User-Agent too [9]): 16,034 → 5,877 bytes per message on the corpus. \`format=full\` is the cheapest format that has bodies; \`fields=\` masks don't change the cost [10]. Decoding a message costs ~64 µs, 23 of them JSON.
- **Connections.** Gmail and Graph each share one reqwest client per process: pooled keep-alive connections, HTTP/2 multiplexing with Google [8]; each IMAP account keeps one connection for sync and one for IDLE, with TCP_NODELAY.
- **Sync and UI reads.** During a Gmail backfill running with no quota (1,496 messages/s, far past any real one), the UI's Inbox read took 0.53 ms p50 and 1.8 ms p95 (0.24 / 0.40 ms idle): WAL readers don't wait for the writer. A UI write waited up to 53 ms p95 behind a 100-message sync transaction; 25-message transactions brought that to 21–32 ms for ~12% less unthrottled ingest (1,496 → ~1,300 messages/s). That is the store's side (write batching is owned by the database work), noted here as the lever.

Reproduce: \`scripts/box-cargo.sh test --release -p penguin-imap --lib bench_ -- --ignored --nocapture --test-threads=1\` (with \`PENGUIN_BENCH_RTT_MS\`, \`PENGUIN_BENCH_MBPS\`, \`PENGUIN_BENCH_CAPS=COMPRESS=DEFLATE\`), and the same for \`-p penguin-graph\` and \`-p penguin-gmail\`.

Sources: [1] [RFC 3501 (IMAP4rev1): §5.5 multiple commands in progress, §6.3.10 STATUS](https://www.rfc-editor.org/rfc/rfc3501) · [2] [RFC 4978: The IMAP COMPRESS Extension](https://www.rfc-editor.org/rfc/rfc4978) · [3] [RFC 7162: IMAP CONDSTORE and QRESYNC](https://www.rfc-editor.org/rfc/rfc7162) · [4] [RFC 2177: IMAP4 IDLE](https://www.rfc-editor.org/rfc/rfc2177) · [5] [Microsoft Graph throttling limits: Outlook service limits](https://learn.microsoft.com/en-us/graph/throttling-limits#outlook-service-limits) · [6] [Microsoft Graph: message delta](https://learn.microsoft.com/en-us/graph/api/message-delta) and [JSON batching](https://learn.microsoft.com/en-us/graph/json-batching) · [7] [Gmail API: batching requests](https://developers.google.com/workspace/gmail/api/guides/batch) · [8] [reqwest ClientBuilder (gzip, pooling, HTTP/2)](https://docs.rs/reqwest/0.12/reqwest/struct.ClientBuilder.html) · [9] [Google APIs: performance tips (gzip)](https://developers.google.com/workspace/gmail/api/guides/performance) · [10] [Gmail API: usage limits](https://developers.google.com/workspace/gmail/api/reference/quota) and [synchronizing clients](https://developers.google.com/workspace/gmail/api/guides/sync). · [11] [Thunderbird mailnews.js: \`mail.server.default.use_compress_deflate\`](https://searchfox.org/comm-central/source/mailnews/mailnews.js)`;

function caveats() {
  const L = machines.get(slowPathTag);
  const [bigSize, big] = [...L.backend.entries()].pop();
  const bigIng = [...L.backend.entries()].filter(([, x]) => x.ingest).pop();
  const q = (text) => big.search.queries.find((x) => x.query === text)?.ms;
  const qms = (text) => {
    const x = q(text);
    return x ? `${ms(x.p50)} ms` : "–";
  };
  const worstType = [...big.asYouType.strings].sort((x, y) => y.ms.p95 - x.ms.p95)[0];
  const worstAsk = [...big.ask.questions].sort((x, y) => y.ms.p50 - x.ms.p50)[0];
  const render2mb = big.render?.find((x) => x.name.startsWith("2 MB newsletter"));
  return `## Caveats

- **Synthetic mail.** Real mailboxes have longer threads, more languages, bigger newsletters and messier HTML. The generator aims for realistic shapes (see the corpus table) but it is not your inbox.
- **Warm, in-process numbers.** Backend timings are in-process calls in a release build with the database in the OS cache (except the cold rows). The app adds the Tauri IPC hop and JSON serialization of the response, and the UI's own work (measured separately above).
- **Ingest order.** The main ingest numbers write mail in the generator's order, random by date; a Gmail backfill arrives newest first. The arrival-order side test under Ingest measures both.
${tags.some((t) => [...machines.get(t).backend.values()][0]?.machine.virtualized) ? `- **A shared cloud VM.** The Linux numbers come from a virtual machine that other jobs were using at the same time (load averages above). Medians are stable between runs; tails are not. Per-core speed differs from Apple silicon, so expect different absolute numbers on a Mac.
` : ""}- **Chromium is not WKWebView.** See the UI section.

## Known slow paths

What is still slow, measured on the Linux machine above, and why.

- **Ingest is ~${ms(bigIng?.[1].ingest.cpuMsPerMessage)} ms of CPU per message at ${sizeLabel(bigIng?.[0] ?? bigSize)} messages (on the writing thread), most of it SQLite's page traffic.** Each message touches a leaf page in about a dozen B-trees at a position set by its date, so a 100-message batch dirties ~1,500 pages (6 MiB of WAL); the FTS5 index adds a new segment per batch that later merges rewrite. Tokenizing for the full-text index (with its 3-character prefix index) is about 10% of the time, zstd about 7%, thread aggregates and view rows about 2%. Gmail's quota (≈50 messages/s per account) hides all of it; an IMAP backfill isn't quota-limited the same way, and at ${int(bigIng?.[1].ingest.lastTenthMessagesPerSec)} messages/s in the slowest stretch the store is still well ahead of any server.
- **Index maintenance still has a few long steps.** \`Store::optimize\` merges the index in steps of 256 pages and lets queued writes go between them, but FTS5 only ends a step between terms, so the steps that reach the most common terms (every address here ends in \`.example\`; real mail has \`com\` and \`the\`) write a whole posting list at once. ${(() => {
    const w = bigIng?.[1].ingest.writeWaitDuringOptimizeMs;
    return w?.n ? `At ${sizeLabel(bigIng[0])} messages a write issued during the merge waited ${ms(w.p50)} ms at the median, ${ms(w.p99)} ms p99 and ${ms(w.max)} ms at worst (${w.n} writes).` : "";
  })()}
- **A common word with a broad filter walks deep.** Search scores the newest 5,000 matches that pass the filters, so a filter that passes a third of the mail (\`account:personal invoice\` ${qms("account:personal invoice")}, \`in:sent budget\` ${qms("in:sent budget")} at ${sizeLabel(bigSize)} messages) walks three times as far into the word's matches, reading each message row to test it. A rowid set for such a filter would cost more to build than it saves; selective filters (\`label:\`, \`is:unread\`, \`is:new-sender\`) avoid the reads. Thread-state filters (\`is:replied\`, \`is:unanswered\`) check the thread per candidate and aren't in this benchmark's query set; with a common word they are the slowest shapes (~80 ms here; \`examples/bench_sql.rs\` times \`is:unanswered budget\`).
- **\`domain:\` and other typed-out addresses are phrase prefixes.** \`domain:acme.example\` searches the phrase \`acme example*\` (the last word is prefix-matched while typing), and FTS5 merges the posting lists of every term starting with \`example\` before it can check the phrase, twice (a bounded count decides whether to rank by BM25, then the ranked walk): ~80 ms here, where every address ends in \`.example\`; a real \`.com\` behaves the same.
- **Huge newsletters.** A 2 MB newsletter takes ${render2mb ? ms(render2mb.ms.p50) : "–"} ms in penguin-render on this CPU (~30 ms on an Apple M5 Pro), most of it html5ever parsing and ammonia's tree walk. penguin-render's release speed tests budget it relative to ammonia's own default sanitizer on the same input (\`cargo test -p penguin-render --release --test perf\`), so they hold on slow CI machines. Typical newsletters (50 KB) take a few milliseconds.
- **Cold launch on this VM reads far more than it needs, through the memory map.** Readers map the file (1 GiB). With the file out of the OS cache, each page fault on the map reads the disk's whole readahead window around it, and this VM's disk reads ahead 8 MiB: ${(() => {
    const c = big.open.cold;
    return c.pageCacheEvicted ? `${ms(c.timeToFirstListMs.p50)} ms to the first Inbox page and ${ms(c.firstSearchMs.p50)} ms for the first search at ${sizeLabel(bigSize)} messages (${ms(big.open.warm.timeToFirstListMs.p50)} and ${ms(big.open.warm.firstSearchMs.p50)} ms warm)` : "see *Opening the database*";
  })()}. Reading with pread instead was measured: the cold first Inbox page of the 100k database dropped from ~320 ms to ~15 ms, but searches that read many message rows got twice as slow at 300k (\`account:personal invoice\` 53 → 113 ms, \`in:sent budget\` 53 → 104 ms), because every page outside SQLite's 32 MiB cache then costs a syscall. That cost is paid on every such search, the cold one once per boot and mostly because of this VM's readahead (a typical 128 KiB window reads at most ~10 MiB for the first page), so the map stays. macOS clusters page-ins differently: measure a cold start there with \`sudo purge\`.
- **Each conversation's body is a new sandboxed iframe.** Of the ~55 ms from \`j\` to the message body painted in Chromium, most is the \`srcdoc\` document being created, parsed, laid out and measured for its height: on this VM, reaching the new frame's document costs ~3 ms and measuring its height ~2.5 ms of forced layout per press. Reusing one frame across conversations would save part of it, but the frame is the boundary that keeps hostile mail HTML away from the app, so it stays one fresh frame per conversation until it is measured in WKWebView.
- **The first layout at launch** takes 30–50 ms in Chromium on this VM for under 200 layout objects, fonts or not: a fresh renderer's one-time setup rather than anything in Penguin's CSS.
- **Typing and Ask.** At ${sizeLabel(bigSize)} messages the slowest string to type was \`${worstType.typed}\` (${ms(worstType.ms.p50)} ms p50, ${ms(worstType.ms.p95)} ms p95 per keystroke); the UI sends at most one search per frame and drops stale answers, so a slow keystroke delays results rather than queueing work. The slowest Ask question is "${worstAsk.template ?? worstAsk.question}" (${ms(worstAsk.ms.p50)} ms p50, ${ms(worstAsk.ms.p95)} ms p95)${worstAsk.ms.p95 >= 100 ? ", over the 100 ms p95 target \`examples/bench_ask.rs\` checks" : ", under the 100 ms p95 target \`examples/bench_ask.rs\` checks"}.`;
}

// ----- document -----
const sections = [];
sections.push(`# Performance

<!-- Generated by scripts/bench-report.mjs from docs/perf/*.json. Edit the script, not this file. -->

Penguin keeps the whole mailbox in a local SQLite database and answers every interaction from it: search, lists, opening a conversation, Ask. This page has the measured cost of each of those on synthetic mailboxes of ${[...machines.get(primaryTag).backend.keys()].map(sizeLabel).join(", ")} messages, how it was measured, and why it is fast. Everything here is reproducible with \`scripts/bench.sh\`.`);
for (const tag of tags) {
  const { backend } = machines.get(tag);
  const b0 = [...backend.values()][0];
  const title = b0 ? `${b0.machine.cpu} (${b0.machine.os})` : tag;
  sections.push(`## Results: ${title}\n\n${machineSection(tag)}\n\n### Headline\n\n${headline(tag)}`);
  sections.push(`### The corpus\n\n${corpusSection(tag)}`);
  sections.push(backendSections(tag));
  const u = uiSection(tag);
  if (u) sections.push(`### UI: the web layer in headless Chromium\n\n${u}`);
}
// Hand-written, measured outside bench.sh: launch, memory and idle wakeups
// of the whole app process (docs/perf/app-process.md).
sections.push(readFileSync(join(perfDir, "app-process.md"), "utf8").trim());
sections.push(HOW);
sections.push(WEB);
sections.push(DB);
sections.push(SYNC);
sections.push(caveats());
sections.push(REPRO);
writeFileSync(join(root, "docs/PERFORMANCE.md"), sections.join("\n\n") + "\n");

// ----- README block -----
function readmeBlock() {
  const { backend } = machines.get(primaryTag);
  const sizes = [...backend.keys()];
  const big = backend.get(sizes[sizes.length - 1]);
  const n = int(big.corpus?.messages ?? sizeKey(sizes[sizes.length - 1]));
  // The README is a front page: four medians in a table, the rest in PERFORMANCE.md.
  const lines = [];
  lines.push(`| Search ${n} messages | Search as you type | Open a conversation | Answer a question |`);
  lines.push("|:---:|:---:|:---:|:---:|");
  lines.push(`| **${ms(big.search.pooledMs.p50)} ms** | **${ms(big.asYouType.pooledMs.p50)} ms** a keystroke | **${ms(big.thread.totalMs.p50)} ms** | **${ms(big.ask.pooledMs.p50)} ms** |`);
  lines.push("");
  lines.push(`Medians on a synthetic ${n}-message mailbox on ${machineShort(big.machine)}, release build, commit ${big.git?.commit ?? "?"}. How it's measured, every number and why it's fast: [docs/PERFORMANCE.md](docs/PERFORMANCE.md).`);
  return lines.join("\n");
}
const readmePath = join(root, "README.md");
const readme = readFileSync(readmePath, "utf8");
const start = "<!-- perf:start -->";
const end = "<!-- perf:end -->";
if (readme.includes(start) && readme.includes(end)) {
  const next = readme.slice(0, readme.indexOf(start) + start.length) + "\n" + readmeBlock() + "\n" + readme.slice(readme.indexOf(end));
  writeFileSync(readmePath, next);
} else {
  console.error("README.md has no <!-- perf:start --> / <!-- perf:end --> markers; skipped the README block.");
}
console.error("wrote docs/PERFORMANCE.md" + (readme.includes(start) ? " and README.md" : ""));
