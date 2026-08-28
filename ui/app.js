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
  $('#d-cpu').textContent = s.cpu;
  const pct = s.mem_total ? Math.round(s.mem_used / s.mem_total * 100) : 0;
  $('#d-mem-bar').style.width = pct + '%';
  $('#d-mem-label').textContent = `${(s.mem_used / 1048576).toFixed(1)} / ${(s.mem_total / 1048576).toFixed(1)} GiB  (${pct}%)`;
  $('#deck-distro').textContent = s.distro;
  $('#deck-kernel').textContent = s.kernel;
  invoke('updates_count').then(n => $('#d-updates').textContent = n);
};

// ---- update ----
$('#btn-update').addEventListener('click', () =>
  handoff($('#update-note'), 'system_update', {}, 'The update'));

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
loaders.weapons = async () => {
  if ($('#weap-cats').childElementCount) return; // once
  const box = $('#weap-cats');
  const cats = await invoke('weapons_categories');
  cats.forEach(c => addCatRow(box, c));
  // live arsenal size: ping the real repo index for the total + the uncategorised "other"
  invoke('arsenal_totals').then(t => {
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
  }).catch(() => {});
};
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
};

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
      c.innerHTML = `<div class="net-head">
          <span class="nif">${i.name}</span>
          <span class="meta"><span class="knd">${i.kind}</span>${i.ip ? `<span class="sep">•</span><span class="ip mono">${i.ip}</span>` : ''}${i.link_mbps > 0 ? `<span class="sep">•</span><span>${i.link_mbps >= 1000 ? (i.link_mbps/1000)+' Gb/s link' : i.link_mbps+' Mb/s link'}</span>` : ''}</span>
          <span class="grow"></span><span class="link ${i.up ? 'up' : 'down'}">${i.up ? 'connected' : 'down'}</span>
        </div>
        <div class="net-flows">
          <div class="flow dn"><div class="flow-top"><span class="arrow">↓</span><span class="rate">0</span><span class="unit">B/s</span><span class="grow"></span><span class="tot">↓ 0</span></div><div class="fbar"><i></i></div></div>
          <div class="flow up"><div class="flow-top"><span class="arrow">↑</span><span class="rate">0</span><span class="unit">B/s</span><span class="grow"></span><span class="tot">↑ 0</span></div><div class="fbar"><i></i></div></div>
        </div>`;
      box.appendChild(c);
    });
  }
  // live values
  ifs.forEach(i => {
    const c = box.querySelector(`.net-if[data-if="${CSS.escape(i.name)}"]`); if (!c) return;
    c.querySelector('.link').className = 'link ' + (i.up ? 'up' : 'down');
    c.querySelector('.link').textContent = i.up ? 'connected' : 'down';
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

// ---- services ----
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

// first paint
loaders.dashboard();
