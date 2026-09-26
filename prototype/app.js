const $ = (s) => document.querySelector(s);
const esc = (s) =>
  String(s).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const paths = {
  folder: "M3 7V5h6l2 2h10v13H3Z",
  file: "M6 3h8l4 4v14H6Z M14 3v5h4",
  home: "m3 11 9-8 9 8 M5 10v11h5v-7h4v7h5V10",
  search: "M16 16l5 5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
  left: "m14 5-7 7 7 7",
  right: "m10 5 7 7-7 7",
  up: "m5 14 7-7 7 7",
  plus: "M12 4v16 M4 12h16",
  close: "m6 6 12 12 M6 18 18 6",
  drive: "M4 4h16v16H4Z M4 14h16 M16 17h1",
  airdrop: "m12 13-5 8h10Z M8 14a6 6 0 1 1 8 0 M5 17a10 10 0 1 1 14 0",
  clock: "M12 7v5l4 2 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
  download: "M12 3v12 m-5-5 5 5 5-5 M4 16v5h16v-5",
  desktop: "M3 4h18v13H3Z M8 21h8 M12 17v4",
  cloud: "M6 18a5 5 0 1 1 1-10 6 6 0 0 1 11 1 4.5 4.5 0 1 1 0 9Z",
  activity: "M3 3v18h18 M4 14h4l3-8 4 12 3-6h3",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8 M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1Z",
  sliders: "M4 6h16 M4 18h16 M4 12h16 M8 3v6 M16 9v6 M10 15v6",
  help: "M9 8a3 3 0 1 1 5 2c-2 1-2 2-2 4 M12 17h.01 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
  edit: "m4 16 12-12 4 4L8 20H4Z",
  cut: "m9 9 11 11 M9 15 20 4 M9 6a3 3 0 1 1-6 0 3 3 0 0 1 6 0 M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
  copy: "M8 8h13v13H8Z M16 8V3H3v13h5",
  paste: "M9 4H4v17h16V4h-5 M9 2h6v5H9Z",
  rename: "M4 8V4h16v4 M12 4v16 M8 20h8",
  share: "M12 15V2 m-5 5 5-5 5 5 M5 10H3v11h18V10h-2",
  trash: "M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7",
  filter: "M4 5h16l-6 7v7l-4 2v-9Z",
  panel: "M3 4h18v16H3Z M15 4v16",
  list: "M8 6h13 M8 12h13 M8 18h13 M3 6h1 M3 12h1 M3 18h1",
  wifi: "M2 8a16 16 0 0 1 20 0 M5 12a11 11 0 0 1 14 0 M8 16a6 6 0 0 1 8 0 M12 20h.01",
  bluetooth: "M12 2v20l6-6-12-8 M6 16 18 8Z",
  phone: "M7 2h10v20H7Z M11 18h2",
  laptop: "M5 3h14v13H5Z M2 16h20l-2 4H4Z",
  check: "m4 12 5 5L20 6",
  sun: "M12 2v2 M12 20v2 M2 12h2 M20 12h2 M5 5l2 2 M17 17l2 2 M5 19l2-2 M17 7l2-2 M17 12a5 5 0 1 1-10 0 5 5 0 0 1 10 0",
};
paths["folder-plus"] = paths.folder + " M12 10v7 M8.5 13.5h7";
const icon = (n) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${paths[n] || paths.file}"/></svg>`;
const icons = () =>
  document
    .querySelectorAll("[data-icon]")
    .forEach((e) => (e.innerHTML = icon(e.dataset.icon)));
const locations = {
  home: ["Home", "/Users/example"],
  recents: ["Recents", ""],
  projects: ["Projects", "/Users/example/Documents/Projects"],
  documents: ["Documents", "/Users/example/Documents"],
  downloads: ["Downloads", "/Users/example/Downloads"],
  desktop: ["Desktop", "/Users/example/Desktop"],
  disk: ["Macintosh HD", "/"],
  icloud: ["iCloud Drive", "/iCloud Drive"],
  external: ["Field SSD", "/Volumes/Field SSD"],
  trash: ["Trash", ""],
  airdrop: ["AirDrop", ""],
  activity: ["Activity Monitor", ""],
  system: ["System Settings", ""],
  settings: ["Crawler Settings", ""],
};
let seq = 0;
const files = [];
function add(name, parent, type = "Folder", size = 0, date = "2026-09-26") {
  const f = { id: ++seq, name, parent, type, size, date };
  files.push(f);
  return f;
}
add("Documents", "home");
add("Downloads", "home");
add("Desktop", "home");
add("Projects", "documents");
add("Personal", "documents");
add("Applications", "disk");
add("Users", "disk");
add("Library", "disk");
add("Brand assets", "projects");
add("Website", "projects");
add("Research", "projects");
add("Archive", "projects");
const initial = add(
  "Northstar — project brief.pdf",
  "projects",
  "PDF document",
  2.4,
);
add("Northstar — budget.xlsx", "projects", "Spreadsheet", 0.18);
add("Meeting notes.md", "projects", "Markdown", 0.012);
add("Moodboard.fig", "projects", "Design file", 18.6);
add("README.md", "projects", "Markdown", 0.004);
add("Timeline.pdf", "projects", "PDF document", 0.86, "2026-09-21");
add("Invoice — September.pdf", "downloads", "PDF document", 0.24);
add("Coast at dusk.jpg", "downloads", "Image", 8.2);
add("Crawler.dmg", "downloads", "Disk image", 84);
add("Inter.zip", "downloads", "Archive", 5.8);
add("Travel itinerary.pdf", "documents", "PDF document", 0.7);
add("Ideas.md", "desktop", "Markdown", 0.006);
add("Presentation.key", "desktop", "Presentation", 32);
add("Family photos", "icloud");
add("Shared notes.md", "icloud", "Markdown", 0.009);
add("Camera originals", "external");
add("Design archive.zip", "external", "Archive", 740);
for (const [folder, names] of [
  ["Brand assets", ["Wordmark.svg", "Palette.fig", "Brand guidelines.pdf"]],
  ["Website", ["index.html", "app.css", "app.js"]],
  ["Research", ["Interview notes.md", "Competitors.pdf"]],
  ["Archive", ["Northstar v1.pdf"]],
]) {
  const f = files.find((x) => x.name === folder);
  locations["folder" + f.id] = [folder, locations.projects[1] + "/" + folder];
  f.location = "folder" + f.id;
  names.forEach((n) =>
    add(
      n,
      f.location,
      n.endsWith(".pdf")
        ? "PDF document"
        : n.endsWith(".fig")
          ? "Design file"
          : "Text document",
      0.12,
    ),
  );
}
const folderLocation = (f) =>
  f.location ||
  Object.keys(locations).find((k) => locations[k][0] === f.name) ||
  "folder" + f.id;
let tabs = [
    { id: 1, location: "projects", query: "" },
    { id: 2, location: "downloads", query: "" },
  ],
  tabId = 1,
  selected = initial.id,
  view = "columns",
  preview = true,
  sort = "name",
  direction = 1,
  searchActive = false,
  filters = { scope: "all", type: "all", date: "all", size: "all" },
  history = ["projects"],
  historyIndex = 0,
  clipboard = null,
  undo = null,
  metric = "memory",
  processSort = "memory",
  settingsSection = "General",
  saved = [],
  transfers = [],
  received = false;
const prefs = {
  wifi: true,
  bluetooth: true,
  hidden: false,
  vim: true,
  previews: true,
};
const tab = () => tabs.find((t) => t.id === tabId);
const file = () => files.find((f) => f.id === selected);
const isTool = () =>
  ["airdrop", "activity", "system", "settings"].includes(tab().location);
const formatSize = (n) =>
  n === 0
    ? "—"
    : n < 1
      ? Math.max(1, Math.round(n * 1024)) + " KB"
      : n.toFixed(1) + " MB";
const fileIcon = (f) =>
  `<span class="file-icon ${f.type === "Folder" ? "folder" : ""}" style="--kind:${f.type === "PDF document" ? "#d9534f" : f.type === "Spreadsheet" ? "#388e65" : "#5c89c4"}">${icon(f.type === "Folder" ? "folder" : "file")}${f.type === "Folder" ? "" : `<span class="extension">${esc(f.name.split(".").pop().slice(0, 4))}</span>`}</span>`;
function toast(message, fn = null) {
  clearTimeout(toast.timer);
  $("#toast span").textContent = message;
  undo = fn;
  $("#undo").hidden = !fn;
  $("#toast").classList.add("show");
  toast.timer = setTimeout(() => $("#toast").classList.remove("show"), 5000);
}
function theme(name) {
  name = name === "classic" ? "classic" : "cupertino";
  document.documentElement.dataset.theme = name;
  document.title = "Crawler · " + (name === "classic" ? "System 7" : "Cupertino");
  document
    .querySelectorAll(".theme-control button")
    .forEach((b) => b.setAttribute("aria-pressed", b.dataset.theme === name));
  const u = new URL(location.href);
  u.search = "?theme=" + name;
  window.history.replaceState({}, "", u);
}
function nav(location, record = true) {
  if (!moveDraft) markedFiles.clear();
  tab().location = location;
  tab().query = "";
  searchActive = false;
  filters = { scope: "all", type: "all", date: "all", size: "all" };
  $("#search").value = "";
  selected = null;
  if (record) {
    history = history.slice(0, historyIndex + 1);
    history.push(location);
    historyIndex++;
  }
  render();
}
function rows() {
  const t = tab();
  let result = files.filter((f) =>
    t.location === "trash" ? f.deleted : !f.deleted,
  );
  if (t.query || searchActive) {
    if (filters.scope === "folder")
      result = result.filter((f) => f.parent === t.location);
    if (filters.scope === "icloud")
      result = result.filter((f) => f.parent === "icloud");
    const words = t.query.toLowerCase().split(/\s+/).filter(Boolean);
    result = result.filter((f) =>
      words.every((w) => f.name.toLowerCase().includes(w)),
    );
    if (filters.type !== "all")
      result = result.filter((f) => f.type === filters.type);
    if (filters.date === "week")
      result = result.filter((f) => f.date >= "2026-09-20");
    if (filters.size === "large") result = result.filter((f) => f.size >= 10);
  } else if (t.location === "recents")
    result = result.filter((f) => f.type !== "Folder");
  else if (t.location !== "trash")
    result = result.filter((f) => f.parent === t.location);
  return result.sort((a, b) => {
    if (sort === "name" && (a.type === "Folder") !== (b.type === "Folder"))
      return a.type === "Folder" ? -1 : 1;
    return (
      direction *
      (sort === "size"
        ? a.size - b.size
        : String(a[sort]).localeCompare(String(b[sort])))
    );
  });
}
function sidebar() {
  const groups = [
    [
      "Favorites",
      [
        ["home", "home", "Home"],
        ["recents", "clock", "Recents"],
        ["airdrop", "airdrop", "AirDrop"],
        ["desktop", "desktop", "Desktop"],
        ["documents", "folder", "Documents"],
        ["projects", "folder", "Projects"],
        ["downloads", "download", "Downloads"],
      ],
    ],
    [
      "Locations",
      [
        ["disk", "drive", "Macintosh HD"],
        ["icloud", "cloud", "iCloud Drive"],
        ["external", "drive", "Field SSD"],
      ],
    ],
    ["Saved searches", saved.map((s, i) => ["saved:" + i, "search", s.name])],
    [
      "Utilities",
      [
        ["activity", "activity", "Activity Monitor"],
        ["system", "settings", "System Settings"],
        ["settings", "sliders", "Crawler Settings"],
        ["trash", "trash", "Trash"],
      ],
    ],
  ];
  $("#sidebar").innerHTML = groups
    .filter((g) => g[1].length)
    .map(
      ([title, items]) =>
        `<div class="nav-section"><h2>${title}</h2>${items.map(([id, i, label]) => `<button class="nav-item ${tab().location === id ? "active" : ""}" data-go="${id}">${icon(i)}<span>${esc(label)}</span>${id === "downloads" ? '<span class="badge">4</span>' : ""}</button>`).join("")}</div>`,
    )
    .join("");
  updateSidebar();
}
function previewArt(f) {
  if (!f) return "";
  if (f.type === "Folder")
    return `<div class="folder-art">${icon("folder")}<p>${esc(f.name)}</p></div>`;
  if (f.type === "PDF document")
    return `<div class="paper-preview"><div class="paper-brand">NORTHSTAR <span>↗</span></div><h2>A clearer<br>way forward.</h2><p>PROJECT BRIEF · SEPTEMBER 2026</p><div class="paper-rule"></div><h3>Good work starts<br>with a little space.</h3><p>A shared direction for the next chapter.<br>Prepared by the Northstar studio.</p><div class="paper-foot">01 <span>northstar.studio</span></div></div>`;
  if (f.type === "Spreadsheet")
    return `<div class="sheet-art"><table><tr><th>Budget</th><th>EUR</th></tr><tr><td>Research</td><td>2,400</td></tr><tr><td>Design</td><td>6,800</td></tr><tr><td>Development</td><td>9,200</td></tr><tr><th>Total</th><th>18,400</th></tr></table></div>`;
  return `<div class="text-art">${esc(f.type === "Markdown" ? "# " + f.name.replace(".md", "") + "\n\nA place for ideas in progress.\n\n- Keep navigation simple\n- Make files easy to find\n- Leave room to think" : f.name + "\n\nSample file preview\n\n" + f.type + " · " + formatSize(f.size))}</div>`;
}
function inspector(f) {
  return `<aside class="inspector"><div class="inspector-title">Details<button class="icon-button" data-action="toggle-preview" aria-label="Hide details">${icon("close")}</button></div>${f ? `<div class="preview-art">${previewArt(f)}${f.type === "PDF document" ? '<span class="preview-page-count">1 of 4 · sample preview</span>' : ""}</div><div class="inspector-info"><h2>${esc(f.name)}</h2><p>${esc(f.type)} · ${formatSize(f.size)}</p><dl class="detail-grid"><dt>Location</dt><dd>${esc(locations[f.parent]?.[0] || f.parent)}</dd><dt>Modified</dt><dd>26 September 2026</dd><dt>Created</dt><dd>21 September 2026</dd><dt>Size</dt><dd>${formatSize(f.size)}</dd></dl><div class="inspector-actions"><button class="secondary" data-action="share">${icon("share")} Share</button><button class="primary" data-action="open">${f.deleted ? "Restore" : "Open"}</button></div></div>` : '<div class="empty-state"><p>Select a file to see its details.</p></div>'}</aside>`;
}
function filterSelect(label, id, options) {
  return `<label>${label}<select data-filter="${id}">${options.map(([v, n]) => `<option value="${v}" ${filters[id] === v ? "selected" : ""}>${n}</option>`).join("")}</select></label>`;
}
function browser() {
  $("#search").disabled = !!moveDraft;
  const list = rows();
  if (!list.some((f) => f.id === selected)) {
    selected =
      view === "columns"
        ? list.find((f) => f.type === "Folder")?.id || list[0]?.id || null
        : list[0]?.id || null;
  }
  const searching = tab().query || searchActive;
  if (view === "columns" && !searching) return renderColumns(list);
  const title = searching
    ? tab().query
      ? `Results for “${tab().query}”`
      : "Find your files"
    : locations[tab().location]?.[0] || tab().location;
  const subtitle = searching
    ? `${list.length} results · ${filters.scope === "folder" ? "Current folder" : filters.scope === "icloud" ? "iCloud Drive" : "This Mac"} · sample files`
    : `${list.length} items${tab().location === "projects" ? " · Everything for your next idea." : ""}`;
  $("#content").innerHTML =
    `<div class="file-browser ${preview ? "" : "preview-hidden"}"><section class="file-area"><header class="page-heading"><div><h1>${esc(title)}</h1><p>${esc(subtitle)}</p></div>${searching ? '<button class="heading-action" data-action="save-search">' + icon("plus") + " Save search</button>" : viewButtons()}</header>${
      searching
        ? `<div class="search-filters">${filterSelect("Search in", "scope", [
            ["all", "This Mac"],
            ["folder", "This folder"],
            ["icloud", "iCloud Drive"],
          ])}${filterSelect("Kind", "type", [
            ["all", "Any kind"],
            ["Folder", "Folders"],
            ["PDF document", "PDF documents"],
            ["Image", "Images"],
            ["Markdown", "Markdown"],
            ["Design file", "Design files"],
          ])}${filterSelect("Modified", "date", [
            ["all", "Any time"],
            ["week", "This week"],
          ])}${filterSelect("Size", "size", [
            ["all", "Any size"],
            ["large", "Over 10 MB"],
          ])}<button class="filter-reset" data-action="reset-filters">Reset</button></div>`
        : ""
    }<div class="table-wrap">${
      list.length
        ? `<table class="file-table ${searching ? "search-table" : ""}"><colgroup><col class="name-col"><col class="date-col"><col class="kind-col"><col class="size-col"></colgroup><thead><tr>${[
            ["name", "Name"],
            [
              searching ? "parent" : "date",
              searching ? "Location" : "Date modified",
            ],
            ["type", "Kind"],
            ["size", "Size"],
          ]
            .map(
              ([s, n]) =>
                `<th><button data-sort="${s}">${n}${sort === s ? (direction === 1 ? " ↑" : " ↓") : ""}</button></th>`,
            )
            .join(
              "",
            )}</tr></thead><tbody>${list.map((f) => `<tr tabindex="0" data-file="${f.id}" class="${selected === f.id ? "active" : ""}" aria-selected="${selected === f.id}"><td class="file-name"><span>${fileIcon(f)}<span class="name-text">${esc(f.name)}</span></span></td><td class="date-cell">${searching ? esc(locations[f.parent]?.[0] || f.parent) : f.date === "2026-09-26" ? "Today, 10:42" : "21 Sep 2026"}</td><td class="kind-cell">${f.type}</td><td class="size">${formatSize(f.size)}</td></tr>`).join("")}</tbody></table>`
        : `<div class="empty-state">${icon(searching ? "search" : "folder")}<h2>${searching ? "No matching files" : "Nothing here yet"}</h2><p>${searching ? "Try another name or broaden your filters." : "New files will appear here."}</p>${searching ? '<button class="secondary" data-action="reset-filters">Clear filters</button>' : ""}</div>`
    }</div></section>${inspector(file())}</div>`;
  decorateMoveState();
  $("#status").textContent =
    `${list.length} items${file() ? " · 1 selected · " + formatSize(file().size) : ""}`;
}
const devices = [
  ["Sample laptop", "Laptop", "laptop"],
  ["Sample phone", "Phone", "phone"],
  ["Sample desktop", "Desktop", "desktop"],
];
function airDrop() {
  return `<div class="tool-page"><header class="tool-header"><div><h1>AirDrop</h1><p>Send a file without leaving your workspace.</p></div><span class="sample-badge">Sample devices</span></header><div class="airdrop-intro"><div class="airdrop-symbol">${icon("airdrop")}</div><div><h2>Ready to share</h2><p>Choose a nearby device, then choose a file.</p></div><label class="receive-control">Discoverable by<select id="discoverability"><option>Contacts only</option><option>Everyone for 10 minutes</option><option>Receiving off</option></select></label></div><h2 class="section-heading">Nearby <small>3 devices</small></h2><div class="devices">${devices.map(([n, t, i], j) => `<button class="device" data-device="${j}"><span class="device-icon">${icon(i)}</span><strong>${n}</strong><small>${t}</small></button>`).join("")}</div><h2 class="section-heading">Transfers <small>This session</small></h2>${!received ? `<div class="transfer-row">${icon("download")}<div><strong>Sample sender wants to share Reference.pdf</strong><small>PDF document · 1.8 MB · simulated request</small></div><button class="secondary" data-action="decline">Decline</button><button class="primary" data-action="receive">Accept</button></div>` : ""}${transfers.map((t) => `<div class="transfer-row">${icon("check")}<div><strong>${esc(t.name)}</strong><small>${esc(t.device)}</small></div><span class="transfer-state">${t.received ? "Saved to Downloads" : "Simulated send complete"}</span></div>`).join("")}<p class="tool-notice">${icon("help")} This mockup uses sample devices. No files are sent or received over AirDrop.</p></div>`;
}
const processes = [
  ["WindowServer", 1.42, 7.4, 18],
  ["Safari", 3.6, 4.2, 42],
  ["Xcode", 4.8, 1.3, 76],
  ["Crawler", 0.084, 0.2, 8],
  ["Music", 0.32, 0.4, 14],
  ["kernel_task", 1.1, 2.1, 190],
];
function activity() {
  return `<div class="tool-page"><header class="tool-header"><div><h1>Activity Monitor</h1><p>A quick look at what your Mac is doing.</p></div><span class="sample-badge">Sample data · not live</span></header><div class="activity-tabs"><button data-metric="memory" class="${metric === "memory" ? "active" : ""}">Memory</button><button data-metric="cpu" class="${metric === "cpu" ? "active" : ""}">CPU</button></div><div class="memory-overview"><div class="memory-bar-block"><div class="memory-title"><strong>${metric === "memory" ? "18.2" : "15.6"}</strong><small>${metric === "memory" ? "GB used" : "% CPU used"}</small><span>${metric === "memory" ? "32 GB physical memory" : "10 cores"}</span></div><div class="memory-bar"><i style="width:35%"></i><i style="width:14%"></i><i style="width:8%"></i></div><div class="memory-legend"><span><i></i>${metric === "memory" ? "App memory 11.2 GB" : "User 11.1%"}</span><span><i></i>${metric === "memory" ? "Wired 4.5 GB" : "System 4.5%"}</span><span><i></i>${metric === "memory" ? "Compressed 2.5 GB" : "Idle 84.4%"}</span></div></div><div class="memory-pressure"><small>${metric === "memory" ? "Memory pressure" : "System load"}</small><strong>Normal</strong><small>${metric === "memory" ? "Swap used · 0 bytes" : "Processes · 326"}</small></div></div><h2 class="section-heading">Processes <small>Click a process to inspect</small></h2><table class="process-table"><thead><tr><th>Process</th><th><button data-process-sort="memory">Memory ↓</button></th><th><button data-process-sort="cpu">CPU % ↓</button></th><th>Threads</th></tr></thead><tbody>${[
    ...processes,
  ]
    .sort(
      (a, b) =>
        b[processSort === "memory" ? 1 : 2] -
        a[processSort === "memory" ? 1 : 2],
    )
    .map(
      (p) =>
        `<tr tabindex="0" data-process="${p[0]}"><td>${icon(p[0] === "Crawler" ? "folder" : "activity")} ${p[0]}</td><td>${p[1] >= 1 ? p[1].toFixed(2) + " GB" : Math.round(p[1] * 1024) + " MB"}</td><td>${p[2].toFixed(1)}</td><td>${p[3]}</td></tr>`,
    )
    .join(
      "",
    )}</tbody></table><p class="tool-notice">${icon("help")} These numbers illustrate the interface. They are not measurements of this Mac or Crawler.</p></div>`;
}
const toggle = (key, label, description) =>
  `<div class="setting-row"><div><strong>${label}</strong><p>${description}</p></div><button class="toggle" role="switch" aria-label="${label}" aria-checked="${!!prefs[key]}" data-toggle="${key}"></button></div>`;
const systemItems = [
  ["wifi", "Wi-Fi", "Connected to Studio"],
  ["bluetooth", "Bluetooth", "On · 3 devices"],
  ["sun", "Displays", "Brightness and appearance"],
  ["drive", "Storage", "312 GB available"],
  ["activity", "Battery", "89% · charging"],
  ["settings", "Privacy & Security", "App permissions"],
];
function settings(system = false) {
  return `<div class="tool-page"><header class="tool-header"><div><h1>${system ? "System Settings" : "Crawler Settings"}</h1><p>${system ? "Common controls, one click away." : "Make this workspace yours."}</p></div><span class="sample-badge">${system ? "Simulated controls" : "Session preferences"}</span></header>${system ? `<div class="system-grid">${systemItems.map(([i, n, d]) => `<button class="system-setting" data-system="${n}">${icon(i)}<div><strong>${n}</strong><small>${d}</small></div><span class="chevron">${icon("right")}</span></button>`).join("")}</div><p class="tool-notice">${icon("help")} Changes here only affect this mockup. macOS settings stay unchanged.</p>` : `<div class="settings-layout"><nav class="settings-nav">${["General", "Search", "Appearance"].map((s) => `<button data-settings-section="${s}" class="${s === settingsSection ? "active" : ""}">${s}</button>`).join("")}</nav><section class="settings-content"><h2>${settingsSection}</h2>${settingsSection === "General" ? toggle("vim", "Vim navigation", "Use h, j, k and l alongside arrow keys.") + toggle("hidden", "Show hidden files", "Sample library contains no hidden files.") + toggle("previews", "File previews", "Show a preview when opening a document.") : settingsSection === "Search" ? `<div class="setting-row"><div><strong>Default search scope</strong><p>Choose where a new search begins.</p></div><select id="default-scope"><option value="all">This Mac</option><option value="folder">This folder</option></select></div><p class="tool-notice">Search matches file names in this sample library.</p>` : `<div class="setting-row"><div><strong>Theme</strong><p>Two ways to feel at home.</p></div><select data-theme-select><option value="cupertino" ${document.documentElement.dataset.theme === "cupertino" ? "selected" : ""}>Cupertino</option><option value="classic" ${document.documentElement.dataset.theme === "classic" ? "selected" : ""}>System 7</option></select></div>`}</section></div>`}</div>`;
}
function render() {
  sidebar();
  $("#tabs").innerHTML = tabs
    .map(
      (t) =>
        `<button class="tab ${t.id === tabId ? "active" : ""}" data-tab="${t.id}">${icon("folder")}<span class="tab-name">${esc(locations[t.location]?.[0] || t.location)}</span>${tabs.length > 1 ? `<span class="close-tab" data-close-tab="${t.id}" aria-label="Close tab">×</span>` : ""}</button>`,
    )
    .join("");
  const loc = locations[tab().location];
  const crumbs = loc?.[1]
    ? loc[1].split("/").filter(Boolean)
    : [loc?.[0] || tab().location];
  $("#address").innerHTML =
    icon(isTool() ? "settings" : "folder") +
    crumbs
      .map(
        (c, i) =>
          `<button data-crumb="${esc(c)}">${esc(c)}</button>${i < crumbs.length - 1 ? icon("right") : ""}`,
      )
      .join("");
  $("#back").disabled = historyIndex === 0;
  $("#forward").disabled = historyIndex === history.length - 1;
  $("#up").disabled =
    isTool() || tab().location === "home" || tab().location === "disk";
  $("#search").value = tab().query;
  $("#file-commandbar").hidden = isTool();
  if (isTool()) {
    $("#content").innerHTML =
      tab().location === "airdrop"
        ? airDrop()
        : tab().location === "activity"
          ? activity()
          : settings(tab().location === "system");
    $("#status").textContent = "Interactive mockup · sample data";
  } else browser();
  if (isTool() || searchActive)
    $("#keyboard-status").textContent = isTool()
      ? "⌘ F to search files"
      : "↑ ↓ to navigate · Space to preview · ⌘ F to find";
  $(".inspector-toggle").setAttribute("aria-pressed", preview);
  $(".inspector-toggle").disabled = view === "columns" && !searchActive;
  icons();
}
function modal(title, body, wide = false) {
  $("#modal-title").textContent = title;
  $("#modal-body").innerHTML = body;
  $("#modal").classList.toggle("preview-modal", wide);
  if (!$("#modal").open) $("#modal").showModal();
  setTimeout(() => $("#modal input")?.focus(), 0);
}
function closeModal() {
  $("#modal").close();
}
function nameDialog(rename = false) {
  const f = file();
  if (rename && !f) return toast("Select a file first.");
  modal(
    rename ? "Rename file" : "New folder",
    `<form id="name-form"><label class="modal-label" for="new-name">${rename ? "File name" : "Folder name"}</label><input class="modal-input" id="new-name" value="${rename ? esc(f.name) : "Untitled folder"}" required><p class="form-error" id="name-error"></p><div class="modal-actions"><button type="button" class="secondary" data-action="close-modal">Cancel</button><button class="primary">${rename ? "Rename" : "Create folder"}</button></div></form>`,
  );
  $("#name-form").onsubmit = (e) => {
    e.preventDefault();
    const name = $("#new-name").value.trim();
    const parent = rename ? f.parent : tab().location;
    if (
      !name ||
      name.includes("/") ||
      files.some(
        (x) =>
          !x.deleted &&
          x.parent === parent &&
          x.name === name &&
          x.id !== (rename ? f.id : 0),
      )
    ) {
      $("#name-error").textContent = "Choose a unique name without a slash.";
      return;
    }
    if (rename) f.name = name;
    else {
      const n = add(name, parent);
      n.location = "folder" + n.id;
      locations[n.location] = [
        name,
        (locations[parent]?.[1] || "/") + "/" + name,
      ];
      selected = n.id;
    }
    closeModal();
    render();
    toast(rename ? "File renamed in mockup." : "Folder created in mockup.");
  };
}
function openFile() {
  const f = file();
  if (!f) return;
  if (f.deleted) {
    f.deleted = false;
    render();
    return toast("Restored " + f.name);
  }
  if (f.type === "Folder") {
    const location = folderLocation(f);
    if (!locations[location])
      locations[location] = [
        f.name,
        (locations[f.parent]?.[1] || "") + "/" + f.name,
      ];
    nav(location);
  } else if (prefs.previews)
    modal(
      f.name,
      `<div class="preview-modal-content">${previewArt(f)}</div><p class="tool-notice">Sample preview · ${esc(f.type)} · ${formatSize(f.size)}</p>`,
      true,
    );
  else toast("Preview is disabled. Enable it in Crawler Settings.");
}
function share(device = null) {
  const f =
    file() && file().type !== "Folder"
      ? file()
      : files.find((f) => f.id === initial.id);
  modal(
    device === null ? "Share a file" : "Send to " + devices[device][0],
    `<label class="modal-label" for="share-file">File</label><select class="modal-input" id="share-file">${files
      .filter((f) => !f.deleted && f.type !== "Folder")
      .map(
        (x) =>
          `<option value="${x.id}" ${x.id === f?.id ? "selected" : ""}>${esc(x.name)}</option>`,
      )
      .join(
        "",
      )}</select><p class="modal-label">${device === null ? "Choose an AirDrop device" : "Sample AirDrop transfer"}</p>${devices.map(([n, t, i], j) => (device === null || device === j ? `<button class="share-device" data-send="${j}">${icon(i)}<div><strong>${n}</strong><small>${t}</small></div><span class="primary-label">Send →</span></button>` : "")).join("")}<p class="tool-notice">No real files will be sent.</p>`,
  );
}
function quickSettings() {
  const p = $("#quick-settings");
  p.hidden = !p.hidden;
  if (!p.hidden) {
    p.innerHTML = `<header><h2>Quick settings</h2><button class="icon-button" data-action="quick-settings" aria-label="Close quick settings">${icon("close")}</button></header><span class="sample-badge">Simulated controls</span>${toggle("wifi", "Wi-Fi", "Studio network")}${toggle("bluetooth", "Bluetooth", "3 sample devices")}<label class="brightness-label">Brightness <input type="range" min="20" max="100" value="85" aria-label="Mockup brightness" id="brightness"></label><button class="secondary" data-go="system">Open System Settings ${icon("right")}</button>`;
  }
}
function action(a) {
  switch (a) {
    case "back":
      if (historyIndex > 0) nav(history[--historyIndex], false);
      break;
    case "forward":
      if (historyIndex < history.length - 1)
        nav(history[++historyIndex], false);
      break;
    case "up": {
      const parent = files.find(
        (f) => folderLocation(f) === tab().location,
      )?.parent;
      nav(parent || "home");
      break;
    }
    case "new-tab": {
      const id = Date.now();
      tabs.push({ id, location: "home", query: "" });
      tabId = id;
      nav("home");
      break;
    }
    case "edit-path":
      $("#address").hidden = true;
      $("#address-form").hidden = false;
      $("#address-input").value =
        locations[tab().location]?.[1] || "/Users/example";
      $("#address-input").focus();
      $("#address-input").select();
      break;
    case "start-search":
      if (!searchActive) filters.scope = prefs.scope || "all";
      searchActive = true;
      render();
      $("#search").focus();
      break;
    case "toggle-preview":
      preview = !preview;
      render();
      break;
    case "new-folder":
      nameDialog();
      break;
    case "rename":
      nameDialog(true);
      break;
    case "open":
      openFile();
      break;
    case "copy":
    case "cut":
      if (file()) {
        clipboard = { id: selected, cut: a === "cut" };
        toast(file().name + " " + (a === "cut" ? "ready to move." : "copied."));
      } else toast("Select a file first.");
      break;
    case "paste": {
      if (!clipboard) return toast("Copy or cut a file first.");
      if (isTool() || ["trash", "recents"].includes(tab().location))
        return toast("Open a destination folder first.");
      const f = files.find((x) => x.id === clipboard.id);
      if (clipboard.cut) {
        f.parent = tab().location;
        clipboard = null;
      } else {
        const n = add(
          f.name.replace(/(\.[^.]+)?$/, " copy$1"),
          tab().location,
          f.type,
          f.size,
        );
        selected = n.id;
      }
      render();
      toast("Pasted in " + locations[tab().location]?.[0] + ".");
      break;
    }
    case "trash": {
      const f = file();
      if (!f) return toast("Select a file first.");
      f.deleted = true;
      render();
      toast("Moved " + f.name + " to Trash.", () => {
        f.deleted = false;
        render();
      });
      break;
    }
    case "share":
      share();
      break;
    case "quick-settings":
      quickSettings();
      break;
    case "close-modal":
      closeModal();
      break;
    case "reset-filters":
      filters = { scope: "all", type: "all", date: "all", size: "all" };
      render();
      break;
    case "save-search":
      saved.push({
        name: tab().query || "Filtered files",
        query: tab().query,
        filters: { ...filters },
        location: tab().location,
      });
      render();
      toast("Search added to the sidebar.");
      break;
    case "receive":
      received = true;
      add("Reference.pdf", "downloads", "PDF document", 1.8);
      transfers.unshift({
        name: "Reference.pdf",
        device: "From Sample laptop",
        received: true,
      });
      render();
      toast("Sample file added to Downloads.");
      break;
    case "decline":
      received = true;
      render();
      toast("Sample request declined.");
      break;
    case "help":
      modal(
        "Keyboard shortcuts",
        `<div class="shortcut-grid">${[
          ["Find files", "⌘ F"],
          ["Edit folder path", "⌘ L"],
          ["Move selection", "↑ ↓ or j k"],
          ["Open folder or file", "Enter or l"],
          ["Parent folder", "h"],
          ["Preview file", "Space"],
          ["Close preview / clear search", "Esc"],
          ["Copy / cut / paste", "⌘ C / X / V"],
          ["New folder", "⇧ ⌘ N"],
          ["Columns: pick up / confirm move", "m / Enter"],
          ["Columns: mark multiple files", "x"],
          ["Columns: cancel move", "Esc"],
        ]
          .map(([n, k]) => `<div><span>${n}</span><kbd>${k}</kbd></div>`)
          .join("")}</div>`,
      );
      break;
  }
}
document.addEventListener("click", (e) => {
  const b = e.target.closest(
    "button,[data-file],[data-process],[data-close-tab]",
  );
  if (!b) return;
  const d = b.dataset;
  if (d.theme) return theme(d.theme);
  if (d.action) return action(d.action);
  if (d.closeTab) {
    tabs = tabs.filter((t) => t.id !== +d.closeTab);
    if (tabId === +d.closeTab) tabId = tabs[0].id;
    return render();
  }
  if (d.tab) {
    tabId = +d.tab;
    searchActive = !!tab().query;
    return render();
  }
  if (d.go) {
    $("#quick-settings").hidden = true;
    if (d.go.startsWith("saved:")) {
      const s = saved[+d.go.split(":")[1]];
      tab().location = s.location;
      tab().query = s.query;
      filters = { ...s.filters };
      searchActive = true;
      render();
    } else nav(d.go);
    return;
  }
  if (d.crumb) {
    const k = Object.keys(locations).find((k) => locations[k][0] === d.crumb);
    if (k) nav(k);
    else if (d.crumb === "example" || d.crumb === "Users") nav("home");
    return;
  }
  if (d.file) {
    selected = +d.file;
    document.querySelectorAll("[data-file]").forEach((r) => {
      r.classList.toggle("active", +r.dataset.file === selected);
      r.setAttribute("aria-selected", +r.dataset.file === selected);
    });
    $(".inspector").outerHTML = inspector(file());
    $("#status").textContent =
      rows().length + " items · 1 selected · " + formatSize(file().size);
    return;
  }
  if (d.view) {
    view = d.view;
    return render();
  }
  if (d.sort) {
    direction = sort === d.sort ? -direction : 1;
    sort = d.sort;
    return browser();
  }
  if (d.device) return share(+d.device);
  if (d.send) {
    const f = files.find((x) => x.id === +$("#share-file").value);
    transfers.unshift({ name: f.name, device: "To " + devices[+d.send][0] });
    closeModal();
    render();
    return toast("Simulated send complete. No real file was transferred.");
  }
  if (d.metric) {
    metric = d.metric;
    processSort = metric;
    return render();
  }
  if (d.processSort) {
    processSort = d.processSort;
    return render();
  }
  if (d.process) {
    const p = processes.find((p) => p[0] === d.process);
    return modal(
      p[0] + " · process details",
      `<p class="sample-badge">Sample memory allocation</p><dl class="detail-grid"><dt>Memory</dt><dd>${p[1]} GB</dd><dt>Private memory</dt><dd>${(p[1] * 0.72).toFixed(2)} GB</dd><dt>Shared memory</dt><dd>${(p[1] * 0.28).toFixed(2)} GB</dd><dt>Threads</dt><dd>${p[3]}</dd><dt>CPU</dt><dd>${p[2]}%</dd></dl>`,
    );
  }
  if (d.settingsSection) {
    settingsSection = d.settingsSection;
    return render();
  }
  if (d.toggle) {
    prefs[d.toggle] = !prefs[d.toggle];
    b.setAttribute("aria-checked", prefs[d.toggle]);
    return;
  }
  if (d.system) {
    modal(
      d.system,
      `<p class="sample-badge">Simulated system settings</p>${d.system === "Wi-Fi" ? toggle("wifi", "Wi-Fi", "Studio network") : d.system === "Bluetooth" ? toggle("bluetooth", "Bluetooth", "Keyboard, trackpad and headphones") : d.system === "Displays" ? '<label class="modal-label">Brightness<input type="range" id="brightness" min="20" max="100" value="85" aria-label="Mockup brightness"></label>' : d.system === "Storage" ? '<p>Macintosh HD · 512 GB</p><div class="memory-bar"><i style="width:39%"></i></div><p>200 GB used · 312 GB available</p>' : d.system === "Battery" ? "<p>89% · Power adapter connected</p>" : "<p>Crawler would request access to folders you choose. This mockup has no filesystem access.</p>"}`,
    );
  }
});
document.addEventListener("dblclick", (e) => {
  const row = e.target.closest("[data-file]");
  if (row) {
    selected = +row.dataset.file;
    openFile();
  }
});
document.addEventListener("change", (e) => {
  if (e.target.dataset.filter) {
    filters[e.target.dataset.filter] = e.target.value;
    browser();
  }
  if (e.target.hasAttribute("data-theme-select")) theme(e.target.value);
  if (e.target.id === "discoverability")
    toast("Mock discoverability: " + e.target.value);
  if (e.target.id === "default-scope") {
    prefs.scope = e.target.value;
    toast("Default search scope updated for this session.");
  }
});
document.addEventListener("input", (e) => {
  if (e.target.id === "brightness")
    $(".window").style.filter =
      `brightness(${0.6 + Number(e.target.value) / 250})`;
});
$("#search").addEventListener("input", (e) => {
  const query = e.target.value;
  if (isTool()) nav("home");
  tab().query = query;
  if (!searchActive) filters.scope = prefs.scope || "all";
  searchActive = !!query;
  if (!searchActive)
    filters = {
      scope: prefs.scope || "all",
      type: "all",
      date: "all",
      size: "all",
    };
  render();
  $("#search").focus();
});
$("#address-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const path = $("#address-input").value.replace(/\/$/, "") || "/";
  const k = Object.keys(locations).find((k) => locations[k][1] === path);
  if (k) nav(k);
  else toast("That path is not in the sample library.");
  $("#address-form").hidden = true;
  $("#address").hidden = false;
});
$("#undo").onclick = () => {
  undo?.();
  $("#toast").classList.remove("show");
};
document.addEventListener("keydown", (e) => {
  const typing = /INPUT|SELECT|TEXTAREA/.test(e.target.tagName);
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
    e.preventDefault();
    if (isTool()) nav("home");
    action("start-search");
    return;
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
    e.preventDefault();
    action("edit-path");
    return;
  }
  if (e.key === "Escape") {
    closeModal();
    $("#quick-settings").hidden = true;
    $("#address-form").hidden = true;
    $("#address").hidden = false;
    if (tab().query || searchActive) {
      tab().query = "";
      searchActive = false;
      render();
    }
    $("#search").blur();
    return;
  }
  if (typing || $("#modal").open) return;
  if (e.metaKey || e.ctrlKey) {
    const a = {
      c: "copy",
      x: "cut",
      v: "paste",
      n: e.shiftKey ? "new-folder" : "new-tab",
    }[e.key.toLowerCase()];
    if (a) {
      e.preventDefault();
      action(a);
    }
    return;
  }
  if (isTool()) return;
  const list = rows(),
    i = list.findIndex((f) => f.id === selected);
  if (
    ["ArrowDown", "ArrowUp"].includes(e.key) ||
    (prefs.vim && ["j", "k"].includes(e.key))
  ) {
    e.preventDefault();
    selected =
      list[
        Math.max(
          0,
          Math.min(
            list.length - 1,
            i + (["ArrowDown", "j"].includes(e.key) ? 1 : -1),
          ),
        )
      ]?.id;
    browser();
    document
      .querySelector(`tr[data-file="${selected}"]`)
      ?.scrollIntoView({ block: "nearest" });
  } else if (
    e.key === "Enter" ||
    e.key === " " ||
    (prefs.vim && e.key === "l")
  ) {
    e.preventDefault();
    openFile();
  } else if (e.key === "ArrowLeft" || (prefs.vim && e.key === "h"))
    action("up");
});
theme(new URL(location.href).searchParams.get("theme"));
