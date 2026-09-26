// Columns is an optional navigation experiment over the same sample library.
let moveDraft = null;
let animateMoves = !matchMedia("(prefers-reduced-motion: reduce)").matches;
const markedFiles = new Set();
const folderOwner = (location) =>
  files.find((f) => f.type === "Folder" && folderLocation(f) === location);
const parentOf = (location) =>
  folderOwner(location)?.parent || (location === "home" ? "disk" : null);
const childrenOf = (location) =>
  files
    .filter((f) => !f.deleted && f.parent === location)
    .sort(
      (a, b) =>
        (b.type === "Folder") - (a.type === "Folder") ||
        a.name.localeCompare(b.name),
    );
function ensureFolder(f) {
  const location = folderLocation(f);
  f.location = location;
  if (!locations[location])
    locations[location] = [
      f.name,
      (locations[f.parent]?.[1] || "") + "/" + f.name,
    ];
  return location;
}
function updateFolderPaths() {
  const visit = (location, seen = new Set()) => {
    if (seen.has(location)) return;
    seen.add(location);
    const owner = folderOwner(location);
    if (!owner) return;
    visit(owner.parent, seen);
    locations[location] = [
      owner.name,
      (locations[owner.parent]?.[1] || "").replace(/\/$/, "") +
        "/" +
        owner.name,
    ];
  };
  Object.keys(locations).forEach((k) => visit(k));
}
function viewButtons() {
  return `<div class="view-options">${[
    ["list", "List", "list"],
    ["columns", "Columns", "panel"],
  ]
    .map(
      ([v, label, i]) =>
        `<button data-view="${v}" class="${view === v ? "active" : ""}" aria-label="${label} view" aria-pressed="${view === v}" title="${label} view">${icon(i)}${v === "columns" ? "<span>Columns</span>" : ""}</button>`,
    )
    .join("")}</div>`;
}
let visibleFolders = [];
function columnRow(f, location) {
  const current = location === tab().location;
  const next = visibleFolders[visibleFolders.indexOf(location) + 1];
  const active = current
    ? f.id === selected
    : f.type === "Folder" && folderLocation(f) === next;
  return `<button class="column-row ${active ? "active" : ""} ${moveDraft?.ids.includes(f.id) ? "in-transit" : ""} ${markedFiles.has(f.id) ? "marked" : ""}" data-column-file="${f.id}" data-location="${location}" data-zone="${current ? "current" : "context"}" aria-pressed="${active || markedFiles.has(f.id)}">${fileIcon(f)}<span>${esc(f.name)}</span>${markedFiles.has(f.id) ? icon("check") : f.type === "Folder" ? icon("right") : `<small>${formatSize(f.size)}</small>`}</button>`;
}
function moveProblem(destination) {
  if (!moveDraft) return "";
  if (!locations[destination]?.[1] || destination === "trash")
    return "Choose a folder as the destination.";
  for (const id of moveDraft.ids) {
    const f = files.find((x) => x.id === id);
    if (!f || f.deleted)
      return "A selected item is no longer available. Cancel and select it again.";
    if (f.parent === destination) return "Choose a different folder.";
    let cursor = destination;
    const visited = new Set();
    while (cursor && !visited.has(cursor)) {
      visited.add(cursor);
      if (f.type === "Folder" && folderLocation(f) === cursor)
        return "A folder cannot move into itself or its contents.";
      cursor = parentOf(cursor);
    }
    if (
      files.some(
        (x) =>
          !x.deleted &&
          x.parent === destination &&
          x.name === f.name &&
          !moveDraft.ids.includes(x.id),
      )
    )
      return `A file named "${f.name}" already exists here.`;
  }
  return "";
}
function renderColumns(list) {
  $("#search").disabled = !!moveDraft;
  const location = tab().location,
    f = file();
  const chain = [];
  let cursor = location;
  while (cursor && !chain.includes(cursor)) {
    chain.unshift(cursor);
    cursor = parentOf(cursor);
  }
  if (!visibleFolders.includes(location)) visibleFolders = chain.slice(-3);
  const index = visibleFolders.indexOf(location);
  const previewFile = f && f.type !== "Folder" ? f : null;
  const leaf = !list.some((item) => item.type === "Folder");
  if (f?.type === "Folder") {
    visibleFolders = visibleFolders.slice(0, index + 1);
    visibleFolders.push(ensureFolder(f));
    visibleFolders = visibleFolders.slice(-3);
  } else if (previewFile || leaf)
    visibleFolders = visibleFolders.slice(0, index + 1).slice(-2);
  else if (index < visibleFolders.length - 1) {
    // Keep the deeper folder visible when stepping left without entering it.
    visibleFolders = visibleFolders.slice(0, 3);
  }
  if (!previewFile && !leaf) {
    while (visibleFolders.length < 3) {
      const ancestor = parentOf(visibleFolders[0]);
      if (!ancestor || visibleFolders.includes(ancestor)) break;
      visibleFolders.unshift(ancestor);
    }
  }
  const pane = (id) => {
    const active = id === location,
      items = active ? list : childrenOf(id);
    return `<section class="file-column ${active ? "current-column active-folder" : "context-column"}" data-pane-location="${id}" aria-label="${esc(locations[id]?.[0] || id)}${active ? ", current folder" : ""}"><header><button data-column-nav="${id}" ${active ? 'aria-current="location"' : ""}>${icon("folder")}${esc(locations[id]?.[0] || id)}</button></header><div class="column-scroll">${items.map((item) => columnRow(item, id)).join("") || '<div class="column-empty">Empty folder</div>'}</div></section>`;
  };
  const panes = visibleFolders.map(pane);
  if (leaf && visibleFolders.length === 1)
    panes.unshift('<section class="file-column unused-column"></section>');
  if (previewFile)
    panes.push(
      `<section class="file-column preview-column"><header><span>Preview</span><strong>${esc(previewFile.name)}</strong></header><div class="column-scroll"><div class="column-preview">${previewArt(previewFile)}<p>${esc(previewFile.type)} · ${formatSize(previewFile.size)}</p></div></div></section>`,
    );
  if (leaf && !previewFile)
    panes.push(
      '<section class="file-column preview-column"><header><span>Preview</span></header><div class="column-empty">No file selected</div></section>',
    );
  while (panes.length < 3)
    panes.push(
      '<section class="file-column unused-column"><div class="column-empty">Select a folder to see its contents,<br>or a file to preview it.</div></section>',
    );
  $("#content").innerHTML =
    `<div class="columns-workspace"><header class="page-heading"><div><h1>${esc(locations[location]?.[0] || location)}</h1><p>${list.length} items</p></div>${viewButtons()}</header><div class="columns-guidance"><span>← parent · → open folder · click a pane to focus it</span><span><kbd>j</kbd><kbd>k</kbd> select <kbd>x</kbd> mark <button class="motion-toggle" data-column-action="motion" aria-pressed="${animateMoves}">Motion ${animateMoves ? "on" : "off"}</button></span></div><div class="columns-layout">${panes.join("")}</div></div>`;
  decorateMoveState();
  $("#status").textContent = `${list.length} items`;
}
// Preview pending items inside the destination without adding them to its data.
function inlineMovePreview() {
  document.querySelectorAll("[data-move-preview]").forEach((el) => el.remove());
  let controls = $("#move-tools");
  if (!controls) {
    controls = document.createElement("div");
    controls.id = "move-tools";
    $("#file-commandbar").prepend(controls);
  }
  const problem = moveProblem(tab().location);
  controls.innerHTML = moveDraft
    ? `<span class="move-count" title="${esc(problem || "Enter to move into " + locations[tab().location]?.[1])}">${moveDraft.ids.length} pending</span><button class="primary" data-column-action="commit" ${problem ? "disabled" : ""}>Move here ↵</button><button data-column-action="cancel">Cancel <kbd>esc</kbd></button>`
    : `<button data-column-action="start" ${!file() ? "disabled" : ""}>${icon("right")} Move <kbd>m</kbd></button>`;
  if (!moveDraft) return;
  const pending = moveDraft.ids
    .map((id) => files.find((f) => f.id === id))
    .filter((f) => f.parent !== tab().location);
  if (!pending.length) return;
  const column = $(".current-column .column-scroll");
  let table = $(".file-table tbody");
  if (!column && !table) {
    const wrap = $(".table-wrap");
    if (!wrap) return;
    wrap.innerHTML = '<table class="file-table"><tbody></tbody></table>';
    table = $(".file-table tbody");
  }
  const target = column || table;
  target.querySelector(".column-empty")?.remove();
  const mode = column ? "columns" : "list";
  for (const f of pending) {
    let markup;
    if (mode === "columns")
      markup = `<div class="column-row pending-file" data-move-preview="${f.id}" aria-label="${esc(f.name)}: pending move">${fileIcon(f)}<span>${esc(f.name)}</span><small>Pending</small></div>`;
    else
      markup = `<tr class="pending-file" data-move-preview="${f.id}" aria-label="${esc(f.name)}: pending move"><td class="file-name"><span>${fileIcon(f)}<span class="name-text">${esc(f.name)}</span></span></td><td class="date-cell">Pending move</td><td class="kind-cell">${f.type}</td><td class="size">${formatSize(f.size)}</td></tr>`;
    target.insertAdjacentHTML("beforeend", markup);
  }
}
function decorateMoveState() {
  inlineMovePreview();
  document.querySelectorAll("[data-file]").forEach((el) => {
    el.classList.toggle(
      "in-transit",
      !!moveDraft?.ids.includes(+el.dataset.file),
    );
    el.classList.toggle("marked", markedFiles.has(+el.dataset.file));
  });
  $("#keyboard-status").textContent = moveDraft
    ? "← / → browse · Enter move here · Esc cancel"
    : "↑ ↓ select · ← parent · → open · m move · x mark";
}
function startMove() {
  const ids = markedFiles.size ? [...markedFiles] : file() ? [selected] : [];
  if (!ids.length) return;
  moveDraft = { ids, source: tab().location };
  browser();
}
function cancelMove() {
  moveDraft = null;
  markedFiles.clear();
  browser();
}
function animateCarry(direction) {
  if (!moveDraft || !animateMoves) return;
  const held = $("[data-move-preview], .in-transit");
  if (held) {
    held.animate(
      [
        { transform: `translateX(${direction * -28}px)`, opacity: 0.35 },
        { transform: "translateX(0)", opacity: 1 },
      ],
      { duration: 180, easing: "ease-out" },
    );
  }
}

function columnNavigate(destination) {
  const goingUp = destination === parentOf(tab().location);
  const oldOwner = folderOwner(tab().location);
  if (!moveDraft) markedFiles.clear();
  nav(destination);
  if (goingUp && oldOwner) {
    selected = oldOwner.id;
    browser();
  }
  animateCarry(goingUp ? -1 : 1);
}
function commitMove() {
  const problem = moveProblem(tab().location);
  if (problem) return toast(problem);
  const moving = moveDraft.ids.map((id) => files.find((f) => f.id === id));
  const previous = moving.map((f) => ({ id: f.id, parent: f.parent }));
  const destination = tab().location;
  const origin = $("[data-move-preview], .in-transit")?.getBoundingClientRect();
  moving.forEach((f) => {
    if (f.type === "Folder") ensureFolder(f);
    f.parent = destination;
  });
  updateFolderPaths();
  moveDraft = null;
  markedFiles.clear();
  selected = moving[0].id;
  render();
  const target = document.querySelector(
    `[data-column-file="${selected}"][data-zone="current"], [data-file="${selected}"]`,
  );
  if (origin && target && animateMoves) {
    const rect = target.getBoundingClientRect(),
      ghost = document.createElement("div");
    ghost.className = "move-ghost";
    ghost.innerHTML =
      fileIcon(moving[0]) +
      `<span>${esc(moving.length === 1 ? moving[0].name : moving.length + " items")}</span>`;
    Object.assign(ghost.style, {
      left: origin.left + "px",
      top: origin.top + "px",
      width: Math.min(rect.width, 300) + "px",
    });
    document.body.append(ghost);
    ghost
      .animate(
        [
          { transform: "translate(0,0)", opacity: 1 },
          {
            transform: `translate(${rect.left - origin.left}px,${rect.top - origin.top}px)`,
            opacity: 0.85,
          },
        ],
        { duration: 340, easing: "cubic-bezier(.2,.7,.2,1)" },
      )
      .finished.then(() => ghost.remove());
    target.animate([{ background: "#c8ead9" }, { background: "transparent" }], {
      duration: 850,
    });
  }
  toast(
    `Moved ${moving.length === 1 ? moving[0].name : moving.length + " items"} to ${locations[destination][0]}.`,
    () => {
      previous.forEach(
        (p) => (files.find((f) => f.id === p.id).parent = p.parent),
      );
      updateFolderPaths();
      render();
    },
  );
}
document.addEventListener(
  "click",
  (e) => {
    const button = e.target.closest("button");
    if (!button) {
      const pane = e.target.closest("[data-pane-location]");
      if (pane && pane.dataset.paneLocation !== tab().location) {
        e.stopImmediatePropagation();
        columnNavigate(pane.dataset.paneLocation);
      }
      return;
    }
    const d = button.dataset;
    if (!moveDraft && (d.go || d.tab || d.view)) markedFiles.clear();
    if (d.columnAction) {
      e.stopImmediatePropagation();
      ({
        start: startMove,
        cancel: cancelMove,
        commit: commitMove,
        motion: () => {
          animateMoves = !animateMoves;
          browser();
        },
      })[d.columnAction]();
      return;
    }
    if (d.columnNav) {
      e.stopImmediatePropagation();
      columnNavigate(d.columnNav);
      return;
    }
    if (d.columnFile) {
      e.stopImmediatePropagation();
      const f = files.find((f) => f.id === +d.columnFile);
      if (d.location !== tab().location) {
        columnNavigate(d.location);
      }
      if (
        f.type === "Folder" &&
        e.target.closest("svg") === button.lastElementChild
      ) {
        columnNavigate(ensureFolder(f));
        return;
      }
      selected = f.id;
      if (!moveDraft && (e.metaKey || e.ctrlKey || e.shiftKey)) {
        if (markedFiles.has(f.id)) markedFiles.delete(f.id);
        else markedFiles.add(f.id);
      }
      browser();
      return;
    }
    if (moveDraft && (d.go || d.tab || d.closeTab || d.action)) {
      const navigation = [
        "back",
        "forward",
        "up",
        "edit-path",
        "help",
        "quick-settings",
      ];
      if (
        d.go &&
        [
          "airdrop",
          "activity",
          "system",
          "settings",
          "trash",
          "recents",
        ].includes(d.go)
      ) {
        e.stopImmediatePropagation();
        toast("Finish or cancel the move before opening a utility.");
      } else if (d.action && !navigation.includes(d.action)) {
        e.stopImmediatePropagation();
        toast("Finish or cancel the current move first.");
      }
    }
  },
  true,
);
document.addEventListener(
  "keydown",
  (e) => {
    if (
      isTool() ||
      tab().query ||
      searchActive ||
      $("#modal").open ||
      /INPUT|SELECT|TEXTAREA/.test(e.target.tagName) ||
      e.metaKey ||
      e.ctrlKey ||
      e.altKey
    )
      return;
    const keys = [
      "m",
      "x",
      "h",
      "j",
      "k",
      "l",
      "ArrowLeft",
      "ArrowRight",
      "ArrowUp",
      "ArrowDown",
      "Enter",
      "Escape",
      " ",
    ];
    if (!keys.includes(e.key)) return;
    if (
      e.key === "Enter" &&
      e.target.closest?.("button") &&
      !e.target.closest?.("[data-file],[data-column-file]")
    )
      return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.key === "m") {
      if (!moveDraft) startMove();
      return;
    }
    if (e.key === "Escape") {
      if (moveDraft) cancelMove();
      else {
        markedFiles.clear();
        browser();
      }
      return;
    }
    if (e.key === "x" && !moveDraft && file()) {
      if (markedFiles.has(selected)) markedFiles.delete(selected);
      else markedFiles.add(selected);
      browser();
      return;
    }
    if (e.key === "Enter" && moveDraft) return commitMove();
    if (["h", "ArrowLeft"].includes(e.key)) {
      const parent = parentOf(tab().location);
      if (parent) columnNavigate(parent);
      return;
    }
    if (["l", "ArrowRight", "Enter"].includes(e.key)) {
      if (file()?.type === "Folder") columnNavigate(ensureFolder(file()));
      else if (!moveDraft) openFile();
      return;
    }
    if (e.key === " ") {
      if (file()?.type !== "Folder") openFile();
      return;
    }
    if (["j", "k", "ArrowDown", "ArrowUp"].includes(e.key)) {
      const list = rows(),
        index = list.findIndex((f) => f.id === selected),
        step = ["j", "ArrowDown"].includes(e.key) ? 1 : -1;
      selected = list[Math.max(0, Math.min(list.length - 1, index + step))]?.id;
      browser();
      document
        .querySelector(`[data-column-file="${selected}"][data-zone="current"]`)
        ?.scrollIntoView({ block: "nearest" });
    }
  },
  true,
);
