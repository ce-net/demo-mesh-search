// demo-mesh-search page: one query, fanned out from this browser to every holder of docs.retrieve@1, each over
// wire.ws-door, each Hello signed here with the key you picked. Every holder checks that key's ce-cap chain itself
// and filters before it ranks: what the key does not reach is counted on the device and never sent.
import { DocsRetrieveClient, Grant, SearchResp, Hit } from './gen/DocsRetrieve';

interface Source { at: string }
interface KeyDef { id: string; label: string; abilities: string[]; grant: string }

const $ = <T extends HTMLElement>(s: string) => document.querySelector(s) as T;
const frag = new URLSearchParams(location.hash.slice(1));
let wireBase = frag.get('w') ?? `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/wire`;

function b64u(s: string): Uint8Array {
  const t = atob(s.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (s.length % 4)) % 4));
  return Uint8Array.from(t, (c) => c.charCodeAt(0));
}

const keys: KeyDef[] = [
  { id: 'guest', label: 'Public guest', abilities: ['docs.retrieve:search'], grant: frag.get('guest') ?? '' },
  { id: 'engineer', label: 'Demo engineer', abilities: ['docs.retrieve:search', 'docs.retrieve:scope:engineer'], grant: frag.get('engineer') ?? '' },
];
let current = keys[0];
let sources: Source[] = [];

function grantFor(k: KeyDef, s: Source): Grant {
  const [seed, chain] = k.grant.split('.');
  const url = s.at ? `${wireBase}${wireBase.includes('?') ? '&' : '?'}at=${encodeURIComponent(s.at)}` : wireBase;
  return { url, seed: b64u(seed), chain: b64u(chain) };
}

const conns = new Map<string, { c: DocsRetrieveClient; connect_ms: number; fresh: boolean }>();
async function client(k: KeyDef, s: Source) {
  const id = `${k.id}|${s.at}`;
  const have = conns.get(id);
  if (have) return { ...have, fresh: false };
  const t = performance.now();
  const c = await DocsRetrieveClient.connect(grantFor(k, s), 6000);
  const e = { c, connect_ms: performance.now() - t, fresh: true };
  conns.set(id, e);
  return e;
}

interface Answer { src: Source; ok: boolean; err?: string; r?: SearchResp; rtt_ms?: number; connect_ms?: number; fresh?: boolean }

async function ask(k: KeyDef, s: Source, q: string): Promise<Answer> {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const e = await client(k, s);
      const t = performance.now();
      const r = await e.c.search({ q, k: 20 });
      const rtt = performance.now() - t;
      if ('err' in r) return { src: s, ok: false, err: `${r.err.tag}: ${r.err.f0}` };
      return { src: s, ok: true, r: r.ok, rtt_ms: rtt, connect_ms: e.connect_ms, fresh: e.fresh };
    } catch (err) {
      conns.get(`${k.id}|${s.at}`)?.c.close();
      conns.delete(`${k.id}|${s.at}`);
      if (attempt === 1) return { src: s, ok: false, err: String((err as Error)?.message ?? err) };
    }
  }
  return { src: s, ok: false, err: 'unreachable' };
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
const ms = (v: number | undefined) => (v === undefined ? '-' : v < 10 ? v.toFixed(2) : v.toFixed(0));

function renderSources(as: Answer[]) {
  $('#sources').innerHTML = as
    .map((a, i) => {
      if (!a.ok || !a.r) return `<div class="glass src"><div class="label"><i class="r"></i>Source ${i + 1}</div><div class="sub">no answer: ${esc(a.err ?? '')}</div></div>`;
      const r = a.r;
      const before = r.counts.reduce((n, c) => n + c.matched, 0), after = r.counts.reduce((n, c) => n + c.visible, 0);
      const rows = r.counts
        .map((c) => `<tr><td>${esc(c.collection)}</td><td class="n">${c.matched}</td><td class="n">${c.visible}</td><td class="n ${c.hidden ? 'acc' : ''}">${c.hidden}</td></tr>`)
        .join('');
      return `<div class="glass src ${i === 0 ? 'ticks' : ''}">
        <div class="label"><i class="g"></i>${esc(r.device)}</div>
        <div class="num">${before}<small> &rarr; </small><span class="acc">${after}</span><small> hits</small></div>
        <div class="sub">before &rarr; after the cap filter, as this device reports it</div>
        <table><thead><tr><th>cell / collection</th><th>before</th><th>after</th><th>stayed</th></tr></thead><tbody>${rows}</tbody></table>
        <dl>
          <dt>cell</dt><dd>${esc(r.cell)}</dd>
          <dt>ranker</dt><dd>${esc(r.ranker)}</dd>
          <dt>key reaches</dt><dd>${r.scopes.map(esc).join(', ')}</dd>
          <dt>in the cell</dt><dd>${ms(r.took_ms)} ms</dd>
          <dt>round trip</dt><dd>${ms(a.rtt_ms)} ms</dd>
          <dt>connect</dt><dd>${ms(a.connect_ms)} ms${a.fresh ? '' : ' (reused)'}</dd>
        </dl></div>`;
    })
    .join('');
}

function renderHits(as: Answer[]) {
  const all: (Hit & { device: string; cell: string })[] = [];
  for (const a of as) if (a.ok && a.r) for (const h of a.r.hits) all.push({ ...h, device: a.r.device, cell: a.r.cell.split(' ')[0] });
  all.sort((x, y) => y.score - x.score);
  $('#count').textContent = String(all.length);
  $('#hits').innerHTML = all.length
    ? all
        .map(
          (h) => `<li class="hit"><div class="meta"><span>${esc(h.device)}</span><span>${esc(h.cell)} / ${esc(h.collection)}</span>${h.scope !== 'public' ? `<span class="scope">${esc(h.scope)}</span>` : ''}<span class="score">${h.score.toFixed(3)}</span></div>
          <div class="t">${esc(h.title)}</div><div class="s">${esc(h.snippet)}</div></li>`,
        )
        .join('')
    : '<li class="hit empty">No result this key may see.</li>';
}

let seq = 0;
async function run() {
  const q = ($('#q') as HTMLInputElement).value.trim();
  if (!q) return;
  const my = ++seq;
  $('#status').textContent = `asking ${sources.length} devices as ${current.label.toLowerCase()}...`;
  const t = performance.now();
  const as = await Promise.all(sources.map((s) => ask(current, s, q)));
  if (my !== seq) return;
  const total = performance.now() - t;
  renderSources(as);
  renderHits(as);
  const ok = as.filter((a) => a.ok).length;
  $('#status').textContent = `${ok} of ${as.length} devices answered in ${total.toFixed(0)} ms, as ${current.label.toLowerCase()}`;
  (window as any).__last = { key: current.id, q, answers: as.map((a) => ({ ok: a.ok, err: a.err, device: a.r?.device, counts: a.r?.counts, hits: a.r?.hits.map((h) => h.id), took_ms: a.r?.took_ms, rtt_ms: a.rtt_ms })) };
  document.body.dataset.ready = `${current.id}:${q}`;
}

function renderKeys() {
  $('#keys').innerHTML = keys.map((k) => `<button aria-pressed="${k === current}" data-k="${k.id}" ${k.grant ? '' : 'disabled'}>${esc(k.label)}</button>`).join('');
  $('#abilities').textContent = current.abilities.join('  ');
}

async function main() {
  const theme = (() => { try { return localStorage.getItem('ce-theme'); } catch { return null; } })();
  if (theme === 'light') document.documentElement.dataset.theme = 'light';
  $('#theme').onclick = () => {
    const l = document.documentElement.dataset.theme !== 'light';
    if (l) document.documentElement.dataset.theme = 'light'; else delete document.documentElement.dataset.theme;
    try { localStorage.setItem('ce-theme', l ? 'light' : 'dark'); } catch {}
  };
  const cfg = await fetch('sources.json').then((r) => r.json()).catch(() => ({ sources: [{ at: '' }] }));
  if (!frag.get('w') && cfg.wire) wireBase = cfg.wire;
  sources = (frag.get('src')?.split(',').map((at) => ({ at: at === 'self' ? '' : at })) ?? cfg.sources) as Source[];
  if (!keys[0].grant) {
    $('#status').textContent = 'This link carries no keys. Open it from the link the demo prints (it puts the two keys in the # part, which never leaves your browser).';
    renderKeys();
    return;
  }
  renderKeys();
  $('#keys').onclick = (e) => {
    const b = (e.target as HTMLElement).closest('button');
    if (!b) return;
    current = keys.find((k) => k.id === b.dataset.k)!;
    renderKeys();
    run();
  };
  $('#form').onsubmit = (e) => { e.preventDefault(); run(); };
  const want = frag.get('key');
  if (want) current = keys.find((k) => k.id === want) ?? current;
  renderKeys();
  run();
}
main();
