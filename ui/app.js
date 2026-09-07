// ArxOS Control Center — frontend logic. Talks to the Rust backend via the global
// Tauri API (invoke for commands, event.listen for the live streamed actions).
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const el = (t, c, h) => { const e = document.createElement(t); if (c) e.className = c; if (h != null) e.innerHTML = h; return e; };

// ---- navigation ----
const loaders = {};
$$('.nav-item').forEach(b => b.addEventListener('click', () => {
  $$('.nav-item').forEach(x => x.classList.remove('active'));
  $$('.panel').forEach(x => x.classList.remove('active'));
  b.classList.add('active');
  const p = b.dataset.panel;
  $('#p-' + p).classList.add('active');
  loaders[p]?.();
}));

// actions hand off to a terminal (arx runs there); the panel just confirms the launch.
function handoff(node, cmd, args, label) {
  node.hidden = false;
  node.innerHTML = `<div class="launched"><span class="spark"></span>${label} is running in a terminal. Watch it there — it closes on success.</div>`;
  invoke(cmd, args).catch(e => { node.innerHTML = `<div class="launched err">Could not launch: ${e}</div>`; });
}

// ---- dashboard ----
loaders.dashboard = async () => {
  const s = await invoke('system_info');
  $('#d-host').textContent = s.host || '—';
  $('#d-distro').textContent = s.distro;
  $('#d-kernel').textContent = s.kernel;
  $('#d-uptime').textContent = 'up ' + s.uptime;
  $('#d-load').textContent = s.load;
  // memory TYPE (DDR4 · 3200 MT/s) under the bar; fall back to the total where SMBIOS is silent (VMs)
  $('#d-cpu').textContent = s.mem_type ? s.mem_type : `${(s.mem_total / 1048576).toFixed(1)} GiB total`;
  const pct = s.mem_total ? Math.round(s.mem_used / s.mem_total * 100) : 0;
  $('#d-mem-bar').style.width = pct + '%';
  $('#d-mem-label').textContent = `${(s.mem_used / 1048576).toFixed(1)} / ${(s.mem_total / 1048576).toFixed(1)} GiB  (${pct}%)`;
  $('#deck-distro').textContent = s.distro;
  $('#deck-kernel').textContent = s.kernel;
  invoke('updates_count').then(n => $('#d-updates').textContent = n);
  paintDashAnon();
  paintDashStorage();
  paintDashNews();
};

// ArxOS news feed — what's new + improved, from the public metadata repo. Hidden if unreachable.
async function paintDashNews() {
  let items; try { items = await invoke('news_list'); } catch { return; }
  const card = $('#d-news-card'); if (!card) return;
  if (!items || !items.length) { card.hidden = true; return; }
  card.hidden = false;
  $('#d-news-updated').textContent = items[0].date || '';
  $('#d-news-list').innerHTML = items.slice(0, 5).map(n => `
    <div class="news-item">
      <div class="news-head"><span class="news-tag">${wifiEsc(n.tag || '')}</span><b class="news-title">${wifiEsc(n.title || '')}</b><span class="grow"></span><span class="news-date mono dim">${wifiEsc(n.date || '')}</span></div>
      <p class="news-body dim">${wifiEsc(n.body || '')}</p>
    </div>`).join('');
}

// Anonymity at a glance on the dashboard: is the system exiting through Tor, and how to turn it off.
async function paintDashAnon() {
  let a; try { a = await invoke('anond_status'); } catch { return; }
  const dot = $('#d-anon-dot'), st = $('#d-anon-state'), hint = $('#d-anon-hint'), card = $('#d-anon-card');
  const set = (cls, label, tip) => {
    dot.className = 'anon-dot ' + cls;
    st.innerHTML = ''; st.appendChild(dot); st.append(label);
    hint.textContent = tip;
    card.classList.toggle('on', cls === 'on');
  };
  if (a.state === 'Active') set('on', 'Anonymous', 'Exiting via Tor. Privacy tab → Stop to turn off.');
  else if (a.state === 'Bootstrapping') set('busy', `Bootstrapping ${a.bootstrap_pct || 0}%`, 'Connecting to Tor…');
  else if (a.state === 'Locked') set('busy', 'Locked (blocked)', 'Traffic is blocked. Privacy tab → Stop to release.');
  else set('off', 'Off', 'Privacy tab → Go anonymous to route through Tor.');
}

const fmtGiB = b => (b / 1073741824).toFixed(b < 10737418240 ? 1 : 0) + ' GiB';
// Storage overview: every real drive with a used/total bar, plus a line for any attached-but-
// unmounted drive so a freshly plugged-in disk is visible at a glance.
async function paintDashStorage() {
  let s; try { s = await invoke('storage_info'); } catch { return; }
  const list = $('#d-storage-list');
  if (!s.disks || !s.disks.length) { list.innerHTML = '<div class="soon">No drives detected.</div>'; return; }
  list.innerHTML = s.disks.map(d => {
    const cls = d.pct >= 90 ? 'crit' : d.pct >= 75 ? 'warn' : '';
    return `<div class="disk-row">
      <div class="disk-head"><b>${d.mount}</b><span class="dim mono">${d.source}</span>
        <span class="disk-nums mono">${fmtGiB(d.used)} / ${fmtGiB(d.size)} · ${d.pct}%</span></div>
      <div class="meter sm"><i class="${cls}" style="width:${d.pct}%"></i></div></div>`;
  }).join('');
  const root = s.disks.find(d => d.mount === '/') || s.disks[0];
  $('#d-storage-sub').textContent = `${fmtGiB(root.avail)} free on ${root.mount}`;
  const u = $('#d-storage-unmounted');
  if (s.unmounted && s.unmounted.length) {
    u.hidden = false;
    u.textContent = `${s.unmounted.length} drive${s.unmounted.length > 1 ? 's' : ''} attached, not mounted: ${s.unmounted.join(', ')}`;
  } else u.hidden = true;
}

// ---- update ----
// A real per-source count (official repos / AUR / ArxOS tools), refreshed on load, on
// demand, and automatically while this panel stays open — so it never goes stale after
// an update finishes in its handoff terminal without the user having to guess and reopen.
let updPollTimer = null;
const updSrcLabel = { pacman: 'official', aur: 'AUR', tools: 'tool' };
async function paintUpdateCounts() {
  // one live call gives BOTH the counts and the named package list, so they never disagree.
  let l; try { l = await invoke('updates_list'); } catch { return; }
  $('#upd-pacman').textContent = l.pacman ?? 0;
  $('#upd-aur').textContent = l.aur ?? 0;
  $('#upd-tools').textContent = l.tools ?? 0;
  $('#upd-total').textContent = l.total ?? 0;
  $('#upd-live').textContent = 'Last checked ' + new Date().toLocaleTimeString();
  const box = $('#upd-pkgs'); if (!box) return;
  const pkgs = l.packages || [];
  if (!pkgs.length) { box.innerHTML = '<div class="soon">Everything is up to date.</div>'; return; }
  box.innerHTML = `<div class="upd-pkgs-head">${pkgs.length} package${pkgs.length > 1 ? 's' : ''} to update</div>` +
    pkgs.map(p => `<div class="upd-pkg">
        <span class="upd-pkg-src src-${p.source}">${updSrcLabel[p.source] || p.source}</span>
        <b class="upd-pkg-name">${wifiEsc(p.name)}</b>
        <span class="grow"></span>
        <span class="upd-pkg-ver mono">${wifiEsc(p.old)} <span class="arrow">→</span> ${wifiEsc(p.new)}</span>
      </div>`).join('');
}
loaders.update = () => {
  paintUpdateCounts();
  if (updPollTimer) clearInterval(updPollTimer);
  updPollTimer = setInterval(paintUpdateCounts, 15000); // live while this panel is open
};
// stop polling when the user leaves the panel (the nav handler swaps .active classes)
$$('.nav-item').forEach(b => b.addEventListener('click', () => {
  if (b.dataset.panel !== 'update' && updPollTimer) { clearInterval(updPollTimer); updPollTimer = null; }
}));

$('#btn-update').addEventListener('click', () =>
  handoff($('#update-note'), 'system_update', {}, 'The update'));
$('#btn-sync-db').addEventListener('click', () =>
  handoff($('#update-note'), 'sync_databases', {}, 'Syncing package databases'));
$('#btn-refresh-counts').addEventListener('click', paintUpdateCounts);

// ---- weapons (the live arsenal installer) ----
let weapSel = null;
function addCatRow(box, c, extra) {
  const row = el('div', 'cat' + (extra ? ' ' + extra : ''));
  row.innerHTML = `<span class="n">${c.name}</span><span class="c">${c.count}</span>`;
  row.addEventListener('click', () => {
    $$('.cat', box).forEach(x => x.classList.remove('on'));
    row.classList.add('on');
    weapSel = c.name;
    $('#weap-selected').textContent = c.name;
    $('#btn-weap-install').disabled = false;
    $('#btn-weap-remove').disabled = false;
  });
  box.appendChild(row);
}
function paintArsenalTotal(t, box) {
  if (!t.total) return;
  $('#weap-total').hidden = false;
  $('#weap-total').innerHTML = `<b>${t.total.toLocaleString()}</b> tools in the full arsenal <span class="dim">· ${t.curated.toLocaleString()} curated · ${t.other.toLocaleString()} in <b>other</b></span> <span class="live-dot"></span><span class="dim">live</span>`;
  if (t.other && !$('.cat[data-other]', box)) {
    const row = el('div', 'cat other');
    row.setAttribute('data-other', '1');
    row.innerHTML = `<span class="n">other</span><span class="c">${t.other.toLocaleString()}</span>`;
    row.addEventListener('click', () => {
      $$('.cat', box).forEach(x => x.classList.remove('on'));
      row.classList.add('on'); weapSel = 'other';
      $('#weap-selected').textContent = 'other'; $('#btn-weap-install').disabled = false; $('#btn-weap-remove').disabled = false;
    });
    box.appendChild(row);
  }
}
// Community registry opt-in: reflect the current state on every panel open, and wire the toggle
// once. Enabling/disabling goes through pkexec (root writes the /etc/arxos flag arx checks).
async function paintCommunity() {
  const t = $('#community-toggle'), s = $('#community-state');
  if (!t) return;
  let on = false; try { on = await invoke('community_status'); } catch {}
  t.checked = on;
  if (s) s.textContent = on ? 'Enabled — arx pub can install community repos.' : 'Disabled — arx pub install is blocked until you enable it.';
  if (!t.dataset.wired) {
    t.dataset.wired = '1';
    t.addEventListener('change', async (e) => {
      const want = e.target.checked;
      try { const now = await invoke('community_set', { enabled: want }); e.target.checked = now; }
      catch (err) { e.target.checked = !want; } // pkexec cancelled/failed: revert
      paintCommunity();
    });
  }
}
loaders.weapons = async () => {
  paintCommunity();
  if ($('#weap-cats').childElementCount) return; // once
  const box = $('#weap-cats');
  const cats = await invoke('weapons_categories');
  cats.forEach(c => addCatRow(box, c));
  // live arsenal size: ping the real repo index for the total + the uncategorised "other"
  invoke('arsenal_totals').then(t => paintArsenalTotal(t, box)).catch(() => {});
};
// Force refresh re-checks the live arsenal AND rebuilds the XFCE Weapons menu, so the
// desktop menu always tracks the same arsenal this panel shows.
$('#btn-weap-refresh').addEventListener('click', async (e) => {
  e.target.disabled = true; e.target.textContent = 'Refreshing…';
  try {
    paintArsenalTotal(await invoke('arsenal_totals_refresh'), $('#weap-cats'));
    await invoke('weapons_menu_rebuild'); // rebuilds the desktop menu in a terminal
  }
  catch (err) { alert('Could not refresh: ' + err); }
  finally { e.target.disabled = false; e.target.textContent = 'Force refresh arsenal count'; }
});
$('#btn-weap-install').addEventListener('click', () => {
  if (weapSel) handoff($('#weap-note'), 'weapons_install', { category: weapSel }, `Installing the ${weapSel} arsenal`);
});
$('#btn-weap-remove').addEventListener('click', () => {
  if (weapSel) handoff($('#weap-note'), 'weapons_remove', { category: weapSel }, `Removing the ${weapSel} arsenal`);
});
// quick loadouts: default / top 10 / full arsenal map straight to `arx weapons install <sel>`;
// browse opens `arx weapons list-all` so the whole database scrolls by in a terminal.
const LOADOUT_LABEL = { 'default': 'Installing the default loadout', 'top 10': 'Installing the top 10', 'everything': 'Installing the full arsenal' };
$$('.loadout').forEach(b => b.addEventListener('click', () => {
  if (b.hasAttribute('data-browse')) { handoff($('#weap-note'), 'weapons_browse', {}, 'The full arsenal listing'); return; }
  const sel = b.dataset.sel;
  handoff($('#weap-note'), 'weapons_install', { category: sel }, LOADOUT_LABEL[sel] || `Installing ${sel}`);
}));

// ---- kernels ----
loaders.kernels = async () => {
  const list = await invoke('kernels_list');
  const box = $('#kernel-list'); box.innerHTML = '';
  list.forEach(k => {
    const row = el('div', 'krow card');
    const badge = k.running ? '<span class="badge running">running</span>'
      : k.status === 'current' ? '<span class="badge current">current</span>'
      : '<span class="badge retired">retired</span>';
    row.innerHTML = `<div><div class="kf">${k.flavor} <span class="kv">${k.version}</span></div><div class="kv">${k.role}</div></div>
      <div class="grow"></div>${badge}
      <button class="btn-g" data-act="install" data-flavor="${k.flavor}">Install</button>
      ${k.running ? '' : `<button class="btn-g" data-act="remove" data-flavor="${k.flavor}">Remove</button>`}`;
    row.querySelectorAll('button').forEach(b => b.addEventListener('click', () =>
      handoff($('#kernel-note'), b.dataset.act === 'remove' ? 'kernel_remove' : 'kernel_install', { flavor: b.dataset.flavor }, `${b.dataset.act === 'remove' ? 'Removing' : 'Installing'} ${b.dataset.flavor}`)));
    box.appendChild(row);
  });
  paintKernelManifest(); // GitHub-backed features + version history
};

async function paintKernelManifest() {
  let m; try { m = await invoke('kernels_manifest'); } catch { return; }
  // features (what every ArxOS kernel carries)
  const tb = $('#kernel-tunes');
  if (m.tunes && m.tunes.length) {
    tb.innerHTML = '';
    m.tunes.forEach(t => tb.appendChild(el('div', 'card ktune', `<b>${t.name}</b><p class="dim">${t.advantage}</p>`)));
  } else { tb.innerHTML = '<div class="soon">Features unavailable (offline).</div>'; }
  // version history (newest first, changelog under each)
  if (m.updated) $('#kernel-updated').textContent = '· manifest ' + m.updated;
  const hb = $('#kernel-history'); hb.innerHTML = '';
  if (!m.history || !m.history.length) { hb.innerHTML = '<div class="soon">History unavailable (offline).</div>'; return; }
  m.history.forEach(h => {
    const badge = h.status === 'current' ? '<span class="badge current">current</span>' : '<span class="badge retired">retired</span>';
    const row = el('div', 'card khrow');
    row.innerHTML = `<div class="khhead"><span class="khf mono">${h.flavor}</span> <span class="khv mono">${h.version}</span>
      <span class="khd dim">${h.upstream}${h.date ? ' · ' + h.date : ''}</span><span class="grow"></span>${badge}</div>
      <p class="khchg dim">${h.changes}</p>`;
    hb.appendChild(row);
  });
}

// ---- performance (live, direct CPU control) ----
let perfTimer = null;
loaders.performance = async () => {
  await paintPerf();
  clearInterval(perfTimer);
  perfTimer = setInterval(() => { if ($('#p-performance').classList.contains('active')) paintPerf(); else clearInterval(perfTimer); }, 1500);
};
let perfWired = false;
async function paintPerf() {
  let s; try { s = await invoke('perf_status'); } catch { return; }
  // context banner: virtual machine and/or no frequency scaling exposed
  const note = $('#pf-note');
  const inVm = s.virt && s.virt !== 'none';
  if (inVm || !s.cpufreq) {
    note.hidden = false;
    note.className = 'pf-note' + (inVm ? ' vm' : '');
    note.innerHTML = inVm
      ? `<b>Virtual machine detected (${s.virt}).</b> The host owns the physical CPU, so frequency, governor and boost are managed by the host — live temperature and per-core load below are still real.`
      : `<b>CPU frequency scaling isn't exposed on this system.</b> Governor, energy preference and boost aren't available here; live per-core load below is still real.`;
  } else { note.hidden = true; }
  // frequency controls only make sense when the kernel exposes cpufreq
  const freq = s.cpufreq;
  $('#pf-gov').closest('.ctlcard').style.display = freq ? '' : 'none';
  $('.profiles').style.display = freq ? '' : 'none';
  $('#pf-temp').textContent = s.temp_c ? s.temp_c + '°C' : '—';
  $('#pf-driver').textContent = freq ? (s.driver || '—') : 'host-managed';
  $('#pf-range').textContent = freq ? `${(s.min_mhz/1000).toFixed(1)}–${(s.max_mhz/1000).toFixed(1)} GHz` : '';
  // governor + epp selects
  if (freq) fillSelect($('#pf-gov'), s.governors, s.governor);
  const eppCard = $('#pf-epp').closest('.ctlcard');
  if (freq && s.epps.length) { eppCard.style.display = ''; fillSelect($('#pf-epp'), s.epps, s.epp); } else { eppCard.style.display = 'none'; }
  // turbo/boost: show the card whenever cpufreq exists; say plainly when boost isn't offered
  const tc = $('#pf-turbo-card');
  tc.style.display = freq ? '' : 'none';
  $('#pf-turbo-na').hidden = s.turbo_supported;
  $('#pf-turbo-switch').style.display = s.turbo_supported ? '' : 'none';
  $('#pf-turbo').checked = s.turbo;
  // per-core live gauges
  const box = $('#pf-cores'); $('#pf-corecount').textContent = s.cores.length + ' threads';
  if (box.childElementCount !== s.cores.length) { box.innerHTML = ''; s.cores.forEach(c => box.appendChild(el('div', 'core', `<span class="cl mono">${c.mhz ? (c.mhz/1000).toFixed(1) : '—'}</span><div class="cbar"><i></i></div><span class="ci dim mono">${c.id}</span>`))); }
  s.cores.forEach((c, i) => { const n = box.children[i]; if (!n) return; n.querySelector('.cl').textContent = c.mhz ? (c.mhz/1000).toFixed(1) : '—'; const bar = n.querySelector('.cbar i'); bar.style.width = c.load + '%'; bar.style.background = c.load > 80 ? 'linear-gradient(90deg,#e8702a,#ff8340)' : 'linear-gradient(90deg,#e8702a,#e8702a)'; });
  // wire controls once
  if (!perfWired) {
    perfWired = true;
    $('#pf-gov').addEventListener('change', e => invoke('perf_set_governor', { governor: e.target.value }).then(paintPerf).catch(alert));
    $('#pf-epp').addEventListener('change', e => invoke('perf_set_epp', { epp: e.target.value }).then(paintPerf).catch(alert));
    $('#pf-turbo').addEventListener('change', e => invoke('perf_set_turbo', { on: e.target.checked }).then(paintPerf).catch(() => { e.target.checked = !e.target.checked; }));
    $$('.prof').forEach(b => b.addEventListener('click', () => invoke('perf_apply_profile', { profile: b.dataset.prof }).then(paintPerf).catch(alert)));
  }
}
function fillSelect(sel, opts, cur) {
  if (sel.dataset.opts !== opts.join(',')) { sel.innerHTML = ''; opts.forEach(o => sel.appendChild(el('option', null, o))); sel.dataset.opts = opts.join(','); }
  sel.value = cur;
}

// ---- network (live per-interface throughput) ----
const netMax = {}; // per-interface rolling peak, so the bars stay meaningful
function fmtRate(bps) {
  if (bps < 1024) return [bps.toFixed(0), 'B/s'];
  if (bps < 1048576) return [(bps / 1024).toFixed(1), 'KB/s'];
  if (bps < 1073741824) return [(bps / 1048576).toFixed(2), 'MB/s'];
  return [(bps / 1073741824).toFixed(2), 'GB/s'];
}
function fmtTotal(b) {
  if (b < 1048576) return (b / 1024).toFixed(0) + ' KB';
  if (b < 1073741824) return (b / 1048576).toFixed(1) + ' MB';
  if (b < 1099511627776) return (b / 1073741824).toFixed(2) + ' GB';
  return (b / 1099511627776).toFixed(2) + ' TB';
}
let netTimer = null;
loaders.network = async () => {
  await paintNet();
  paintPorts();               // ports change rarely: load once per open, refresh after an action
  paintWifi();
  wireWifiOnce();
  clearInterval(netTimer);
  netTimer = setInterval(() => { if ($('#p-network').classList.contains('active')) paintNet(); else clearInterval(netTimer); }, 1000);
};

async function paintPorts() {
  let ports; try { ports = await invoke('net_ports'); } catch { return; }
  const box = $('#net-ports');
  if (!ports.length) { box.innerHTML = '<div class="soon">Nothing is listening. Nice and locked down.</div>'; return; }
  box.innerHTML = '';
  ports.forEach(p => {
    const svc = p.service || p.process || 'unknown';
    const row = el('div', 'card port');
    row.innerHTML = `
      <span class="pport mono">${p.proto}/${p.port}</span>
      <span class="pexp ${p.exposed ? 'ex' : 'lo'}">${p.exposed ? 'exposed' : 'local only'}</span>
      <div class="pmid"><b class="psvc">${svc}</b><span class="paddr mono dim">${p.addr}${p.unit ? ' · ' + p.unit : ''}</span></div>
      <span class="grow"></span>
      <div class="pacts"></div>`;
    const acts = row.querySelector('.pacts');
    if (p.unit) {
      const b = el('button', 'btn-g sm', 'Disable service');
      b.addEventListener('click', () => doDisable(p, b));
      acts.appendChild(b);
    }
    const bp = el('button', 'btn-g sm', 'Block port');
    bp.addEventListener('click', () => doBlock(p, bp));
    acts.appendChild(bp);
    box.appendChild(row);
  });
}
function sshGuard(p) {
  if (p.port === 22 || p.service === 'ssh' || p.process === 'sshd')
    return confirm('This is SSH (port 22). If you are connected over SSH, closing it will cut your session. Continue?');
  return true;
}
async function doDisable(p, btn) {
  if (!sshGuard(p)) return;
  if (!confirm(`Disable ${p.unit}? It stops now and will not start at boot (reversible with: systemctl enable --now ${p.unit}).`)) return;
  btn.disabled = true; btn.textContent = 'Disabling…';
  try { await invoke('net_disable_service', { unit: p.unit }); } catch (e) { alert('Failed: ' + e); btn.disabled = false; btn.textContent = 'Disable service'; return; }
  paintPorts();
}
async function doBlock(p, btn) {
  if (!sshGuard(p)) return;
  if (!confirm(`Block ${p.proto}/${p.port} at the firewall? The service keeps running but the port goes dark (isolated in the arxos_harden nft table, reversible).`)) return;
  btn.disabled = true; btn.textContent = 'Blocking…';
  try { await invoke('net_block_port', { proto: p.proto, port: p.port }); btn.textContent = 'Blocked'; }
  catch (e) { alert('Failed: ' + e); btn.disabled = false; btn.textContent = 'Block port'; }
}
async function paintNet() {
  let ifs; try { ifs = await invoke('net_status'); } catch { return; }
  const box = $('#net-list');
  if (!ifs.length) { box.innerHTML = '<div class="soon">No interfaces found.</div>'; return; }
  // rebuild the card scaffold only when the interface set changes
  const key = ifs.map(i => i.name).join(',');
  if (box.dataset.key !== key) {
    box.dataset.key = key; box.innerHTML = '';
    ifs.forEach(i => {
      const c = el('div', 'card net-if');
      c.dataset.if = i.name;
      const canToggle = i.kind === 'ethernet' || i.kind === 'wireless';
      c.innerHTML = `<div class="net-head">
          <span class="nif">${i.name}</span>
          <span class="meta"><span class="knd">${i.kind}</span>${i.ip ? `<span class="sep">•</span><span class="ip mono">${i.ip}</span>` : ''}${i.link_mbps > 0 ? `<span class="sep">•</span><span>${i.link_mbps >= 1000 ? (i.link_mbps/1000)+' Gb/s link' : i.link_mbps+' Mb/s link'}</span>` : ''}</span>
          <span class="grow"></span><span class="link ${i.up ? 'up' : 'down'}">${i.up ? 'connected' : 'down'}</span>
          ${canToggle ? `<button class="btn-g sm net-toggle" data-primary="${i.primary ? 1 : 0}">${i.up ? 'Turn off' : 'Turn on'}</button>` : ''}
        </div>
        <div class="net-flows">
          <div class="flow dn"><div class="flow-top"><span class="arrow">↓</span><span class="rate">0</span><span class="unit">B/s</span><span class="grow"></span><span class="tot">↓ 0</span></div><div class="fbar"><i></i></div></div>
          <div class="flow up"><div class="flow-top"><span class="arrow">↑</span><span class="rate">0</span><span class="unit">B/s</span><span class="grow"></span><span class="tot">↑ 0</span></div><div class="fbar"><i></i></div></div>
        </div>`;
      box.appendChild(c);
      const tb = c.querySelector('.net-toggle');
      if (tb) tb.addEventListener('click', () => doIfaceToggle(i.name, tb));
    });
  }
  // live values
  ifs.forEach(i => {
    const c = box.querySelector(`.net-if[data-if="${CSS.escape(i.name)}"]`); if (!c) return;
    c.querySelector('.link').className = 'link ' + (i.up ? 'up' : 'down');
    c.querySelector('.link').textContent = i.up ? 'connected' : 'down';
    const tb = c.querySelector('.net-toggle');
    if (tb && !tb.disabled) { tb.textContent = i.up ? 'Turn off' : 'Turn on'; tb.dataset.primary = i.primary ? 1 : 0; }
    const dn = c.querySelector('.flow.dn'), up = c.querySelector('.flow.up');
    const [dr, du] = fmtRate(i.rx_bps), [ur, uu] = fmtRate(i.tx_bps);
    dn.querySelector('.rate').textContent = dr; dn.querySelector('.unit').textContent = du;
    up.querySelector('.rate').textContent = ur; up.querySelector('.unit').textContent = uu;
    dn.querySelector('.tot').textContent = '↓ ' + fmtTotal(i.rx_total);
    up.querySelector('.tot').textContent = '↑ ' + fmtTotal(i.tx_total);
    // adaptive bar: scale to this interface's rolling peak (min 64 KB/s floor so idle reads low)
    const peak = netMax[i.name] = Math.max((netMax[i.name] || 0) * 0.9, i.rx_bps, i.tx_bps, 65536);
    dn.querySelector('.fbar i').style.width = Math.min(100, i.rx_bps / peak * 100) + '%';
    up.querySelector('.fbar i').style.width = Math.min(100, i.tx_bps / peak * 100) + '%';
  });
}

// Turn an interface on/off. Downing the interface that carries the default route drops the box off
// the network (and cuts a remote session), so we confirm loudly for the primary one.
async function doIfaceToggle(name, btn) {
  const goingUp = btn.textContent.trim() === 'Turn on';
  if (!goingUp && btn.dataset.primary === '1') {
    if (!confirm(`${name} carries this machine's default route. Turning it off drops the box off the network right now (and would cut any remote/SSH session). Continue?`)) return;
  }
  btn.disabled = true; btn.textContent = goingUp ? 'Turning on…' : 'Turning off…';
  try { await invoke('net_iface_set', { name, up: goingUp }); }
  catch (e) { alert('Failed: ' + e); }
  btn.disabled = false;
  setTimeout(paintNet, 900);   // let the link settle, then reflect the new state
}

// ---- Wi-Fi ----
const wifiEsc = s => { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; };
async function paintWifi() {
  let w; try { w = await invoke('net_wifi_state'); } catch { return; }
  $('#wifi-section').hidden = !w.available;   // only shown when the box has a Wi-Fi device
  if (!w.available) return;
  const radio = $('#wifi-radio');
  radio.setAttribute('aria-checked', w.radio_on ? 'true' : 'false');
  radio.disabled = !w.nm;
  radio.title = w.nm ? 'Wi-Fi radio on/off' : 'NetworkManager not active — Wi-Fi controls unavailable';
  $('#wifi-scan').disabled = !w.radio_on || !w.nm;
  if (!w.radio_on) $('#wifi-list').innerHTML = '<div class="soon">Radio is off. Turn it on to scan.</div>';
}
let wifiWired = false;
function wireWifiOnce() {
  if (wifiWired) return; wifiWired = true;
  $('#wifi-radio').addEventListener('click', async () => {
    const on = $('#wifi-radio').getAttribute('aria-checked') === 'true';
    try { await invoke('net_wifi_radio', { on: !on }); } catch (e) { alert('Failed: ' + e); }
    setTimeout(paintWifi, 700);
  });
  $('#wifi-scan').addEventListener('click', scanWifi);
}
async function scanWifi() {
  const list = $('#wifi-list'); list.innerHTML = '<div class="soon">Scanning…</div>';
  let nets; try { nets = await invoke('net_wifi_scan'); } catch { list.innerHTML = '<div class="soon">Scan failed.</div>'; return; }
  if (!nets.length) { list.innerHTML = '<div class="soon">No networks found.</div>'; return; }
  list.innerHTML = '';
  nets.forEach(n => {
    const open = !n.security || n.security === 'open';
    const row = el('div', 'card wifi-row');
    row.innerHTML = `<span class="wifi-ssid"><b>${wifiEsc(n.ssid)}</b>${n.active ? ' <span class="wifi-conn">connected</span>' : ''}</span>
      <span class="wifi-meta mono dim">${n.signal}% · ${open ? 'open' : wifiEsc(n.security)}</span><span class="grow"></span>`;
    if (!n.active) {
      const b = el('button', 'btn-p sm', 'Connect');
      b.addEventListener('click', () => connectWifi(n, b));
      row.appendChild(b);
    }
    list.appendChild(row);
  });
}
async function connectWifi(n, btn) {
  const open = !n.security || n.security === 'open';
  let pw = '';
  if (!open) { pw = prompt(`Password for "${n.ssid}":`); if (pw === null) return; }
  btn.disabled = true; btn.textContent = 'Connecting…';
  try { await invoke('net_wifi_connect', { ssid: n.ssid, password: pw }); }
  catch (e) { alert('Failed: ' + e); btn.disabled = false; btn.textContent = 'Connect'; return; }
  setTimeout(scanWifi, 1600);
}

// ---- privacy (anond, the anonymity daemon) ----
let anonTimer = null;
const ANON_UI = {
  Active:        { cls: 'on',   txt: 'Anonymous — exiting via Tor' },
  Bootstrapping: { cls: 'busy', txt: 'Bootstrapping…' },
  Locked:        { cls: 'busy', txt: 'Locked (traffic blocked)' },
  Draining:      { cls: 'busy', txt: 'Draining…' },
  Down:          { cls: 'off',  txt: 'Off — not anonymous' },
};
loaders.privacy = async () => {
  await paintAnon();
  paintBrowserHardening();
  paintOnion();
  clearInterval(anonTimer);
  anonTimer = setInterval(() => { if ($('#p-privacy').classList.contains('active')) { paintAnon(); paintOnion(); } else clearInterval(anonTimer); }, 2000);
};

// arxonion app-isolation toggle: reflects whether the Tor-only namespace is up, and offers the
// isolated terminal + per-browser isolated launch. Reuses the detected-browser set.
async function paintOnion() {
  let st; try { st = await invoke('arxonion_status'); } catch { return; }
  const toggle = $('#onion-toggle'), actions = $('#onion-actions');
  if (!toggle) return;
  toggle.setAttribute('aria-checked', st.up ? 'true' : 'false');
  toggle.disabled = !st.tor_available && !st.up;   // cannot isolate without Tor (fails closed)
  toggle.title = st.tor_available ? 'Isolate apps you launch into a Tor-only network namespace'
                                  : 'Start anond/Tor first — isolation needs Tor and fails closed without it';
  // Run-in-isolation is available FROM COLD: `arxonion run` makes its own ephemeral namespace and
  // the command starts Tor first (with the bootstrap loader) if it is not up. So wire it once,
  // regardless of whether the persistent namespace toggle is on.
  const appIn = $('#onion-app'), runBtn = $('#onion-run');
  if (runBtn && !runBtn.dataset.wired) {
    const runApp = () => { const v = appIn.value.trim(); if (v) invoke('arxonion_run_app', { app: v }).then(() => appIn.value = '').catch(e => alert(String(e))); };
    runBtn.addEventListener('click', runApp);
    appIn.addEventListener('keydown', e => { if (e.key === 'Enter') runApp(); });
    runBtn.dataset.wired = '1';
  }
  actions.hidden = !st.up;
  if (st.up && !actions.dataset.filled) {
    // populate the per-browser isolated-launch buttons from the detected set
    let found = []; try { found = await invoke('browser_status'); } catch {}
    $('#onion-browsers-label').textContent = found.length ? 'Launch isolated:' : '';
    $('#onion-browsers').innerHTML = found.map(b =>
      `<button class="onion-browsers-btn" data-browser="${b}">${b}</button>`).join(' ');
    $$('#onion-browsers .onion-browsers-btn').forEach(btn =>
      btn.addEventListener('click', () => invoke('arxonion_launch_browser', { browser: btn.dataset.browser }).catch(e => alert(String(e)))));
    actions.dataset.filled = '1';
  }
  if (!st.up) { actions.dataset.filled = ''; }
}
async function paintBrowserHardening() {
  let found; try { found = await invoke('browser_status'); } catch { return; }
  const dot = $('#bh-dot'), detail = $('#bh-detail');
  if (found.length) {
    dot.className = 'dot on';
    detail.textContent = 'Covers ' + found.join(', ') + ' on this system.';
  } else {
    dot.className = 'dot off';
    detail.textContent = 'No supported browser found (Firefox, Waterfox, or Brave).';
  }
  $('#btn-browser-harden').disabled = !found.length;
}
let lastExitIp = '';
async function paintAnon() {
  let s; try { s = await invoke('anond_status'); } catch { return; }
  const u = ANON_UI[s.state] || ANON_UI.Down;
  $('#anon-dot').className = 'anon-dot ' + u.cls;
  // Bootstrapping: show the REAL % + a note that it can take a few minutes, so the panel never
  // looks frozen/dead during Tor's multi-minute bring-up (the % comes live from anond's snapshot).
  if (s.state === 'Bootstrapping') {
    const pct = s.bootstrap_pct || 0;
    $('#anon-state-txt').textContent = `Bootstrapping Tor ${pct}% — this can take a few minutes…`;
    $('#anon-exit').textContent = pct < 50 ? 'connecting to the Tor network…' : 'loading relay descriptors…';
  } else {
    $('#anon-state-txt').textContent = u.txt;
    $('#anon-exit').textContent = s.state === 'Active' && s.exit_ip ? 'exit IP ' + s.exit_ip : '';
  }
  $('#anon-up').disabled = s.state === 'Active';
  $('#anon-down').disabled = s.state === 'Down';

  // the live layer-by-layer picture, straight from anond's own snapshot
  const cell = (id, val, good) => {
    const el = $(id); if (!el) return;
    el.textContent = val;
    el.className = (el.classList.contains('mono') ? 'mono ' : '') + (good ? 'on' : 'off');
  };
  cell('#al-tor', s.tor, s.tor === 'running');
  cell('#al-ks', s.killswitch, s.killswitch === 'armed');
  cell('#al-dns', s.dns, s.dns === 'pinned');
  cell('#al-i2p', s.i2p, s.i2p === 'running');
  cell('#al-ip', s.exit_ip || '—', !!s.exit_ip);
  cell('#al-mac', s.iface ? `${s.iface} · ${s.mac}` : '—', !!s.iface);
  cell('#al-res', s.resolver || '—', s.dns === 'pinned');

  // exit-node location: only look it up when the IP actually changes, so a 2s status
  // refresh does not turn into a lookup every tick
  if (s.exit_ip && s.exit_ip !== lastExitIp) {
    lastExitIp = s.exit_ip;
    $('#al-loc').textContent = 'locating…';
    invoke('anond_exit_location', { ip: s.exit_ip })
      .then(loc => { $('#al-loc').textContent = loc || 'unknown'; $('#al-loc').className = loc ? 'on' : 'off'; })
      .catch(() => { $('#al-loc').textContent = 'unknown'; });
  } else if (!s.exit_ip) {
    lastExitIp = ''; $('#al-loc').textContent = '—'; $('#al-loc').className = 'off';
  }
}

// live output from anond, streamed into the panel instead of an external terminal
const alConsole = () => $('#al-console');
function alLog(line, first) {
  const box = alConsole();
  if (first) box.innerHTML = '';
  const d = el('div', 'ln' + (/fail|refus|error|leak/i.test(line) ? ' hit' : ''), line);
  box.appendChild(d);
  box.scrollTop = box.scrollHeight;
}
listen('anond-line', e => alLog(String(e.payload)));

async function runAnond(action, label) {
  alLog(`> ${label}…`, true);
  try {
    const code = await invoke('anond_action_streamed', { action });
    alLog(code === 0 ? `> ${label} finished.` : `> ${label} exited with code ${code}.`);
  } catch (e) {
    alLog(`> could not run ${label}: ${e}`);
  }
  paintAnon();
}
{
  // i2p is an icon toggle now, not a tick: easier to see at a glance whether it is armed
  const i2p = $('#anon-i2p');
  const i2pOn = () => i2p.getAttribute('aria-checked') === 'true';
  i2p.addEventListener('click', () => i2p.setAttribute('aria-checked', i2pOn() ? 'false' : 'true'));

  $('#anon-up').addEventListener('click', () =>
    runAnond(i2pOn() ? 'up-i2p' : 'up', i2pOn() ? 'Going anonymous (Tor + i2p)' : 'Going anonymous'));
  $('#anon-down').addEventListener('click', () => runAnond('down', 'Stopping anond'));
  $('#anon-verify').addEventListener('click', () => runAnond('verify', 'The leak test'));
  $('#anon-newid').addEventListener('click', () => runAnond('new-identity', 'A new identity'));
  $('#btn-browser-harden').addEventListener('click', () =>
    handoff($('#bh-note'), 'browser_harden', {}, 'Browser hardening'));

  // arxonion app-isolation: the toggle flips the persistent Tor-only namespace on/off; the
  // isolated terminal opens a shell where every command routes through Tor.
  const onion = $('#onion-toggle');
  if (onion) {
    onion.addEventListener('click', async () => {
      const turningOn = onion.getAttribute('aria-checked') !== 'true';
      onion.setAttribute('aria-checked', turningOn ? 'true' : 'false'); // optimistic; paintOnion reconciles
      try { await invoke('arxonion_toggle', { on: turningOn }); } catch (e) { alert(String(e)); }
      setTimeout(paintOnion, 600);
    });
    $('#onion-shell')?.addEventListener('click', () => invoke('arxonion_shell').catch(e => alert(String(e))));
  }
}

// ---- services ----
// ---- VM tools ----
loaders.vms = async () => {
  const box = $('#vm-list');
  let engines; try { engines = await invoke('vm_status'); } catch (e) { box.innerHTML = `<div class="soon">Could not check: ${e}</div>`; return; }
  box.innerHTML = '';
  engines.forEach(v => {
    const c = el('div', 'card row vm-card');
    const btnLabel = v.ready ? 'Reinstall / repair' : v.installed ? 'Finish setup' : 'Install';
    c.innerHTML = `<span class="dot ${v.ready ? 'on' : v.installed ? '' : 'off'}"></span>
      <div class="grow"><b>${v.name}</b><div class="dim" style="font-size:.8rem">${v.detail}</div></div>
      <button class="btn-g sm">${btnLabel}</button>`;
    c.querySelector('button').addEventListener('click', async (e) => {
      e.target.disabled = true; e.target.textContent = 'Launching…';
      try { await invoke('vm_setup', { target: v.id }); }
      catch (err) { alert('Could not start setup: ' + err); }
      finally { e.target.disabled = false; e.target.textContent = btnLabel; }
    });
    box.appendChild(c);
  });
};

loaders.services = async () => {
  const svc = await invoke('services_status');
  const box = $('#svc-list'); box.innerHTML = '';
  svc.forEach(s => {
    const c = el('div', 'card');
    c.innerHTML = `<span class="dot ${s.active ? 'on' : 'off'}"></span><div><b>${s.name}</b><div class="dim" style="font-size:.8rem">${s.active ? 'active' : 'inactive'}</div></div>`;
    box.appendChild(c);
  });
};

// ---- info ----
loaders.info = async () => {
  const s = await invoke('system_info');
  const rows = [['Distribution', s.distro], ['Hostname', s.host], ['Kernel', s.kernel], ['CPU', s.cpu],
    ['Memory', `${(s.mem_total / 1048576).toFixed(1)} GiB`], ['Uptime', s.uptime], ['Load average', s.load]];
  const box = $('#info-list'); box.innerHTML = '';
  rows.forEach(([k, v]) => box.appendChild(el('div', 'card', `<span class="kk">${k}</span><span class="vv">${v || '—'}</span>`)));
};

// ---- wallpaper ----
// The engine (arxos-wallpaper) is the single source of truth for both the wallpaper
// list and the full set of styles the desktop manager itself supports — this panel
// is a thin driver over it, same as every other terminal-handoff panel is a driver
// over arx.
loaders.wallpaper = async () => {
  const box = $('#wall-grid'); const sel = $('#wall-style');
  let cat; try { cat = await invoke('wallpapers_list'); } catch (e) { box.innerHTML = `<div class="soon">Could not scan: ${e}</div>`; return; }
  sel.innerHTML = '';
  cat.styles.forEach(s => sel.appendChild(el('option', null, s)).value = s);
  sel.value = cat.default_style;
  if (!cat.wallpapers.length) { box.innerHTML = '<div class="soon">No backgrounds found.</div>'; return; }
  box.innerHTML = '';
  const dw = cat.desktop?.width, dh = cat.desktop?.height;
  cat.wallpapers.forEach(w => {
    const t = el('div', 'wall-tile');
    t.style.backgroundImage = `url("${encodeURI('file://' + w.path)}")`;
    t.title = w.name;
    const res = w.width && w.height ? `${w.width}×${w.height}` : '';
    const matches = dw && w.width === dw && w.height === dh;
    t.innerHTML = `<div class="wcheck"><svg viewBox="0 0 24 24"><path d="M9 16.2l-3.5-3.5L4 14.2 9 19l11-11-1.4-1.4z"/></svg></div>
      <span class="wsrc wsrc-${w.source}">${w.source}</span>
      ${matches ? '<span class="wmatch" title="Matches this desktop\'s resolution">●</span>' : ''}
      <div class="wname">${w.name}${res ? ` <span class="wres">${res}</span>` : ''}</div>`;
    t.addEventListener('click', async () => {
      $$('.wall-tile', box).forEach(x => x.classList.remove('active'));
      t.classList.add('active');
      try { await invoke('wallpaper_set', { path: w.path, style: sel.value }); }
      catch (e) { t.classList.remove('active'); alert('Could not set wallpaper: ' + e); }
    });
    box.appendChild(t);
  });
  // changing the style re-applies it to whichever tile is currently active
  sel.onchange = () => {
    const active = $('.wall-tile.active', box);
    if (active) invoke('wallpaper_set', { path: cat.wallpapers.find(w => active.title === w.name)?.path, style: sel.value }).catch(alert);
  };

  // toolbar — same feature set as the original standalone tool (slideshow,
  // shuffle, surprise me, download more), all driven through the one engine
  const slideBtn = $('#wall-slide'), shufBtn = $('#wall-shuffle');
  invoke('wallpaper_cycle_status').then(st => {
    slideBtn.classList.toggle('on', st.enabled);
    shufBtn.classList.toggle('on', st.shuffle);
  }).catch(() => {});
  slideBtn.onclick = async () => {
    const on = !slideBtn.classList.contains('on');
    try { const st = await invoke('wallpaper_cycle_set', { enabled: on }); slideBtn.classList.toggle('on', st.enabled); shufBtn.classList.toggle('on', st.shuffle); }
    catch (e) { alert('Could not change slideshow: ' + e); }
  };
  shufBtn.onclick = async () => {
    const on = !shufBtn.classList.contains('on');
    try { const st = await invoke('wallpaper_cycle_set', { shuffle: on }); slideBtn.classList.toggle('on', st.enabled); shufBtn.classList.toggle('on', st.shuffle); }
    catch (e) { alert('Could not change shuffle: ' + e); }
  };
  $('#wall-surprise').onclick = async () => {
    if (!cat.wallpapers.length) return;
    const w = cat.wallpapers[Math.floor(Math.random() * cat.wallpapers.length)];
    try { await invoke('wallpaper_set', { path: w.path, style: sel.value }); }
    catch (e) { alert('Could not set wallpaper: ' + e); }
  };

  const dl = $('#wall-dl'), log = $('#wall-dl-log'), goBtn = $('#wall-dl-go');
  $('#wall-download').onclick = () => { dl.hidden = !dl.hidden; };
  $('#wall-dl-close').onclick = () => { dl.hidden = true; };
  goBtn.onclick = async () => {
    const n = Math.max(1, Math.min(200, parseInt($('#wall-dl-n').value, 10) || 20));
    const source = $('#wall-dl-src').value;
    goBtn.disabled = true; goBtn.textContent = 'Downloading…';
    log.textContent = `Fetching up to ${n} wallpapers (${source})…\n`;
    try {
      const r = await invoke('wallpaper_fetch', { limit: n, source });
      log.textContent += `Found ${r.found} candidates, added ${r.added}:\n`;
      r.files.forEach(f => log.textContent += `  + ${f.split('/').pop()}\n`);
      if (r.errors?.length) log.textContent += `\n${r.errors.length} failed:\n` + r.errors.map(e => '  ! ' + e).join('\n') + '\n';
      log.scrollTop = log.scrollHeight;
      loaders.wallpaper();
    } catch (e) {
      log.textContent += `Failed: ${e}\n`;
    } finally {
      goBtn.disabled = false; goBtn.textContent = 'Download';
    }
  };
};

// ---- per-tab notification indicators ----
// A count pill on Update (pending package updates) and a dot on Kernels (a newer kernel is
// published for an installed flavor). Refreshed at boot and after any update action, so a user
// sees a pending update without opening the tab. No polling loop (idle-is-a-feature doctrine).
const kver = v => (String(v).match(/\d+\.\d+\.\d+/) || [String(v)])[0]; // compare on the x.y.z core
async function refreshBadges() {
  try {
    const n = await invoke('updates_count');
    const b = $('#badge-update');
    if (n > 0) { b.textContent = n > 99 ? '99+' : String(n); b.hidden = false; } else { b.hidden = true; }
  } catch { /* leave the badge as-is on a transient failure */ }
  try {
    const [installed, manifest] = await Promise.all([invoke('kernels_list'), invoke('kernels_manifest')]);
    const cur = {}; (manifest.flavors || []).forEach(f => { cur[f.name] = kver(f.current); });
    const stale = (installed || []).some(k => cur[k.flavor] && kver(k.version) !== cur[k.flavor]);
    $('#dot-kernels').hidden = !stale;
  } catch { /* manifest unreachable: no dot rather than a false one */ }
}

// first paint
loaders.dashboard();
refreshBadges();
