// Command reveals shortcuts without changing the user's collapsed preference.
let sidebarCollapsed = false;
let commandHeld = false;
const sidebarKeys = new Map([
  ["home", "1"],
  ["recents", "2"],
  ["airdrop", "3"],
  ["desktop", "4"],
  ["documents", "5"],
  ["projects", "6"],
  ["downloads", "7"],
  ["disk", "8"],
  ["icloud", "9"],
  ["external", "0"],
  ["activity", "⇧1"],
  ["system", "⇧2"],
  ["settings", "⇧3"],
  ["trash", "⇧4"],
]);
function updateSidebar() {
  const hidden = sidebarCollapsed && !commandHeld;
  $(".window").classList.toggle("sidebar-collapsed", hidden);
  $(".window").classList.toggle(
    "sidebar-peeking",
    sidebarCollapsed && commandHeld,
  );
  $(".window").classList.toggle("sidebar-shortcuts", commandHeld);
  $("#navigation-sidebar").inert = hidden;
  $("#navigation-sidebar").setAttribute("aria-hidden", String(hidden));
  $("#sidebar-toggle").setAttribute("aria-expanded", String(!hidden));
  $("#sidebar-toggle").setAttribute(
    "aria-label",
    sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar",
  );
  document.querySelectorAll("#sidebar .nav-item").forEach((button) => {
    const key = sidebarKeys.get(button.dataset.go);
    if (!key || button.querySelector(".sidebar-key")) return;
    const hint = document.createElement("kbd");
    hint.className = "sidebar-key";
    hint.textContent = key;
    hint.setAttribute("aria-hidden", "true");
    button.append(hint);
  });
}
$("#sidebar-toggle").addEventListener("click", () => {
  sidebarCollapsed = !sidebarCollapsed;
  updateSidebar();
});
document.addEventListener(
  "keydown",
  (e) => {
    if (e.key === "Meta") {
      commandHeld = true;
      updateSidebar();
      return;
    }
    if (
      !e.metaKey ||
      e.altKey ||
      e.ctrlKey ||
      $("#modal").open ||
      /INPUT|TEXTAREA|SELECT/.test(e.target.tagName)
    )
      return;
    const digit = e.code.startsWith("Digit")
      ? e.code.slice(5)
      : /^[0-9]$/.test(e.key)
        ? e.key
        : null;
    if (!digit) return;
    const shortcut = (e.shiftKey ? "⇧" : "") + digit;
    const entry = [...sidebarKeys].find(([, key]) => key === shortcut);
    if (!entry) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    commandHeld = true;
    updateSidebar();
    document.querySelector(`#sidebar [data-go="${entry[0]}"]`)?.click();
  },
  true,
);
function releaseCommand() {
  if (commandHeld) {
    commandHeld = false;
    updateSidebar();
  }
}
document.addEventListener(
  "keyup",
  (e) => {
    if (e.key === "Meta" || !e.metaKey) releaseCommand();
  },
  true,
);
window.addEventListener("blur", releaseCommand);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) releaseCommand();
});
