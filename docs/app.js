const REPO = 'GLK-Dev/tortor';
const FALLBACK_TAG = 'v1.8.2';

const I18N = {
  en: {
    'nav-features': 'Features', 'nav-preview': 'GUI Preview', 'nav-platforms': 'Downloads', 'nav-ecosystem': 'GLK Dev & Hey', 'nav-github': 'GitHub',
    'hero-badge': 'RUST 2021 · HEY GUI · IO_URING · QUIC + IPv6',
    'hero-title': 'BitTorrent at <span class="accent">Native Speed</span>. Zero Bloat.',
    'hero-sub': 'High-performance cross-platform BitTorrent client built in Rust with dynamic SIMD verification, Linux io_uring zero-copy I/O, QUIC + TCP dual-stack networking, and a custom GPU-accelerated Hey UI.',
    'hero-dl-latest': 'Download for Desktop', 'hero-view-rel': 'All Releases',
    'tel-status': 'SWARM READY · DUAL-STACK',
    'tel-m1-l': 'Disk Engine', 'tel-m1-v': 'io_uring + Zero-Copy',
    'tel-m2-l': 'Transport', 'tel-m2-v': 'TCP + QUIC (TLS 1.3)',
    'tel-m3-l': 'UI Framework', 'tel-m3-v': 'Hey MVU (GPU/CPU)',
    'tel-m4-l': 'Tests Passed', 'tel-m4-v': '109 Verified',
    'prev-kicker': 'INTERACTIVE PREVIEW', 'prev-title': 'Industrial Dark Dashboard powered by Hey',
    'prev-desc': 'No Electron, no Chromium WebView. TorTor renders directly via wgpu / softbuffer with HiDPI scaling, system TextScaleFactor support, and live RU/EN Fluent localization.',
    'sb-cat': 'CATEGORIES', 'sb-all': 'All', 'sb-dl': 'Downloading', 'sb-seed': 'Seeding', 'sb-paused': 'Paused',
    'th-name': 'Name', 'th-size': 'Size', 'th-prog': 'Progress', 'th-down': 'Down', 'th-up': 'Up', 'th-peers': 'Peers',
    'tab-pieces': 'Piece Map', 'tab-files': 'Files', 'tab-peers': 'Peers', 'tab-info': 'Engine Tuning',
    'feat-kicker': 'ARCHITECTURE & PROTOCOL', 'feat-title': 'Engineered for Throughput & Safety',
    'f1-tag': 'STORAGE', 'f1-title': 'Linux io_uring & Share-Delete Safety',
    'f1-desc': 'Native tokio-uring async disk I/O on Linux, Windows FILE_SHARE_DELETE handles so files stay movable/deletable, and crash-safe atomic fastresume flushes.',
    'f2-tag': 'NETWORKING', 'f2-title': 'Dual-Stack TCP, QUIC & UPnP',
    'f2-desc': 'Single-port IPv4/IPv6 listeners, HTTP/UDP multi-tracker announce-list, Kademlia DHT (BEP 5/32), PEX (BEP 11), UPnP mapping, and TorTor-to-TorTor QUIC.',
    'f3-tag': 'INTEGRITY', 'f3-title': 'Dynamic SIMD & Hardened Parser',
    'f3-desc': 'Every piece is verified via SHA-1 before commit, paths are sanitized against traversal, and bencode payloads enforce strict size and nesting bounds.',
    'f4-tag': 'CONTROL', 'f4-title': '4-Tab Settings & Live Tuning',
    'f4-desc': 'Adjust bandwidth limits, peers per torrent, global inbound cap, upload slots, block pipeline depth, timeouts, PEX/QUIC toggles, and compact UI density.',
    'f5-tag': 'SELECTIVE I/O', 'f5-title': 'Live Multi-File Selection',
    'f5-desc': 'Choose which files inside a torrent to download even mid-swarm. Unselected files are never created on disk and skipped at piece level.',
    'f6-tag': 'SCALING & I18N', 'f6-title': 'DPI, TextScaleFactor & RU/EN UI',
    'f6-desc': 'Proportional scaling across Windows, Linux, and macOS Retina displays with bundled Onest typography and fluent translations.',
    'plat-kicker': 'READY DESKTOP BINARIES', 'plat-title': 'Native GUI Out of the Box on Windows, Linux & macOS',
    'plat-desc': 'Every release tag automatically builds packaged GUI archives via GitHub Actions.',
    'p-win-t': 'Windows x86_64', 'p-win-d': 'Standalone tortor.exe with embedded 7-frame icon, wgpu DirectX/Vulkan rendering, and Windows TextScaleFactor scaling.',
    'p-lin-t': 'Linux x86_64', 'p-lin-d': 'ELF binary with io_uring backend, Wayland & X11 support, GTK3 XDG portal dialogs, plus install.sh and .desktop launcher.',
    'p-mac-t': 'macOS Universal (.app)', 'p-mac-d': 'Universal binary (Apple Silicon arm64 + Intel x86_64) in TorTor.app with .icns icon, Retina HiDPI, .torrent and magnet: handlers.',
    'promo-kicker': 'CREATED BY VITALY GOLIK (MJOJO / GLK DEV)',
    'promo-title': 'Built on Open-Source Rust Foundations: TorTor + Hey',
    'promo-desc': 'TorTor is crafted by Vitaly Golik (mjojo) at GLK Dev as a showcase of fast systems programming paired with our own lightweight Rust GUI framework — Hey.',
    'promo-li1': 'Hey GUI Engine: pure-Rust declarative MVU framework without Chromium/Electron memory bloat (~19 MB binary).',
    'promo-li2': 'End-to-end Rust stack: custom BitTorrent protocol engine, DHT, trackers, UPnP, and GPU rendering in one codebase.',
    'promo-li3': 'Explore more tools, libraries, and desktop software built at GLK Dev.',
    'promo-btn-hey': 'Explore Hey Framework', 'promo-btn-glk': 'GLK Dev on GitHub', 'promo-btn-author': '@mjojo Profile',
    'foot-copy': 'TorTor by Vitaly Golik (mjojo / GLK Dev) · MIT OR Apache-2.0',
  },
  ru: {
    'nav-features': 'Возможности', 'nav-preview': 'Интерфейс', 'nav-platforms': 'Скачать', 'nav-ecosystem': 'GLK Dev и Hey', 'nav-github': 'GitHub',
    'hero-badge': 'RUST 2021 · HEY GUI · IO_URING · QUIC + IPv6',
    'hero-title': 'BitTorrent на <span class="accent">нативной скорости</span>. Без лишнего веса.',
    'hero-sub': 'Быстрый кроссплатформенный BitTorrent-клиент на Rust с динамическим SIMD-хешированием, zero-copy дисковым io_uring в Linux, стеком QUIC + TCP IPv4/IPv6 и GPU-интерфейсом Hey.',
    'hero-dl-latest': 'Скачать для ПК', 'hero-view-rel': 'Все релизы',
    'tel-status': 'РОЙ АКТИВЕН · DUAL-STACK',
    'tel-m1-l': 'Дисковый движок', 'tel-m1-v': 'io_uring + Zero-Copy',
    'tel-m2-l': 'Транспорт', 'tel-m2-v': 'TCP + QUIC (TLS 1.3)',
    'tel-m3-l': 'Движок GUI', 'tel-m3-v': 'Hey MVU (GPU/CPU)',
    'tel-m4-l': 'Автотесты', 'tel-m4-v': '109 пройдено',
    'prev-kicker': 'ИНТЕРАКТИВНЫЙ ПРЕВЬЮ-МАКЕТ', 'prev-title': 'Интерфейс Industrial Dark на базе Hey',
    'prev-desc': 'Никакого Electron и WebView. TorTor рисует интерфейс напрямую через wgpu / softbuffer, поддерживает HiDPI, системный TextScaleFactor и переключение языка RU/EN на лету.',
    'sb-cat': 'КАТЕГОРИИ', 'sb-all': 'Все', 'sb-dl': 'Загрузка', 'sb-seed': 'Раздача', 'sb-paused': 'На паузе',
    'th-name': 'Имя', 'th-size': 'Размер', 'th-prog': 'Прогресс', 'th-down': 'Приём', 'th-up': 'Отдача', 'th-peers': 'Пиры',
    'tab-pieces': 'Карта частей', 'tab-files': 'Файлы', 'tab-peers': 'Пиры', 'tab-info': 'Тюнинг движка',
    'feat-kicker': 'АРХИТЕКТУРА И ПРОТОКОЛ', 'feat-title': 'Спроектирован ради скорости и надёжности',
    'f1-tag': 'ДИСК И ФАЙЛЫ', 'f1-title': 'Linux io_uring и защита от блокировок',
    'f1-desc': 'Асинхронный tokio-uring в Linux, режим FILE_SHARE_DELETE в Windows (файлы можно удалять и переносить во время работы) и атомарный fastresume.',
    'f2-tag': 'СЕТЬ', 'f2-title': 'Dual-Stack TCP, QUIC и UPnP',
    'f2-desc': 'Одновременная работа IPv4 и IPv6 на одном порту, HTTP/UDP-трекеры и announce-list, Kademlia DHT (BEP 5/32), PEX (BEP 11), UPnP и QUIC между клиентами TorTor.',
    'f3-tag': 'БЕЗОПАСНОСТЬ', 'f3-title': 'SIMD-проверка и жёсткий парсер',
    'f3-desc': 'Каждый кусок проверяется по SHA-1 перед записью, пути очищаются от path traversal, а bencode-сообщения ограничены по размеру и глубине.',
    'f4-tag': 'КОНТРОЛЬ', 'f4-title': '4 вкладки тонких настроек',
    'f4-desc': 'Лимиты скорости, число пиров на торрент, входящий лимит, слоты отдачи, глубина конвейера, таймауты, PEX/QUIC и компактный режим интерфейса.',
    'f5-tag': 'ГИБКОСТЬ', 'f5-title': 'Выборочная загрузка файлов',
    'f5-desc': 'Выбирайте нужные файлы в многофайловом торренте даже во время скачивания — невыбранные файлы не создаются на диске.',
    'f6-tag': 'МАСШТАБ И ЯЗЫК', 'f6-title': 'DPI, TextScaleFactor и RU/EN',
    'f6-desc': 'Пропорциональное масштабирование текста, кнопок и колонок таблицы на Windows, Linux и macOS Retina со встроенным шрифтом Onest.',
    'plat-kicker': 'ГОТОВЫЕ СБОРКИ ДЛЯ РАБОЧЕГО СТОЛА', 'plat-title': 'Готовый GUI сразу из коробки для Windows, Linux и macOS',
    'plat-desc': 'Каждый релизный тег автоматически собирает готовые пакеты для всех трёх ОС в GitHub Actions.',
    'p-win-t': 'Windows x86_64', 'p-win-d': 'Автономный tortor.exe со встроенной 7-кадровой иконкой, аппаратным wgpu-рендерингом и поддержкой TextScaleFactor.',
    'p-lin-t': 'Linux x86_64', 'p-lin-d': 'ELF-бинарник с бэкендом io_uring, поддержкой Wayland и X11, диалогами GTK3 XDG portal, скриптом install.sh и .desktop.',
    'p-mac-t': 'macOS Universal (.app)', 'p-mac-d': 'Универсальный бандл TorTor.app (Apple Silicon arm64 + Intel x86_64) с иконкой .icns, Retina HiDPI, .torrent и magnet:.',
    'promo-kicker': 'АВТОР: ВИТАЛИЙ ГОЛИК (MJOJO / GLK DEV)',
    'promo-title': 'Собственный стек на чистом Rust: TorTor + Hey',
    'promo-desc': 'TorTor создан Виталием Голиком (mjojo) в GLK Dev как демонстрация быстрого системного движка в паре с нашим нативным GUI-фреймворком Hey.',
    'promo-li1': 'Движок Hey UI: декларативный MVU-интерфейс на чистом Rust без Chromium и Electron (~19 МБ весь бинарник).',
    'promo-li2': 'Сквозной Rust-стек: собственный BitTorrent-движок, DHT, трекеры, UPnP и GPU-рендеринг в одном проекте.',
    'promo-li3': 'Другие открытые инструменты и проекты команды ищите в профилях GLK Dev и @mjojo.',
    'promo-btn-hey': 'Открыть Hey Framework', 'promo-btn-glk': 'GLK Dev на GitHub', 'promo-btn-author': 'Профиль @mjojo',
    'foot-copy': 'TorTor — Виталий Голик (mjojo / GLK Dev) · Лицензия MIT OR Apache-2.0',
  },
};

const TORRENTS = [
  {
    id: 0, cat: 'downloading', name: 'ubuntu-24.04.2-desktop-amd64.iso', size: '5.8 GB', progress: 74, down: '18.4 MB/s', up: '4.1 MB/s', peers: '42 / 118',
    files: [['ubuntu-24.04.2-desktop-amd64.iso', '5.8 GB', '74%', true], ['SHA256SUMS', '1.2 KB', '100%', true]],
    peersList: [['185.125.190.48:6881', 'TCP', '9.2 MB/s', '1.4 MB/s'], ['[2001:67c:1562::23]:51413', 'TCP v6', '6.1 MB/s', '2.0 MB/s'], ['91.189.91.157:6881', 'QUIC TLS1.3', '3.1 MB/s', '0.7 MB/s']],
  },
  {
    id: 1, cat: 'seeding', name: 'blender-4.3-studio-assets-pack.tar.xz', size: '2.4 GB', progress: 100, down: '—', up: '7.6 MB/s', peers: '19 / 64',
    files: [['scenes/industrial_rig.blend', '1.6 GB', '100%', true], ['textures/pbr_4k_pack.tar', '820 MB', '100%', true]],
    peersList: [['88.198.42.19:6881', 'QUIC TLS1.3', '—', '4.8 MB/s'], ['172.104.210.9:51413', 'TCP', '—', '2.8 MB/s']],
  },
  {
    id: 2, cat: 'paused', name: 'archlinux-2026.11-x86_64.iso', size: '1.1 GB', progress: 38, down: '—', up: '—', peers: '0 / 24',
    files: [['archlinux-2026.11-x86_64.iso', '1.1 GB', '38%', true], ['archlinux-x86_64.iso.sig', '566 B', '100%', true]],
    peersList: [],
  },
];

let state = {
  lang: localStorage.getItem('tortor_lang') || (navigator.language.startsWith('ru') ? 'ru' : 'en'),
  cat: 'all', selected: 0, tab: 'pieces', tick: 0,
};

function applyLang(lang) {
  state.lang = lang;
  localStorage.setItem('tortor_lang', lang);
  document.documentElement.lang = lang;
  const dict = I18N[lang];
  document.querySelectorAll('[data-i18n]').forEach((el) => {
    const key = el.getAttribute('data-i18n');
    if (dict[key]) el.innerHTML = dict[key];
  });
  const btn = document.getElementById('lang-toggle');
  if (btn) btn.textContent = lang === 'en' ? 'RU / EN' : 'EN / RU';
}

function renderMockup() {
  const tbody = document.getElementById('mock-rows');
  const filtered = TORRENTS.filter((t) => state.cat === 'all' || t.cat === state.cat);
  if (!filtered.some((t) => t.id === state.selected) && filtered.length) state.selected = filtered[0].id;

  tbody.innerHTML = filtered.map((t) => {
    const pct = t.id === 0 ? Math.min(99, 74 + (state.tick % 6)) : t.progress;
    const cls = t.cat === 'seeding' ? 'done' : t.cat === 'paused' ? 'paused' : '';
    return `<tr class="torrent-row ${t.id === state.selected ? 'selected' : ''}" data-id="${t.id}">
      <td><strong>${t.name}</strong></td><td><code>${t.size}</code></td>
      <td><div class="progress-wrap"><div class="progress-track"><div class="progress-fill ${cls}" style="width:${pct}%"></div></div><code>${pct}%</code></div></td>
      <td><code class="${t.down !== '—' ? 'status-cyan' : ''}">${t.down}</code></td>
      <td><code class="${t.up !== '—' ? 'status-ok' : ''}">${t.up}</code></td>
      <td><code>${t.peers}</code></td></tr>`;
  }).join('');

  tbody.querySelectorAll('.torrent-row').forEach((row) => {
    row.addEventListener('click', () => { state.selected = Number(row.dataset.id); renderMockup(); });
  });

  const current = TORRENTS.find((t) => t.id === state.selected) || TORRENTS[0];
  const pane = document.getElementById('mock-inspector-body');
  if (state.tab === 'pieces') {
    const total = 96, doneCount = Math.floor(total * (current.progress / 100));
    let cells = '';
    for (let i = 0; i < total; i++) {
      let c = i < doneCount ? 'done' : 'pending';
      if (current.cat === 'downloading' && (i === doneCount + (state.tick % 4) || i === doneCount + ((state.tick + 2) % 5))) c = 'active';
      cells += `<div class="piece-cell ${c}"></div>`;
    }
    pane.innerHTML = `<div class="piece-map">${cells}</div>`;
  } else if (state.tab === 'files') {
    pane.innerHTML = current.files.map(([n, s, p, ok]) => `<div class="stack-row"><span>${ok ? '☑' : '☐'} <code>${n}</code></span><span>${s} · <strong class="status-ok">${p}</strong></span></div>`).join('');
  } else if (state.tab === 'peers') {
    pane.innerHTML = current.peersList.length
      ? current.peersList.map(([a, pr, d, u]) => `<div class="stack-row"><code>${a}</code><span style="color:var(--accent)">${pr}</span><span class="status-cyan">${d}</span><span class="status-ok">${u}</span></div>`).join('')
      : `<div style="color:var(--text-muted);font-size:0.84rem">Paused — 0 active swarm peers.</div>`;
  } else {
    pane.innerHTML = `
      <div class="stack-row"><span>Pipeline Depth / Peers / Upload Slots</span><code>16 blocks · 30 peers · 8 slots</code></div>
      <div class="stack-row"><span>Disk I/O &amp; Integrity</span><code>io_uring (Linux) · StandardDisk + FILE_SHARE_DELETE (Win/macOS)</code></div>
      <div class="stack-row"><span>DHT / PEX / UPnP / QUIC</span><code class="status-ok">Enabled · Port 6881 (Dual-Stack)</code></div>`;
  }
}

function setReleaseLinks(tag) {
  const cleanTag = tag || FALLBACK_TAG;
  const base = `https://github.com/${REPO}/releases/download/${cleanTag}`;
  const map = {
    'dl-win': `${base}/tortor_${cleanTag}_windows_x86_64.zip`, 'card-dl-win': `${base}/tortor_${cleanTag}_windows_x86_64.zip`,
    'dl-lin': `${base}/tortor_${cleanTag}_linux_x86_64.tar.gz`, 'card-dl-lin': `${base}/tortor_${cleanTag}_linux_x86_64.tar.gz`,
    'dl-mac': `${base}/tortor_${cleanTag}_macos_universal.zip`, 'card-dl-mac': `${base}/tortor_${cleanTag}_macos_universal.zip`,
  };
  Object.entries(map).forEach(([id, url]) => { const el = document.getElementById(id); if (el) el.href = url; });
  const verEl = document.getElementById('nav-ver');
  if (verEl) verEl.textContent = cleanTag;

  const ua = navigator.userAgent.toLowerCase();
  const primary = ua.includes('mac')
    ? { label: 'macOS Universal (.app)', url: map['dl-mac'] }
    : ua.includes('linux')
      ? { label: 'Linux x86_64 (.tar.gz)', url: map['dl-lin'] }
      : { label: 'Windows x86_64 (.zip)', url: map['dl-win'] };
  const heroBtn = document.getElementById('hero-primary-dl');
  if (heroBtn) {
    heroBtn.href = primary.url;
    const sub = heroBtn.querySelector('.dl-os');
    if (sub) sub.textContent = `${cleanTag} · ${primary.label}`;
  }
}

document.addEventListener('DOMContentLoaded', () => {
  applyLang(state.lang);
  setReleaseLinks(FALLBACK_TAG);
  renderMockup();
  document.getElementById('lang-toggle').addEventListener('click', () => applyLang(state.lang === 'en' ? 'ru' : 'en'));
  document.querySelectorAll('.sidebar-btn').forEach((btn) => btn.addEventListener('click', () => {
    document.querySelectorAll('.sidebar-btn').forEach((b) => b.classList.remove('active'));
    btn.classList.add('active'); state.cat = btn.dataset.cat; renderMockup();
  }));
  document.querySelectorAll('.tab-btn').forEach((btn) => btn.addEventListener('click', () => {
    document.querySelectorAll('.tab-btn').forEach((b) => b.classList.remove('active'));
    btn.classList.add('active'); state.tab = btn.dataset.tab; renderMockup();
  }));
  setInterval(() => { state.tick += 1; renderMockup(); }, 1400);
  fetch(`https://api.github.com/repos/${REPO}/releases/latest`).then((r) => (r.ok ? r.json() : null)).then((d) => { if (d && d.tag_name) setReleaseLinks(d.tag_name); }).catch(() => {});
});