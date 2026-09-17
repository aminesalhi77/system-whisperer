const { invoke } = window.__TAURI__.core;

const NAME_KEY = "sw_user_name";

let currentSort = "ram";
let modalRefreshTimer = null;
let started = false;

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
    setInterval(refresh, 2000);
  }

  async function refresh() {
    try {
      const s = await invoke("get_system_snapshot");

      document.getElementById("whisper").textContent = s.whisper;

      const cpu = Math.min(s.cpu_usage, 100);
      document.getElementById("cpu").textContent = `${cpu.toFixed(0)}%`;
      document.getElementById("cpu-bar").style.width = `${cpu}%`;

      document.getElementById("ram").textContent =
        `${s.used_ram_gb.toFixed(1)} / ${s.total_ram_gb.toFixed(1)} GB`;
      document.getElementById("ram-bar").style.width = `${s.ram_percent}%`;

      const diskUsedPct = s.disk_used_percent;
      document.getElementById("disk").textContent =
        `${s.disk_used_gb.toFixed(0)} / ${s.disk_total_gb.toFixed(0)} GB`;
      document.getElementById("disk-bar").style.width = `${diskUsedPct}%`;

      const cpuHint = document.getElementById("cpu-hint");
      if (cpu > 80) { cpuHint.textContent = "Very busy"; cpuHint.dataset.level = "high"; }
      else if (cpu > 50) { cpuHint.textContent = "Working"; cpuHint.dataset.level = "mid"; }
      else { cpuHint.textContent = "Relaxed"; cpuHint.dataset.level = "low"; }

      const ramHint = document.getElementById("ram-hint");
      if (s.ram_percent > 80) { ramHint.textContent = "Nearly full"; ramHint.dataset.level = "high"; }
      else if (s.ram_percent > 55) { ramHint.textContent = "Getting full"; ramHint.dataset.level = "mid"; }
      else { ramHint.textContent = "Plenty of space"; ramHint.dataset.level = "low"; }

      const diskHint = document.getElementById("disk-hint");
      if (diskUsedPct > 90) { diskHint.textContent = "Critical"; diskHint.dataset.level = "high"; }
      else if (diskUsedPct > 75) { diskHint.textContent = "Getting tight"; diskHint.dataset.level = "mid"; }
      else { diskHint.textContent = "Comfortable"; diskHint.dataset.level = "low"; }
    } catch (err) {
      console.error("Snapshot error:", err);
    }
  }

  // ---------- MODAL ----------
  document.getElementById("cpu-card").addEventListener("click", () => openModal("cpu"));
  document.getElementById("ram-card").addEventListener("click", () => openModal("ram"));

  document.getElementById("modal-close").addEventListener("click", closeModal);
  modal.addEventListener("click", (e) => { if (e.target === modal) closeModal(); });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !modal.classList.contains("hidden")) closeModal();
  });

  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      currentSort = tab.dataset.sort;
      loadProcesses(currentSort);
    });
  });

  function openModal(sortBy) {
    currentSort = sortBy;
    modalKicker.textContent = sortBy === "cpu" ? "CPU hogs" : "Memory hogs";
    modalTitle.textContent = "Top 8 consumers";
    document.querySelectorAll(".tab").forEach((t) => {
      t.classList.toggle("active", t.dataset.sort === sortBy);
    });
    modal.classList.remove("hidden");
    loadProcesses(sortBy);

    clearInterval(modalRefreshTimer);
    modalRefreshTimer = setInterval(() => {
      if (!modal.classList.contains("hidden")) loadProcesses(currentSort);
    }, 3000);
  }

  function closeModal() {
    modal.classList.add("hidden");
    clearInterval(modalRefreshTimer);
  }

  async function loadProcesses(sortBy) {
    processList.innerHTML = `<li class="proc-loading">Scanning processes…</li>`;
    try {
      const procs = await invoke("get_top_processes", { sortBy });
      renderProcesses(procs, sortBy);
    } catch (err) {
      console.error(err);
      processList.innerHTML = `<li class="proc-loading">Error: ${escapeHtml(String(err))}</li>`;
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

        const initial = (p.name[0] || "?").toUpperCase();
        const hue = (p.name.charCodeAt(0) * 37) % 360;

        const iconHtml = `<div class="proc-fallback" style="background:hsl(${hue},60%,45%)">${initial}</div>`;

        return `
          <li class="proc-row">
            <span class="proc-rank">${i + 1}</span>
            ${iconHtml}
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

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])
    );
  }
});