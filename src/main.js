const { invoke } = window.__TAURI__.core;
const { convertFileSrc } = window.__TAURI__.core;

const NAME_KEY = "sw_user_name";

let currentView = "ram";
let modalRefreshTimer = null;
let started = false;
let isLoading = false;
let refreshing = false;
let hasLoaded = false;

window.addEventListener("DOMContentLoaded", () => {
  const onboarding  = document.getElementById("onboarding");
  const dashboard   = document.getElementById("dashboard");
  const nameInput   = document.getElementById("name-input");
  const startBtn    = document.getElementById("start-btn");
  const greeting    = document.getElementById("greeting");
  const modal       = document.getElementById("modal");
  const modalTitle  = document.getElementById("modal-title");
  const modalKicker = document.getElementById("modal-kicker");
  const processList = document.getElementById("process-list");

  // ---------- SCREENS ----------
  function showOnly(el) {
    [onboarding, dashboard].forEach((s) => s.classList.toggle("hidden", s !== el));
  }

  function showDashboard(name) {
    greeting.textContent = `Hello, ${name} 👋`;
    showOnly(dashboard);
    startRefreshLoop();
  }

  function showOnboarding() {
    showOnly(onboarding);
    setTimeout(() => nameInput.focus(), 50);
  }

  startBtn.addEventListener("click", () => {
    const name = nameInput.value.trim();
    if (!name) return nameInput.focus();
    localStorage.setItem(NAME_KEY, name);
    showDashboard(name);
  });

  nameInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") startBtn.click();
  });

  const saved = localStorage.getItem(NAME_KEY);
  if (saved) showDashboard(saved);
  else showOnboarding();

  // ---------- LIVE STATS ----------
  function startRefreshLoop() {
    if (started) return;
    started = true;
    refresh();
    setInterval(refresh, 2500);
  }

  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    try {
      const s = await invoke("get_system_snapshot");

      document.getElementById("whisper").textContent = s.whisper;

      const cpu = Math.min(s.cpu_usage, 100);
      document.getElementById("cpu").textContent = `${cpu.toFixed(0)}%`;
      document.getElementById("cpu-bar").style.width = `${cpu}%`;

      document.getElementById("ram").textContent =
        `${s.used_ram_gb.toFixed(1)} / ${s.total_ram_gb.toFixed(1)} GB`;
      document.getElementById("ram-bar").style.width = `${s.ram_percent}%`;

      document.getElementById("disk").textContent =
        `${s.disk_used_gb.toFixed(0)} / ${s.disk_total_gb.toFixed(0)} GB`;
      document.getElementById("disk-bar").style.width = `${s.disk_used_percent}%`;

      const cpuHint = document.getElementById("cpu-hint");
      if (cpu > 80) { cpuHint.textContent = "Very busy"; cpuHint.dataset.level = "high"; }
      else if (cpu > 50) { cpuHint.textContent = "Working"; cpuHint.dataset.level = "mid"; }
      else { cpuHint.textContent = "Relaxed"; cpuHint.dataset.level = "low"; }

      const ramHint = document.getElementById("ram-hint");
      if (s.ram_percent > 80) { ramHint.textContent = "Nearly full"; ramHint.dataset.level = "high"; }
      else if (s.ram_percent > 55) { ramHint.textContent = "Getting full"; ramHint.dataset.level = "mid"; }
      else { ramHint.textContent = "Plenty of space"; ramHint.dataset.level = "low"; }

      const diskHint = document.getElementById("disk-hint");
      if (s.disk_used_percent > 90) { diskHint.textContent = "Critical"; diskHint.dataset.level = "high"; }
      else if (s.disk_used_percent > 75) { diskHint.textContent = "Getting tight"; diskHint.dataset.level = "mid"; }
      else { diskHint.textContent = "Comfortable"; diskHint.dataset.level = "low"; }

      // Network
      const down = s.net_down_mbps || 0;
      const up = s.net_up_mbps || 0;
      const netTotal = down + up;
      document.getElementById("net").textContent =
        `↓ ${down.toFixed(1)} · ↑ ${up.toFixed(1)} MB/s`;
      document.getElementById("net-bar").style.width =
        `${Math.min(netTotal / 50 * 100, 100)}%`;

      const netHint = document.getElementById("net-hint");
      if (netTotal > 20) { netHint.textContent = "Heavy traffic"; netHint.dataset.level = "high"; }
      else if (netTotal > 5) { netHint.textContent = "Active"; netHint.dataset.level = "mid"; }
      else { netHint.textContent = "Idle"; netHint.dataset.level = "low"; }
    } catch (err) {
      console.error("Snapshot error:", err);
    } finally {
      refreshing = false;
    }
  }

  // ---------- MODAL ----------
  document.getElementById("cpu-card").addEventListener("click", () => openModal("cpu"));
  document.getElementById("ram-card").addEventListener("click", () => openModal("ram"));
  document.getElementById("disk-card").addEventListener("click", () => openModal("disk"));
  document.getElementById("net-card").addEventListener("click", () => openModal("net"));

  document.getElementById("modal-close").addEventListener("click", closeModal);
  modal.addEventListener("click", (e) => { if (e.target === modal) closeModal(); });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !modal.classList.contains("hidden")) closeModal();
  });

  function openModal(view) {
    currentView = view;
    hasLoaded = false;
    clearInterval(modalRefreshTimer);

    if (view === "disk") {
      modalKicker.textContent = "Disk usage";
      modalTitle.textContent = "Biggest items in your home";
      loadDiskUsage();
    } else if (view === "net") {
      modalKicker.textContent = "Network activity";
      modalTitle.textContent = "Live interfaces";
      loadNetwork();
      modalRefreshTimer = setInterval(() => {
        if (!modal.classList.contains("hidden") && currentView === "net") {
          loadNetwork();
        }
      }, 3000);
    } else if (view === "cpu") {
      modalKicker.textContent = "CPU consumers";
      modalTitle.textContent = "Top 8 by CPU";
      loadProcesses("cpu");
      modalRefreshTimer = setInterval(() => {
        if (!modal.classList.contains("hidden") && currentView === "cpu" && !isLoading) {
          loadProcesses("cpu");
        }
      }, 6000);
    } else {
      modalKicker.textContent = "Memory consumers";
      modalTitle.textContent = "Top 8 by memory";
      loadProcesses("ram");
      modalRefreshTimer = setInterval(() => {
        if (!modal.classList.contains("hidden") && currentView === "ram" && !isLoading) {
          loadProcesses("ram");
        }
      }, 6000);
    }

    modal.classList.remove("hidden");
  }

  function closeModal() {
    modal.classList.add("hidden");
    clearInterval(modalRefreshTimer);
    modalRefreshTimer = null;
  }

  // ---------- PROCESSES ----------
  async function loadProcesses(sortBy) {
    if (isLoading) return;
    isLoading = true;
    if (!hasLoaded) {
      processList.innerHTML = `<li class="proc-loading">Scanning processes…</li>`;
    }
    try {
      const procs = await invoke("get_top_processes", { sortBy });
      renderProcesses(procs, sortBy);
      hasLoaded = true;
    } catch (err) {
      console.error(err);
      processList.innerHTML = `<li class="proc-loading">Error: ${escapeHtml(String(err))}</li>`;
    } finally {
      isLoading = false;
    }
  }

  function renderProcesses(procs, sortBy) {
    if (!procs || !procs.length) {
      processList.innerHTML = `<li class="proc-loading">No processes found.</li>`;
      return;
    }

    processList.innerHTML = procs
      .map((p, i) => {
        const metric = sortBy === "cpu"
          ? `${p.cpu_usage.toFixed(1)}% CPU`
          : `${p.ram_mb.toFixed(0)} MB`;

        const barPct = sortBy === "cpu"
          ? Math.min(p.cpu_usage, 100)
          : Math.min((p.ram_mb / 2000) * 100, 100);

        const icon = iconHtml(p.name, p.icon_path);

        return `
          <li class="proc-row">
            <span class="proc-rank">${i + 1}</span>
            ${icon}
            <div class="proc-info">
              <div class="proc-name">${escapeHtml(p.name)}</div>
              <div class="proc-meta">PID ${p.pid}</div>
            </div>
            <div class="proc-metric">
              <span class="proc-value">${metric}</span>
              <div class="proc-bar"><div class="proc-bar-fill" style="width:${barPct}%"></div></div>
            </div>
          </li>
        `;
      })
      .join("");
  }

  // ---------- DISK ----------
  async function loadDiskUsage() {
    processList.innerHTML = `<li class="proc-loading">Scanning your home folder… may take a few seconds</li>`;
    try {
      const items = await invoke("get_disk_usage", { path: null });
      renderDisk(items);
    } catch (err) {
      console.error(err);
      processList.innerHTML = `<li class="proc-loading">Error: ${escapeHtml(String(err))}</li>`;
    }
  }

  function renderDisk(items) {
    if (!items || !items.length) {
      processList.innerHTML = `<li class="proc-loading">No large items found.</li>`;
      return;
    }

    const max = items[0].size_mb || 1;

    processList.innerHTML = items
      .map((it, i) => {
        const size = it.size_mb > 1024
          ? `${(it.size_mb / 1024).toFixed(2)} GB`
          : `${it.size_mb.toFixed(0)} MB`;

        const barPct = Math.min((it.size_mb / max) * 100, 100);
        const icon = iconHtml(it.name, null, it.is_dir);

        return `
          <li class="proc-row">
            <span class="proc-rank">${i + 1}</span>
            ${icon}
            <div class="proc-info">
              <div class="proc-name">${escapeHtml(it.name)}</div>
              <div class="proc-meta">${it.is_dir ? "Folder" : "File"}</div>
            </div>
            <div class="proc-metric">
              <span class="proc-value">${size}</span>
              <div class="proc-bar"><div class="proc-bar-fill" style="width:${barPct}%"></div></div>
            </div>
          </li>
        `;
      })
      .join("");
  }

  // ---------- NETWORK ----------
  async function loadNetwork() {
    processList.innerHTML = `<li class="proc-loading">Sampling network…</li>`;
    try {
      const ifaces = await invoke("get_network_interfaces");
      renderNetwork(ifaces);
    } catch (err) {
      console.error(err);
      processList.innerHTML = `<li class="proc-loading">Error: ${escapeHtml(String(err))}</li>`;
    }
  }

  function renderNetwork(ifaces) {
    if (!ifaces || !ifaces.length) {
      processList.innerHTML = `<li class="proc-loading">No active interfaces.</li>`;
      return;
    }

    const maxTotal = Math.max(...ifaces.map(i => i.down_mbps + i.up_mbps), 0.001);

    processList.innerHTML = ifaces
      .map((iface, i) => {
        const total = iface.down_mbps + iface.up_mbps;
        const barPct = Math.min((total / maxTotal) * 100, 100);
        const label = total > 1
          ? `${total.toFixed(1)} MB/s`
          : `${(total * 1024).toFixed(0)} KB/s`;

        return `
          <li class="proc-row">
            <span class="proc-rank">${i + 1}</span>
            <div class="proc-fallback" style="background:rgba(34,211,238,0.15)">🌐</div>
            <div class="proc-info">
              <div class="proc-name">${escapeHtml(iface.name)}</div>
              <div class="proc-meta">↓ ${iface.down_mbps.toFixed(2)} · ↑ ${iface.up_mbps.toFixed(2)} MB/s</div>
            </div>
            <div class="proc-metric">
              <span class="proc-value">${label}</span>
              <div class="proc-bar"><div class="proc-bar-fill" style="width:${barPct}%"></div></div>
            </div>
          </li>
        `;
      })
      .join("");
  }

  // ---------- ICON HELPER ----------
  function iconHtml(name, iconPath, isDir = false) {
    const initial = (name[0] || "?").toUpperCase();
    const hue = (name.charCodeAt(0) * 37) % 360;

    const fallback = isDir
      ? `<div class="proc-fallback proc-folder">📁</div>`
      : `<div class="proc-fallback" style="background:hsl(${hue},60%,45%)">${initial}</div>`;

    if (!iconPath) return fallback;

    const src = convertFileSrc(iconPath);
    const esc = fallback.replace(/"/g, "&quot;").replace(/'/g, "&#39;");
    return `<img class="proc-icon" src="${src}" alt="" onerror="this.outerHTML='${esc}'" />`;
  }

  // ---------- UTIL ----------
  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])
    );
  }
});