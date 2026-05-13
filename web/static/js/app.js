const translations = {};
let currentLang = "zh-CN";
let currentVersion = "v1.0.0";

document.addEventListener("DOMContentLoaded", async () => {
  await initLang();
  initNavigation();

  document.getElementById("refresh-printers")?.addEventListener("click", loadPrinters);
  document.getElementById("add-device")?.addEventListener("click", addDevice);
  document.getElementById("discover-devices")?.addEventListener("click", discoverDevices);
  document.getElementById("save-settings-btn")?.addEventListener("click", saveSettings);
  document.getElementById("version-btn")?.addEventListener("click", checkUpdates);

  const hashPage = window.location.hash.replace("#", "");
  if (hashPage) {
    showPage(hashPage);
  }

  loadDashboardStats();
  loadDevices();
  loadSettings();
  initSettingsNav();
  initToggles();
  fetchVersion();
  initMobileMenu();
});

async function initLang() {
  const selector = document.getElementById("language-select");
  if (!selector) return;
  currentLang = localStorage.getItem("lang") || "zh-CN";
  selector.value = currentLang;
  await loadTranslations(currentLang);
  selector.addEventListener("change", async (e) => {
    currentLang = e.target.value;
    localStorage.setItem("lang", currentLang);
    await loadTranslations(currentLang);
    applyTranslations();
  });
}

async function loadTranslations(lang) {
  const res = await fetch(`/i18n/${lang}.json`);
  translations[lang] = await res.json();
  applyTranslations();
}

function applyTranslations() {
  const t = translations[currentLang] || {};
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.getAttribute("data-i18n");
    if (t[key]) el.textContent = t[key];
  });
}

function initMobileMenu() {
  const toggle = document.getElementById("menu-toggle");
  const sidebar = document.querySelector(".sidebar");
  if (!toggle || !sidebar) return;

  toggle.addEventListener("click", () => {
    sidebar.classList.toggle("active");
  });

  // 点击内容区域或导航项时自动关闭侧边栏
  document.querySelectorAll(".sidebar nav li").forEach(li => {
    li.addEventListener("click", () => {
      if (window.innerWidth <= 760) {
        sidebar.classList.remove("active");
      }
    });
  });

  document.querySelector(".content").addEventListener("click", () => {
    if (window.innerWidth <= 760) {
      sidebar.classList.remove("active");
    }
  });
}

function initNavigation() {
  const navItems = document.querySelectorAll("nav li");
  navItems.forEach((item) => {
    item.addEventListener("click", () => {
      const pageId = item.getAttribute("data-page");
      showPage(pageId);
    });
  });
}

function showPage(pageId) {
  const navItems = document.querySelectorAll("nav li");
  navItems.forEach((i) => i.classList.remove("active"));
  const targetNav = document.querySelector(`nav li[data-page="${pageId}"]`);
  if (targetNav) {
    targetNav.classList.add("active");
    document.getElementById("page-title").textContent = targetNav.textContent.trim();
  }

  document.querySelectorAll(".page").forEach((p) => p.classList.add("hidden"));
  const page = document.getElementById(pageId);
  if (page) page.classList.remove("hidden");

  window.location.hash = pageId;

  if (pageId === "printers") loadPrinters();
  if (pageId === "client") loadDevices();
  if (pageId === "dashboard") loadDashboardStats();
  if (pageId === "settings") loadSettings();
}

async function loadSettings() {
  const selector = document.getElementById("log-level-select");
  if (!selector) return;
  try {
    const res = await fetch("/api/v1/settings");
    const data = await res.json();
    if (data.log_level) selector.value = data.log_level;
  } catch (err) {
    console.error(err);
  }
}

async function saveSettings() {
  const selector = document.getElementById("log-level-select");
  if (!selector) return;
  const res = await fetch("/api/v1/settings", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ log_level: selector.value }),
  });
  const data = await res.json();
  if (data.status === "success") {
    showNotification(translations[currentLang]?.save_success || "设置已保存", "success");
  } else {
    showNotification(data.error || "保存失败", "error");
  }
}

function initSettingsNav() {
  const navItems = document.querySelectorAll(".settings-nav-item");
  navItems.forEach((item) => {
    item.addEventListener("click", () => {
      navItems.forEach((i) => i.classList.remove("active"));
      item.classList.add("active");
      // 这里可以根据 item 的内容或 ID 切换右侧面板内容
      // 目前示例中保持一个面板，仅切换激活状态
    });
  });
}

function initToggles() {
  document.querySelectorAll(".toggle").forEach((t) => {
    t.addEventListener("click", () => {
      t.classList.toggle("on");
    });
  });
}

async function loadDashboardStats() {
  const res = await fetch("/api/v1/stats");
  const data = await res.json();
  document.getElementById("stat-total").textContent = data.total_printers ?? 0;
  document.getElementById("stat-shared").textContent = data.shared_printers ?? 0;
  document.getElementById("stat-jobs").textContent = data.today_jobs ?? 0;
}

async function loadPrinters() {
  const list = document.getElementById("printer-list");
  list.innerHTML = '<p class="text-muted">Loading...</p>';

  try {
    const res = await fetch("/api/v1/printers");
    const printers = await res.json();
    list.innerHTML = "";

    printers.forEach((p) => {
      const card = document.createElement("div");
      card.className = "printer-card glass";

      const flags = [];
      if (p.analyzed_by_agent) flags.push('<span class="badge analyzed">已分析</span>');
      if (p.shared) flags.push('<span class="badge shared">已共享</span>');
      if (p.added_via_client) flags.push('<span class="badge client">客户端添加</span>');

      // 能力标签（如果有 capabilities 数据）
      if (p.capabilities && typeof p.capabilities === 'object') {
        const caps = p.capabilities;
        if (caps.color) flags.push('<span class="badge color">彩色</span>');
        else flags.push('<span class="badge bw">黑白</span>');
        if (caps.duplex) flags.push('<span class="badge duplex">双面</span>');
        if (caps.a3) flags.push('<span class="badge a3">A3</span>');
      }

      let btnText = p.shared ? (translations[currentLang]?.btn_cancel_share || "取消共享") : (translations[currentLang]?.btn_convert || "共享打印机");
      let btnClass = p.shared ? "btn" : "btn btn-primary";

      if (p.added_via_client) {
        btnText = "断开连接";
        btnClass = "btn btn-danger";
      }

      card.innerHTML = `
        <div class="printer-status">${p.status || "idle"}</div>
        <h4>${p.name}</h4>
        <p class="text-muted">${p.location || "local"}</p>
        <div class="badge-row">${flags.join("")}</div>
        <button class="${btnClass}">${btnText}</button>
      `;
      card.querySelector("button").addEventListener("click", () => {
        if (p.added_via_client) {
          disconnectPrinter(p.name);
        } else if (!p.shared) {
          openShareModal(p.name);
        } else {
          doSharePrinter(p.name, false, "");
        }
      });
      list.appendChild(card);
    });
  } catch (err) {
    list.innerHTML = `<p class="error">${err.message}</p>`;
  }
}

// 打开共享弹窗
let _shareTargetName = '';
function openShareModal(name) {
  _shareTargetName = name;
  document.getElementById('share-modal-printer-name').textContent = name;
  document.getElementById('share-password-input').value = '';
  document.getElementById('share-modal').classList.remove('hidden');
}

document.addEventListener('DOMContentLoaded', () => {
  document.getElementById('share-modal-cancel')?.addEventListener('click', () => {
    document.getElementById('share-modal').classList.add('hidden');
  });
  document.getElementById('share-modal-confirm')?.addEventListener('click', () => {
    const pw = document.getElementById('share-password-input').value;
    document.getElementById('share-modal').classList.add('hidden');
    doSharePrinter(_shareTargetName, true, pw);
  });
});

async function doSharePrinter(name, shared, password) {
  const res = await fetch("/api/v1/printers/share", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, shared, password }),
  });
  const result = await res.json();
  if (result.status === "success") {
    showNotification(result.message || "操作成功", "success");
  } else {
    showNotification(result.error || "操作失败", "error");
  }
  loadDashboardStats();
  loadPrinters();
}

// 保持向后兼容
async function sharePrinter(name, shared) {
  if (shared) {
    openShareModal(name);
  } else {
    await doSharePrinter(name, false, "");
  }
}

async function disconnectPrinter(name) {
  if (!confirm(`确定要断开打印机 "${name}" 的连接吗？`)) return;

  const res = await fetch("/api/v1/client/connect", {
    method: "DELETE",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ local_name: name }),
  });
  const result = await res.json();
  if (result.status === "success") {
    showNotification(result.message || "已成功断开连接", "success");
  } else {
    showNotification(result.error || "操作失败", "error");
  }
  loadDashboardStats();
  loadPrinters();
}

async function discoverDevices() {
  const box = document.getElementById("discovered-list");
  box.innerHTML = '<p class="text-muted">Scanning mDNS...</p>';

  const res = await fetch("/api/v1/client/discover");
  const devices = await res.json();

  if (devices.error) {
    box.innerHTML = `<p class="error">${devices.error}</p>`;
    return;
  }

  if (!devices.length) {
    box.innerHTML = '<p class="text-muted">No LAN devices found.</p>';
    return;
  }

  box.innerHTML = "";
  devices.forEach((d) => {
    const row = document.createElement("div");
    row.className = "discovered-row";
    row.innerHTML = `
      <span>${d.name} (${d.address}:${d.port})</span>
      <button class="btn" data-action="add">Add</button>
    `;
    row.querySelector('[data-action="add"]').addEventListener("click", () => addDiscoveredDevice(d));
    box.appendChild(row);
  });
}

async function addDiscoveredDevice(d) {
  const res = await fetch("/api/v1/client/devices", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: d.name, address: d.address, port: d.port }),
  });
  const data = await res.json();
  if (data.error) return showNotification(data.error, "error");
  showNotification("设备已添加", "success");
  loadDevices();
}

async function addDevice() {
  const name = document.getElementById("device-name").value.trim();
  const address = document.getElementById("device-address").value.trim();
  const port = Number(document.getElementById("device-port").value || "52333");
  if (!address) return showNotification("请输入地址", "error");

  const res = await fetch("/api/v1/client/devices", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, address, port }),
  });
  const data = await res.json();
  if (data.error) return showNotification(data.error, "error");
  showNotification("设备已手动添加", "success");
  loadDevices();
}

async function loadDevices() {
  const list = document.getElementById("device-list");
  list.innerHTML = '<p class="text-muted">Loading...</p>';
  const res = await fetch("/api/v1/client/devices");
  const devices = await res.json();
  list.innerHTML = "";

  if (!devices.length) {
    list.innerHTML = '<p class="text-muted">No remote devices yet.</p>';
    return;
  }

  for (const d of devices) {
    const row = document.createElement("div");
    row.className = "glass mt-20";
    row.innerHTML = `
      <div class="section-header">
        <strong>${d.name}</strong>
        <div>
          <button class="btn" data-action="load">List Shared Printers</button>
          <button class="btn" data-action="delete">Delete</button>
        </div>
      </div>
      <p class="text-muted">${d.address}:${d.port}</p>
      <div id="remote-printers-${d.id}" class="mt-20"></div>
    `;

    row.querySelector('[data-action="delete"]').addEventListener("click", async () => {
      await fetch(`/api/v1/client/devices/${d.id}`, { method: "DELETE" });
      loadDevices();
    });

    row.querySelector('[data-action="load"]').addEventListener("click", () => loadRemotePrinters(d.id));

    list.appendChild(row);
  }
}

async function loadRemotePrinters(deviceId) {
  const box = document.getElementById(`remote-printers-${deviceId}`);
  box.innerHTML = '<p class="text-muted">Loading...</p>';

  const res = await fetch(`/api/v1/client/devices/${deviceId}/printers`);
  const printers = await res.json();
  if (printers.error) {
    box.innerHTML = `<p class="error">${printers.error}</p>`;
    return;
  }

  if (!printers.length) {
    box.innerHTML = '<p class="text-muted">No shared printers found.</p>';
    return;
  }

  box.innerHTML = "";
  printers.forEach((p) => {
    const btn = document.createElement("button");
    btn.className = "btn btn-primary";
    btn.textContent = `Connect ${p.name}`;
    btn.addEventListener("click", () => connectRemotePrinter(deviceId, p.name));
    box.appendChild(btn);
  });
}

async function connectRemotePrinter(deviceId, printerName, password = "") {
  const res = await fetch("/api/v1/client/connect", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ device_id: deviceId, printer_name: printerName, password }),
  });
  const data = await res.json();
  if (res.status === 401 || data.error === 'password_required') {
    // 需要密码，弹出输入框
    const pw = prompt(`请输入打印机 "${printerName}" 的访问密码：`);
    if (pw !== null) {
      await connectRemotePrinter(deviceId, printerName, pw);
    }
    return;
  }
  if (data.status === "success") {
    showNotification(data.message || "已成功连接远程打印机", "success");
  } else {
    showNotification(data.error || "连接失败", "error");
  }
  loadPrinters();
}

function showNotification(message, type = "success") {
  const container = document.getElementById("notification-container");
  if (!container) return;

  const div = document.createElement("div");
  div.className = `notification ${type}`;
  div.innerHTML = `
    <div class="notification-content">
      <strong>${type === "success" ? "成功" : "提示"}</strong>
      <div class="msg-body">${message}</div>
    </div>
    <div class="notification-close">&times;</div>
  `;

  div.querySelector(".notification-close").onclick = () => div.remove();
  container.appendChild(div);

  setTimeout(() => {
    if (div.parentNode) div.remove();
  }, 6000);
}


async function fetchVersion() {
  try {
    const res = await fetch("/api/v1/version");
    const data = await res.json();
    currentVersion = data.version || "dev";
    const btn = document.getElementById("version-btn");
    if (btn) btn.textContent = currentVersion;
  } catch (err) {
    console.error("Fetch version failed:", err);
  }
}

function stripV(v) {
  return v ? v.replace(/^v/, "") : "";
}

async function checkUpdates() {
  const btn = document.getElementById("version-btn");
  const updateLink = document.getElementById("update-link");
  if (!btn) return;

  const t = translations[currentLang] || {};
  const originalText = btn.textContent;
  btn.textContent = "...";
  btn.disabled = true;

  try {
    const res = await fetch("https://api.github.com/repos/kaiyuan/lanPrint/releases/latest");
    if (!res.ok) throw new Error("Failed to fetch");
    const data = await res.json();
    const latestVersion = data.tag_name;

    // 比较时忽略 'v' 前缀
    if (stripV(latestVersion) !== stripV(currentVersion)) {
      if (updateLink) {
        updateLink.href = data.html_url;
        updateLink.style.display = "inline";
        showNotification(`${t['update_available'] || 'New version available: '}${latestVersion}`, "info");
      }
    } else {
      showNotification(t['update_latest'] || "You are using the latest version.", "success");
      if (updateLink) updateLink.style.display = "none";
    }
  } catch (err) {
    console.error("Check update failed:", err);
    showNotification(t['update_failed'] || "Failed to check for updates.", "error");
  } finally {
    btn.textContent = originalText;
    btn.disabled = false;
  }
}
