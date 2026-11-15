// Headless proof: open the page with both keys, search 'gearbox' as guest then as engineer, record every WebSocket
// frame the browser received (the network tab, via the DevTools protocol), and check that no engineer-scope record
// id or title is in any frame received under the guest key. Screenshots go next to the report.
// usage: node scripts/prove.mjs <page url with #guest=..&engineer=..> <out dir> [engineer-only marker ...]
import puppeteer from 'puppeteer-core';
import fs from 'node:fs';

const [url, out, ...markers] = process.argv.slice(2);
const hidden = markers.length ? markers : ['fd-003', 'fd-004', 'fd-007', 'fd-010', 'dp-004', 'tw-1003', 'tw-1005', 'tw-1008', 'torque', '(engineer)'];
fs.mkdirSync(out, { recursive: true });
const browser = await puppeteer.launch({ executablePath: process.env.CHROME ?? '/usr/bin/chromium', headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();
await page.setViewport({ width: 1440, height: 1100 });
const cdp = await page.createCDPSession();
await cdp.send('Network.enable');
let phase = 'load';
const frames = { load: [], guest: [], engineer: [] };
const sockets = new Set();
cdp.on('Network.webSocketCreated', (e) => sockets.add(e.url.replace(/\?.*/, '') + (e.url.includes('at=') ? '?at=<peer>' : '')));
cdp.on('Network.webSocketFrameReceived', (e) => frames[phase].push(Buffer.from(e.response.payloadData, 'base64')));

phase = 'guest';
await page.goto(url.replace('#', '#key=guest&'));
await page.waitForFunction(() => document.body.dataset.ready === 'guest:gearbox', { timeout: 30000 });
const guest = await page.evaluate(() => window.__last);
await page.screenshot({ path: `${out}/guest.png`, fullPage: true });

phase = 'engineer';
await page.click('button[data-k=engineer]');
await page.waitForFunction(() => document.body.dataset.ready === 'engineer:gearbox', { timeout: 30000 });
const engineer = await page.evaluate(() => window.__last);
await page.screenshot({ path: `${out}/engineer.png`, fullPage: true });

await page.setViewport({ width: 390, height: 844 });
await page.screenshot({ path: `${out}/phone.png`, fullPage: false });
await browser.close();

const scan = (bufs) => {
  const all = Buffer.concat(bufs).toString('latin1');
  return { frames: bufs.length, bytes: all.length, found: hidden.filter((m) => all.includes(m)) };
};
const report = {
  at: new Date().toISOString(),
  query: 'gearbox',
  sockets: [...sockets],
  network: { guest: scan(frames.guest), engineer: scan(frames.engineer) },
  guest,
  engineer,
};
fs.writeFileSync(`${out}/report.json`, JSON.stringify(report, null, 2));
const g = report.network.guest, en = report.network.engineer;
console.log(`guest:    ${g.frames} frames, ${g.bytes} bytes received; engineer-only markers found: ${g.found.length ? g.found.join(', ') : 'none'}`);
console.log(`engineer: ${en.frames} frames, ${en.bytes} bytes received; engineer-only markers found: ${en.found.join(', ') || 'none'}`);
for (const [k, r] of [['guest', guest], ['engineer', engineer]])
  for (const a of r.answers)
    console.log(`${k.padEnd(9)} ${String(a.device).padEnd(16)} ${(a.counts ?? []).map((c) => `${c.collection} ${c.matched}->${c.visible}`).join('  ')}  cell ${a.took_ms} ms, round trip ${a.rtt_ms?.toFixed(1)} ms  ${a.ok ? '' : a.err}`);
process.exit(g.found.length === 0 && en.found.length > 0 ? 0 : 1);
