// A stand-in screen recording for the `screen` reel: a social feed
// scrolling, captured frame by frame from Chromium (Playwright).
//
//   node feed.js OUT_DIR   → OUT_DIR/screen-feed.mp4 (1080x1920, 24 fps, 10 s)
const { chromium } = require('playwright');
const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const W = 1080, H = 1920, FPS = 24, DUR = 10;
const POSTS = [
  ['Mina Park', '@mina', '#ff7a59', '#ffcf5c', 'Golden hour from the roof today', '12.4k'],
  ['Studio Lahza', '@lahza', '#5b5bff', '#00d4c8', 'New drop: everything is light now', '48.1k'],
  ['Omar R.', '@omar', '#1fbf8f', '#c6f36b', 'Weekend hike, no filter', '3.2k'],
  ['Ava Chen', '@avac', '#ff4f8b', '#9b5cff', 'Moodboard for the launch', '9.8k'],
  ['Kenji', '@kenji', '#ffb020', '#ff5a1f', 'Tokyo at 2am', '22.7k'],
];
const CSS = `
* { box-sizing: border-box; margin: 0; }
body { width: ${W}px; background: #f4f4f6; font-family: Inter, sans-serif; color: #111; }
.bar { position: fixed; top: 0; left: 0; right: 0; height: 150px; background: #ffffffee; display: flex;
       align-items: flex-end; padding: 0 44px 26px; font: 800 52px Inter; letter-spacing: -1px; z-index: 2;
       border-bottom: 1px solid #e3e3e8; }
.feed { padding: 180px 36px 400px; }
.post { background: #fff; border-radius: 36px; margin-bottom: 40px; overflow: hidden; box-shadow: 0 2px 10px #0000000d; }
.head { display: flex; align-items: center; gap: 22px; padding: 30px 32px; }
.av { width: 84px; height: 84px; border-radius: 50%; }
.name { font: 700 34px Inter; } .handle { font: 500 28px Inter; color: #888; }
.img { height: 820px; }
.foot { padding: 28px 32px 36px; font: 500 32px Inter; }
.likes { font: 700 30px Inter; margin-bottom: 10px; }`;

function page() {
  const posts = [...POSTS, ...POSTS].map(([n, h, a, b, text, likes]) => `
    <div class="post"><div class="head"><div class="av" style="background:linear-gradient(135deg,${a},${b})"></div>
    <div><div class="name">${n}</div><div class="handle">${h}</div></div></div>
    <div class="img" style="background:radial-gradient(circle at 30% 30%,${b},${a} 60%,#222 120%)"></div>
    <div class="foot"><div class="likes">&#9829; ${likes} likes</div>${text}</div></div>`).join('');
  return `<html><head><style>${CSS}</style></head><body><div class="bar">Feed</div><div class="feed">${posts}</div></body></html>`;
}

(async () => {
  const out = path.resolve(process.argv[2] || '.');
  fs.mkdirSync(out, { recursive: true });
  const frames = fs.mkdtempSync(path.join(os.tmpdir(), 'feed-'));
  const html = path.join(frames, 'feed.html');
  fs.writeFileSync(html, page());
  const browser = await chromium.launch();
  const pg = await browser.newPage({ viewport: { width: W, height: H } });
  await pg.goto('file://' + html);
  const total = (await pg.evaluate('document.body.scrollHeight')) - H;
  const ease = (x) => 1 - (1 - x) ** 3;
  for (let i = 0; i < FPS * DUR; i++) {
    const t = i / FPS;
    // A thumb scroll: flick, coast, rest; twice.
    const u = Math.min(t / 4.5, 1), v = Math.min(Math.max((t - 5.2) / 4.0, 0), 1);
    const y = (0.35 * ease(u) + 0.3 * ease(v)) * total;
    await pg.evaluate(`window.scrollTo(0, ${y.toFixed(1)})`);
    await pg.screenshot({ path: path.join(frames, String(i).padStart(4, '0') + '.png') });
  }
  await browser.close();
  const mp4 = path.join(out, 'screen-feed.mp4');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-framerate', String(FPS), '-i', path.join(frames, '%04d.png'),
    '-c:v', 'libx264', '-crf', '16', '-pix_fmt', 'yuv420p', mp4]);
  fs.rmSync(frames, { recursive: true });
  console.log(mp4);
})();
