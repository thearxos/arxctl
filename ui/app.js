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

// ---- a live console: append streamed lines, close with a verdict ----
function makeConsole(node) {
  node.innerHTML = '';
  node.hidden = false;
  return {
    line(text) {
      let cls = 'line';
      if (text.startsWith('$ ')) cls += ' cmd';
      else if (/\b(done|installed|recovered|verified|ok|success)\b/i.test(text)) cls += ' ok';
      else if (/\b(error|fail|refused|xx|cannot)\b/i.test(text)) cls += ' err';
      node.appendChild(el('div', cls, escapeHtml(text)));
      node.scrollTop = node.scrollHeight;
    },
    done(ok, okMsg, failMsg) {
      const d = el('div', 'done ' + (ok ? 'good' : 'fail'), ok ? (okMsg || 'Done ✓') : (failMsg || 'Failed'));
      node.appendChild(d); node.scrollTop = node.scrollHeight;
    }
  };
}
const escapeHtml = s => s.replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));

// run a streamed backend action, wiring its topic events into a console
async function runStreamed(cmd, args, topic, node, okMsg) {
  const con = makeConsole(node);
  const off1 = await listen(`${topic}:progress`, e => con.line(e.payload));
  const off2 = await listen(`${topic}:done`, e => { con.done(e.payload, okMsg); off1(); off2(); });
  try { await invoke(cmd, args); }
  catch (err) { con.line(String(err)); }
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
  runStreamed('system_update', {}, 'update', $('#update-console'), 'System up to date ✓'));

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
  if (!weapSel) return;
  runStreamed('weapons_install', { category: weapSel }, 'weapons', $('#weap-console'), `Installed the ${weapSel} arsenal ✓`);
});
$('#btn-weap-remove').addEventListener('click', () => {
  if (!weapSel) return;
  runStreamed('weapons_remove', { category: weapSel }, 'weapons', $('#weap-console'), `Removed the ${weapSel} arsenal ✓`);
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
    row.innerHTML = `<div><div class="kf">${k.flavor}</div><div class="kv">${k.version} · ${k.role}</div></div>
      <div class="grow"></div>${badge}
      ${k.running ? '' : `<button class="btn-g" data-flavor="${k.flavor}">Install</button>`}`;
    const b = row.querySelector('button');
    if (b) b.addEventListener('click', () => runStreamed('kernel_install', { flavor: b.dataset.flavor }, 'kernel', $('#kernel-console'), 'Kernel installed ✓'));
    box.appendChild(row);
  });
};

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
