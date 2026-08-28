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
loaders.weapons = async () => {
  if ($('#weap-cats').childElementCount) return; // once
  const cats = await invoke('weapons_categories');
  const box = $('#weap-cats');
  cats.forEach(c => {
    const row = el('div', 'cat');
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
  });
};
$('#btn-weap-install').addEventListener('click', () => {
  if (weapSel) handoff($('#weap-note'), 'weapons_install', { category: weapSel }, `Installing the ${weapSel} arsenal`);
});
$('#btn-weap-remove').addEventListener('click', () => {
  if (weapSel) handoff($('#weap-note'), 'weapons_remove', { category: weapSel }, `Removing the ${weapSel} arsenal`);
});

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
  $('#pf-temp').textContent = s.temp_c ? s.temp_c + '°C' : '—';
  $('#pf-driver').textContent = s.driver || '—';
  $('#pf-range').textContent = `${(s.min_mhz/1000).toFixed(1)}–${(s.max_mhz/1000).toFixed(1)} GHz`;
  // governor + epp selects
  fillSelect($('#pf-gov'), s.governors, s.governor);
  const eppCard = $('#pf-epp').closest('.ctlcard');
  if (s.epps.length) { eppCard.style.display = ''; fillSelect($('#pf-epp'), s.epps, s.epp); } else { eppCard.style.display = 'none'; }
  // turbo
  const tc = $('#pf-turbo-card'); tc.style.display = s.turbo_supported ? '' : 'none';
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
