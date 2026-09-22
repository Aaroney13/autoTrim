import { esc, dur } from "./format.js";

const steps = ["Welcome", "Your focus", "Preferences", "Chrome tabs", "Startup"];
const areas = [
  { id: "browser", label: "Browser tabs", detail: "Find idle Chrome tabs and see browser memory use.", apps: [["chrome", "Chrome"], ["safari", "Safari"]], note: "Tab cleanup is available for Chrome. Safari: memory monitoring." },
  { id: "agent", label: "AI sessions", detail: "Keep track of idle sessions and the memory they hold.", apps: [["codex", "Codex"], ["claude", "Claude"], ["cursor", "Cursor"]], note: "Codex, Claude, Cursor & more" },
  { id: "app", label: "Apps & memory", detail: "Spot heavy apps and watch memory use over time.", apps: [["activity-monitor", "Activity Monitor"]], note: "A clearer view of your Mac" },
];
const appLogos = apps => `<span class="setup-app-logos">${apps.map(([id, name]) => `<img src="brand/${id}.png" alt="${name}" width="40" height="40">`).join("")}</span>`;
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
// Exact hosts only: choosing Google must never authorize Gmail or Docs.
const starterSites = [
  { id: "google", label: "Google searches", domains: ["google.com", "www.google.com"], detail: "Search results and other pages on these two domains. Gmail and Docs aren’t included." },
  { id: "reddit", label: "Reddit", domains: ["reddit.com", "www.reddit.com", "old.reddit.com"], detail: "Feeds and threads you can come back to." },
  { id: "instagram", label: "Instagram", domains: ["instagram.com", "www.instagram.com"], detail: "Posts, reels, and profiles." },
  { id: "facebook", label: "Facebook", domains: ["facebook.com", "www.facebook.com"], detail: "Feeds, posts, and profiles." },
  { id: "x", label: "X / Twitter", domains: ["x.com", "www.x.com", "twitter.com", "www.twitter.com"], detail: "Timelines and posts." },
  { id: "tiktok", label: "TikTok", domains: ["tiktok.com", "www.tiktok.com"], detail: "Videos and profiles." },
];

export function initializeOnboarding(onSaved) {
  const dialog = document.getElementById("onboarding");
  let step = 0, draft, supported = false, busy = false, opening = false, replay = false, autoEnabled = false;

  let suggestions = null, reviewIndex = 0, scanError = "", scanning = false, scanToken = 0;
  let tabHours = 24, tabMode = "off";
  let chromeReview = false, existingRules = [];
  const alreadyListed = domain => existingRules.some(rule => domain === rule.domain || (rule.include_subdomains && domain.endsWith(`.${rule.domain}`)));
  const starterDomains = site => site.domains.filter(domain => !alreadyListed(domain));
  const hasBrowser = () => draft.focus_areas.includes("browser");
  const visibleSteps = () => steps.map((label, index) => ({ label, index })).filter(({ index }) => index !== 3 || hasBrowser());
  const selectedDomains = () => hasBrowser() ? draft.add_tab_domains : [];

  function move(next) {
    const direction = next < step ? "back" : "forward";
    step = next;
    draw("", direction);
    if (step === 3 && chromeReview && suggestions === null && !scanning) void scanTabs();
  }

  async function scanTabs() {
    const token = ++scanToken;
    scanning = true;
    scanError = "";
    draw();
    try {
      const rows = await invoke("suggest_tab_rules");
      if (token !== scanToken) return;
      suggestions = rows;
      reviewIndex = 0;
    } catch (e) {
      if (token !== scanToken) return;
      scanError = String(e);
    } finally {
      if (token === scanToken) {
        scanning = false;
        if (dialog.open && step === 3 && chromeReview) draw("", "forward");
      }
    }
  }

  function draw(error = "", direction = "") {
    const progress = visibleSteps();
    const reviewing = step === 3 && chromeReview && (scanning || reviewIndex < (suggestions?.length || 0));
    dialog.innerHTML = `<form id="setup-form" class="setup-shell">
      <div class="setup-brand"><img class="setup-logo" src="logo.png" width="28" height="28" alt=""><b>autoTrim</b>
      <ol class="setup-progress" aria-label="Setup progress">${progress.map(({label, index: i}, position) => `<li aria-label="Step ${position + 1} of ${progress.length}: ${label}" ${i === step ? 'aria-current="step"' : ""} class="${i < step ? "done" : ""}"><span aria-hidden="true"></span></li>`).join("")}</ol>
      <button type="button" id="setup-later">${replay ? "Cancel" : "Set up later"}</button></div>
      <div class="setup-page ${direction ? `setup-enter-${direction}` : ""}">${content()}</div>
      <p class="setup-error" id="setup-error" role="alert">${esc(error)}</p>
      <div class="setup-footer" ${step === 0 ? "hidden" : ""}><button type="button" id="setup-back">Back</button><span>Step ${progress.findIndex(item => item.index === step) + 1} of ${progress.length}</span>${step === 0 ? "" : `<button class="primary setup-next ${reviewing ? "setup-review-later" : ""}" type="submit">${step === 4 ? "Finish setup" : reviewing ? "Review later" : "Continue"}</button>`}</div>
    </form>`;
    const form = dialog.querySelector("form");
    form.onsubmit = async event => {
      event.preventDefault();
      if (busy || !form.reportValidity()) return;
      if (step === 1 && !draft.focus_areas.length) {
        dialog.querySelector("#setup-error").textContent = "Choose at least one area to focus on.";
        return;
      }
      if (step < 4) { move(step === 2 && !hasBrowser() ? 4 : step + 1); return; }
      busy = true;
      dialog.setAttribute("aria-busy", "true");
      form.querySelectorAll("input, button").forEach(el => el.disabled = true);
      dialog.querySelector(".setup-next").textContent = "Saving…";
      let settings;
      try { settings = await invoke("complete_onboarding", { choices: { ...draft, add_tab_domains: selectedDomains() } }); }
      catch (e) { busy = false; dialog.removeAttribute("aria-busy"); draw(String(e)); return; }
      busy = false;
      dialog.removeAttribute("aria-busy");
      dialog.close();
      await onSaved(settings);
    };
    dialog.querySelector("#setup-back").onclick = () => {
      if (step === 3 && chromeReview) { chromeReview = false; draw("", "back"); return; }
      move(step === 4 && !hasBrowser() ? 2 : step - 1);
    };
    dialog.querySelector("#setup-later").onclick = () => dialog.close();
    const accountButton = dialog.querySelector("#setup-account");
    if (accountButton) accountButton.onclick = () => {
      dialog.querySelector("#setup-account-status").hidden = false;
      accountButton.setAttribute("aria-expanded", "true");
    };
    form.querySelectorAll("[data-focus]").forEach(input => input.onchange = () => {
      draft.focus_areas = [...form.querySelectorAll("[data-focus]:checked")].map(el => el.value);
      dialog.querySelector("#setup-error").textContent = "";
    });
    form.querySelectorAll("[data-number]").forEach(input => input.oninput = () => { draft[input.dataset.number] = Number(input.value); });
    form.querySelectorAll("[data-bool]").forEach(input => input.onchange = () => { draft[input.dataset.bool] = input.checked; });
    form.querySelectorAll('[name="startup"]').forEach(input => input.onchange = () => { draft.background = input.value === "background"; });
    form.querySelectorAll("[data-starter]").forEach(input => input.onchange = () => {
      const domains = starterDomains(starterSites.find(site => site.id === input.dataset.starter));
      draft.add_tab_domains = draft.add_tab_domains.filter(domain => !domains.includes(domain));
      if (input.checked) draft.add_tab_domains.push(...domains);
      input.indeterminate = false;
      dialog.querySelector("#setup-starter-count").textContent = starterCount();
    });
    form.querySelectorAll("[data-partial]").forEach(input => { input.indeterminate = true; });
    const review = dialog.querySelector("#setup-review-tabs");
    if (review) review.onclick = () => { chromeReview = true; move(3); };
    const decide = (add) => {
      const domain = suggestions[reviewIndex].domain;
      draft.add_tab_domains = draft.add_tab_domains.filter(item => item !== domain);
      if (add) draft.add_tab_domains.push(domain);
      reviewIndex++;
      draw("", "forward");
    };
    const add = dialog.querySelector("#setup-domain-add");
    if (add) add.onclick = () => decide(true);
    const skip = dialog.querySelector("#setup-domain-skip");
    if (skip) skip.onclick = () => decide(false);
    const previous = dialog.querySelector("#setup-domain-previous");
    if (previous) previous.onclick = () => { reviewIndex--; draw("", "back"); };
    const restart = dialog.querySelector("#setup-domain-restart");
    if (restart) restart.onclick = () => { reviewIndex = 0; draw("", "back"); };
    const retry = dialog.querySelector("#setup-scan-retry");
    if (retry) retry.onclick = () => void scanTabs();
    form.querySelectorAll("[data-remove-domain]").forEach(button => button.onclick = () => {
      draft.add_tab_domains = draft.add_tab_domains.filter(domain => domain !== button.dataset.removeDomain);
      draw();
    });
    // Put keyboard users at the question; Enter still submits the form.
    const heading = dialog.querySelector("h1");
    heading.tabIndex = -1;
    heading.focus();
  }

  function content() {
    if (step === 0) return `<h1 id="setup-title">A little setup.<br>A lighter workspace.</h1><p id="setup-description">Choose how you’d like to get started.</p><div class="setup-entry-cards">
      <button type="button" class="setup-entry-card" id="setup-account" aria-label="Sign up / sign in" aria-expanded="false" aria-controls="setup-account-status"><span class="setup-entry-art" aria-hidden="true"><svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="24" cy="16" r="7"/><path d="M10 39v-4a14 14 0 0 1 28 0v4"/></svg></span><span class="setup-card-title">Sign up / sign in</span><span class="setup-card-description">An account for paid features, when they’re ready.</span><span class="setup-card-trailing">Coming soon</span></button>
      <button type="submit" class="setup-entry-card setup-entry-free setup-next" aria-label="Continue free"><span class="setup-entry-art" aria-hidden="true"><img src="logo.png" width="48" height="48" alt=""></span><span class="setup-card-title">Continue free</span><span class="setup-card-description">Start monitoring on this device. Make autoTrim your own.</span><span class="setup-card-trailing">Get started <span aria-hidden="true">→</span></span></button>
      </div><p id="setup-account-status" class="setup-footnote" role="status" hidden>Sign-up and login are coming soon. Continue free for now; you can connect an account later.</p><p class="setup-footnote">Your activity and preferences stay on this device.</p>`;
    if (step === 1) return `<h1 id="setup-title">What brings you here?</h1><p id="setup-description">Pick what you’d like to keep an eye on. Choose as many as you like.</p><fieldset class="setup-focus-cards"><legend class="setup-sr">Your interests</legend>${areas.map(({ id, label, detail, apps, note }) => `<label class="setup-focus-card"><input type="checkbox" data-focus value="${id}" aria-label="${label}" ${draft.focus_areas.includes(id) ? "checked" : ""}>${appLogos(apps)}<span class="setup-card-title">${label}</span><span class="setup-card-description">${detail}</span><small>${note}</small></label>`).join("")}</fieldset><p class="setup-footnote">Your choices appear first in the sidebar. You can still see every app.</p>`;
    if (step === 2) return `<h1 id="setup-title">When is it time to tidy up?</h1><p id="setup-description">Choose when tabs and sessions count as stale. These settings guide autoTrim’s suggestions.</p><div class="setup-fields"><label><span>Browser tabs idle for</span><span><input aria-label="Browser tab idle hours" data-number="tab_stale_after_hours" type="number" min="0.25" max="8760" step="any" required value="${draft.tab_stale_after_hours}"> hours</span></label><label><span>AI sessions idle for</span><span><input aria-label="AI session idle hours" data-number="stale_after_hours" type="number" min="0.25" max="8760" step="any" required value="${draft.stale_after_hours}"> hours</span></label></div><label class="setup-option"><input type="checkbox" data-bool="notify" ${draft.notify ? "checked" : ""}><span><b>Notify me when there’s something to review</b><small>Reminders work while the background monitor is running.</small></span></label><p class="setup-footnote">${autoEnabled ? "Your existing automatic cleanup settings will stay enabled. Review them in Settings." : "Automatic cleanup stays off. You decide what to close; you can enable auto mode later in Settings."}</p>`;
    if (step === 3) return chromeContent();
    return `<h1 id="setup-title">When should autoTrim run?</h1><p id="setup-description">Keep the monitor ready at login, or open autoTrim when you need it.</p><fieldset class="setup-options"><legend class="setup-sr">Startup preference</legend><label class="setup-option ${!supported ? "unavailable" : ""}"><input type="radio" name="startup" value="background" ${draft.background ? "checked" : ""} ${!supported ? "disabled" : ""}><span><b>Always on, starting at login</b><small>${supported ? "Start the background monitor now and at every login. It keeps working after you close or quit the app." : "Background startup is currently available on macOS."}</small></span></label><label class="setup-option"><input type="radio" name="startup" value="manual" ${!draft.background ? "checked" : ""}><span><b>Open manually</b><small>Use live scans when you open autoTrim. Disable the login monitor; trends and reminders need background monitoring.</small></span></label></fieldset><label class="setup-window"><input type="checkbox" data-bool="open_window_at_launch" ${draft.open_window_at_launch ? "checked" : ""}> Show the dashboard when I launch the app</label>${selectedDomains().length ? `<p class="setup-save-summary">${selectedDomains().length} ${selectedDomains().length === 1 ? "domain" : "domains"} will be added to your auto-close whitelist. ${modeNote()}</p>` : ""}<p class="setup-footnote">The startup choice controls the background monitor. Open the menu bar app from Applications. You can change these choices in Settings.</p>`;
  }


  function modeNote() {
    return tabMode === "off" ? "Auto-close stays off until you enable it in Settings." : tabMode === "preview" ? "Preview mode stays on; tabs won’t close." : "Auto-close is already on. These rules take effect when you finish setup.";
  }

  function starterCount() {
    const count = selectedDomains().length;
    return count ? `${count} ${count === 1 ? "domain" : "domains"} selected. Saved when you finish setup.` : "No new domains selected. Choose only the sites you want autoTrim to close.";
  }

  function starterContent() {
    return `<h1 id="setup-title">Which Chrome tabs can we close?</h1><p id="setup-description">Start with searches and social feeds you don’t need to keep. Adding a site to your whitelist allows autoTrim to close its idle tabs.</p><p class="setup-footnote setup-mode-note">${modeNote()}</p>
      <div class="setup-new-tabs"><b>Empty new tabs are already covered</b><p>When Chrome auto-close is on, unused new-tab pages can close after the warning period, without waiting for the site inactivity timer. Pinned and selected tabs stay open.</p></div>
      <fieldset class="setup-starters"><legend>Sites to consider</legend><p>Close tabs after ${esc(tabHours < 1 ? `${Math.round(tabHours * 60)} minutes` : `${tabHours} ${tabHours === 1 ? "hour" : "hours"}`)} without selecting them, plus the warning period.</p>${starterSites.map(site => {
        const domains = starterDomains(site);
        const selected = domains.filter(domain => draft.add_tab_domains.includes(domain)).length;
        return `<label class="setup-starter"><input type="checkbox" data-starter="${site.id}" ${!domains.length || selected === domains.length ? "checked" : ""} ${!domains.length ? "disabled" : ""} ${selected && selected < domains.length ? "data-partial" : ""}><span><b>${site.label}</b>${!domains.length ? '<em>Already whitelisted</em>' : ""}<small>${site.detail}</small><small class="setup-starter-domains">${site.domains.join(", ")}</small></span></label>`;
      }).join("")}</fieldset>
      <p id="setup-starter-count" class="setup-footnote" role="status">${starterCount()}</p><div class="setup-review-entry"><div><b>Make suggestions from your tabs</b><p>Review other idle sites open in Chrome, one at a time.</p></div><button type="button" id="setup-review-tabs">Review my tabs</button></div><p class="setup-footnote">Site rules cover every page on the listed domains, including future tabs. Pinned, selected, and unknown-activity site tabs stay open. Unsaved drafts and background activity can’t be detected.</p>`;
  }

  function chromeContent() {
    if (!chromeReview) return starterContent();
    const intro = `<h1 id="setup-title">Which sites can we tidy up?</h1><p id="setup-description">Review idle Chrome tabs, one domain at a time. Whitelist a site to allow its tabs to close after ${esc(tabHours < 1 ? `${Math.round(tabHours * 60)} minutes` : `${tabHours} ${tabHours === 1 ? "hour" : "hours"}`)} of inactivity and the warning period.</p><p class="setup-footnote setup-mode-note">${modeNote()}</p>`;
    const protections = `<p class="setup-footnote">Pinned and selected tabs stay open, as do tabs with unknown activity. A rule applies to all pages on this exact domain, including future tabs. Unsaved work and background activity can’t be detected.</p>`;
    if (scanning) return `${intro}<div class="setup-scan" role="status"><span class="setup-scan-mark" aria-hidden="true">⌕</span><b>Reading your Chrome tabs…</b><p>Looking for tabs you haven’t selected in ${esc(tabHours < 1 ? `${Math.round(tabHours * 60)} minutes` : `${tabHours} ${tabHours === 1 ? "hour" : "hours"}`)}.</p></div>`;
    if (scanError) return `${intro}<div class="setup-scan"><b>Chrome tabs couldn’t be reviewed</b><p role="alert">${esc(scanError)}</p><button type="button" id="setup-scan-retry">Try again</button><p>You can continue setup and add domains in Settings later.</p></div>`;
    if (!suggestions?.length) return `${intro}<div class="setup-scan"><b>No new domains to suggest</b><p>Open Chrome tabs may be recent, protected, or already on your whitelist. You can add domains in Settings anytime.</p><button type="button" id="setup-scan-retry">Check again</button></div>${protections}`;
    const item = suggestions[reviewIndex];
    if (!item) return `<h1 id="setup-title">Your whitelist is ready to save.</h1><p id="setup-description">${selectedDomains().length ? "These domains will be added when you finish setup. Remove any you’d rather keep open." : "No new domains selected. You can add sites in Settings whenever you’re ready."}</p><p class="setup-footnote setup-mode-note">${modeNote()}</p><ul class="setup-domain-list">${selectedDomains().map(domain => `<li><b>${esc(domain)}</b><button type="button" data-remove-domain="${esc(domain)}" aria-label="Remove ${esc(domain)}">Remove</button></li>`).join("")}</ul><button type="button" id="setup-domain-restart">Review again</button>${protections}`;
    const added = draft.add_tab_domains.includes(item.domain);
    return `${intro}<section class="setup-domain-card" aria-label="Suggested domain"><div class="setup-domain-meta"><span>Domain ${reviewIndex + 1} of ${suggestions.length}</span><span>${item.tabs.length} idle ${item.tabs.length === 1 ? "tab" : "tabs"}</span></div><h2>${esc(item.domain)}</h2><p class="setup-domain-reason">Not selected for ${esc(dur(item.tabs[0].idle_secs))}${added ? " · Added to your draft" : ""}</p><ul class="setup-tab-list">${item.tabs.map(tab => `<li><span>${esc(tab.title || item.domain)}</span><small>${esc(tab.profile)} · Idle ${esc(dur(tab.idle_secs))}</small></li>`).join("")}</ul><div class="setup-domain-actions"><button type="button" id="setup-domain-skip">Skip this site</button><button type="button" class="primary" id="setup-domain-add">${added ? "Keep on whitelist" : "Add to whitelist"}</button></div></section><div class="setup-review-status"><span>${draft.add_tab_domains.length} added to your draft</span>${reviewIndex ? '<button type="button" id="setup-domain-previous">Previous site</button>' : ""}</div>${protections}`;
  }

  async function open(force = false) {
    if (opening || busy || dialog.open) return;
    opening = true;
    replay = force;
    try {
      const [settings, service] = await Promise.all([invoke("settings"), invoke("service_info")]);
      if (!settings || !service) throw new Error("Setup settings are unavailable. Try again.");
      if (settings.config_error) throw new Error(settings.config_error);
      if (!force && settings.onboarding_completed !== false) return;
      supported = service.supported;
      draft = {
        focus_areas: [...(settings.focus_areas || ["browser", "agent"])],
        stale_after_hours: (settings.stale_after_secs ?? 21600) / 3600,
        tab_stale_after_hours: (settings.tab_stale_after_secs ?? 86400) / 3600,
        notify: settings.notify ?? true,
        background: supported && (service.installed || !settings.onboarding_completed),
        open_window_at_launch: settings.open_window_at_launch ?? true,
        add_tab_domains: [],
        tab_rules_revision: settings.tab_rules_revision ?? "",
      };
      ++scanToken;
      suggestions = null;
      reviewIndex = 0;
      scanError = "";
      scanning = false;
      chromeReview = false;
      existingRules = settings.auto_tab_domains || [];
      tabHours = settings.auto_tab_inactive_hours ?? 24;
      tabMode = !settings.auto_close_tabs ? "off" : settings.auto_dry_run ? "preview" : "on";
      autoEnabled = settings.auto_close_sessions || settings.auto_stop_servers || settings.auto_close_tabs;
      step = force ? 1 : 0;
      draw();
      dialog.showModal();
      dialog.querySelector("h1").focus();
    } catch (e) {
      dialog.innerHTML = `<div class="setup-shell"><h1 id="setup-title">Setup couldn’t load</h1><p id="setup-description">${esc(String(e))}</p><button id="setup-retry">Try again</button><button id="setup-dismiss">Close</button></div>`;
      dialog.querySelector("#setup-retry").onclick = () => { dialog.close(); open(force); };
      dialog.querySelector("#setup-dismiss").onclick = () => dialog.close();
      dialog.showModal();
    } finally { opening = false; }
  }
  dialog.onclose = () => { ++scanToken; scanning = false; };
  dialog.oncancel = event => { if (busy) event.preventDefault(); };
  // Enter works from the heading too, without interfering with input/button behavior.
  dialog.onkeydown = event => {
    if (event.key === "Enter" && event.target.tagName === "H1") {
      event.preventDefault();
      dialog.querySelector("form")?.requestSubmit();
    }
  };
  window.addEventListener("autotrim-setup", () => open(true));
  return open();
}
