// Formatting helpers shared by rendering and action review.
function icon(name){const paths={close:'<path d="m6 6 12 12M6 18 18 6"/>',browser:'<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 9h18M7 6.5h.01M10 6.5h.01"/>',agent:'<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m7 9 3 3-3 3m6 0h4"/>',app:'<rect x="4" y="4" width="16" height="16" rx="4"/><path d="M4 9h16"/>',other:'<path d="M8 4 4 8m12-4 4 4M4 16l4 4m12-4-4 4"/><rect x="8" y="8" width="8" height="8" rx="2"/>',lock:'<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',overview:'<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',ports:'<path d="M8 3v5m8-5v5M6 8h12v4a6 6 0 0 1-12 0V8zm6 10v3"/>',history:'<path d="M3 11a9 9 0 1 1 2 7M3 4v7h7m2-5v6l4 2"/>',settings:'<path d="M4 7h16M4 17h16"/><circle cx="9" cy="7" r="3" fill="var(--side)"/><circle cx="15" cy="17" r="3" fill="var(--side)"/>',search:'<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',check:'<path d="m5 12 4 4L19 6"/>',info:'<circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/>',back:'<path d="m10 5-7 7 7 7M3 12h18"/>',resume:'<path d="M3 9a9 9 0 1 1 0 7M3 3v6h6m3-3v6l4 2"/>'};return `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${paths[name]||paths.info}</svg>`}


const GB = 1024 ** 3, MB = 1024 ** 2;

const bytes = b => b >= GB ? (b / GB).toFixed(1) + " GiB" : b >= MB ? Math.round(b / MB) + " MiB" : b >= 1024 ? Math.round(b / 1024) + " KiB" : Math.round(b) + " B";

const dur = s => { s = Math.max(0, Math.floor(s)); const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60); return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : m ? `${m}m` : `${s}s`; };

const pct = (n, d) => d ? Math.round(n * 100 / d) + "%" : "n/a";

const esc = s => String(s ?? "").replace(/[&<>"]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));

const plural = (n, w) => `${n} ${w}${n === 1 ? "" : w.endsWith("s") ? "es" : "s"}`;

const kindTag = k => k === "chat" ? `<span class="tag" title="a conversation UI: keeps the whole exchange in the page and grows with it">chat</span>` : k === "local" ? `<span class="tag" title="served from this machine">local</span>` : "";

const AGENT_LABEL = { claude_code: "Claude Code", codex: "Codex", cursor_agent: "Cursor Agent", gemini_cli: "Gemini CLI", aider: "Aider", open_code: "OpenCode", copilot: "Copilot CLI", open_claw: "OpenClaw" };


const epochNow = () => Date.now() / 1000;


const nowSecs = () => Date.now() / 1000;

// Ages in seconds as of right now, against this machine's clock, so the
// text and the bar keep moving between polls. The bar follows the daemon's
// cycle while it runs; otherwise the window scans on its own every poll.

const compactDuration = seconds => seconds == null || !Number.isFinite(seconds) ? "" : seconds >= 86400 ? Math.floor(seconds / 86400) + "d" : seconds >= 3600 ? Math.floor(seconds / 3600) + "h" : seconds >= 60 ? Math.floor(seconds / 60) + "m" : "<1m";

const signedBytes = n => (n < 0 ? "−" : "+") + bytes(Math.abs(n));

export { icon, GB, bytes, dur, pct, esc, plural, kindTag, AGENT_LABEL, epochNow, nowSecs, compactDuration, signedBytes, MB };
