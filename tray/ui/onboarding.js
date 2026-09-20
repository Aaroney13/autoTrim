import { esc } from "./format.js";

const steps = ["Welcome", "Your focus", "Preferences", "Startup"];
const areas = [["browser", "Chrome & browser tabs", "Find tabs you haven’t visited in a while."], ["agent", "AI sessions", "Keep track of idle Claude, Codex, and other sessions."], ["app", "Apps & memory", "See which apps hold memory and how they grow."]];
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);

export function initializeOnboarding(onSaved) {
  const dialog = document.getElementById("onboarding");
  let step = 0, draft, supported = false, busy = false, opening = false, replay = false, autoEnabled = false;

  function draw(error = "") {
    dialog.innerHTML = `<form id="setup-form" class="setup-shell">
      <div class="setup-brand"><span class="setup-mark" aria-hidden="true"></span><b>autoTrim</b><button type="button" id="setup-later">${replay ? "Cancel" : "Set up later"}</button></div>
      <ol class="setup-progress" aria-label="Setup progress">${steps.map((label, i) => `<li ${i === step ? 'aria-current="step"' : ""} class="${i < step ? "done" : ""}"><span>${i < step ? "✓" : i + 1}</span>${label}</li>`).join("")}</ol>
      <div class="setup-page">${content()}</div>
      <p class="setup-error" id="setup-error" role="alert">${esc(error)}</p>
      <div class="setup-footer"><button type="button" id="setup-back" ${step === 0 ? "hidden" : ""}>Back</button><span>${step === 0 ? "Press Enter to continue free" : `Step ${step + 1} of ${steps.length}`}</span><button class="primary setup-next" type="submit">${step === 0 ? "Continue free" : step === 3 ? "Finish setup" : "Continue"}</button></div>
    </form>`;
    const form = dialog.querySelector("form");
    form.onsubmit = async event => {
      event.preventDefault();
      if (busy || !form.reportValidity()) return;
      if (step === 1 && !draft.focus_areas.length) {
        dialog.querySelector("#setup-error").textContent = "Choose at least one area to focus on.";
        return;
      }
      if (step < 3) { step++; draw(); return; }
      busy = true;
      dialog.setAttribute("aria-busy", "true");
      form.querySelectorAll("input, button").forEach(el => el.disabled = true);
      dialog.querySelector(".setup-next").textContent = "Saving…";
      let settings;
      try { settings = await invoke("complete_onboarding", { choices: draft }); }
      catch (e) { busy = false; dialog.removeAttribute("aria-busy"); draw(String(e)); return; }
      busy = false;
      dialog.removeAttribute("aria-busy");
      dialog.close();
      await onSaved(settings);
    };
    dialog.querySelector("#setup-back").onclick = () => { step--; draw(); };
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
    // Put keyboard users at the question; Enter still submits the form.
    const heading = dialog.querySelector("h1");
    heading.tabIndex = -1;
    heading.focus();
  }

  function content() {
    if (step === 0) return `<h1 id="setup-title">A little setup.<br>A lighter workspace.</h1><p id="setup-description">Let’s make autoTrim fit the way you work.</p><div class="setup-welcome"><span class="setup-mark large" aria-hidden="true"></span><div><h2>Start free. No account needed.</h2><p class="muted">Use autoTrim for free and set it up your way.<br>You can add an account later.</p></div></div><div class="setup-account"><div><h2>Looking for paid features?</h2><p>An account will give you access when they’re available.</p></div><button type="button" id="setup-account" aria-expanded="false" aria-controls="setup-account-status">Sign up / log in</button></div><p id="setup-account-status" class="setup-footnote" role="status" hidden>Sign-up and login are coming soon. Continue free for now; you can connect an account later.</p><p class="setup-footnote">Your activity and preferences stay on this device.</p>`;
    if (step === 1) return `<h1 id="setup-title">What brings you here?</h1><p id="setup-description">Pick what you’d like to keep an eye on. Choose as many as you like.</p><fieldset class="setup-options"><legend class="setup-sr">Your interests</legend>${areas.map(([id, label, detail]) => `<label class="setup-option"><input type="checkbox" data-focus value="${id}" ${draft.focus_areas.includes(id) ? "checked" : ""}><span><b>${label}</b><small>${detail}</small></span></label>`).join("")}</fieldset><p class="setup-footnote">Your choices appear first in the sidebar. You can still see every app.</p>`;
    if (step === 2) return `<h1 id="setup-title">When is it time to tidy up?</h1><p id="setup-description">Choose when tabs and sessions count as stale. These settings guide autoTrim’s suggestions.</p><div class="setup-fields"><label><span>Browser tabs idle for</span><span><input aria-label="Browser tab idle hours" data-number="tab_stale_after_hours" type="number" min="0.25" max="8760" step="any" required value="${draft.tab_stale_after_hours}"> hours</span></label><label><span>AI sessions idle for</span><span><input aria-label="AI session idle hours" data-number="stale_after_hours" type="number" min="0.25" max="8760" step="any" required value="${draft.stale_after_hours}"> hours</span></label></div><label class="setup-option"><input type="checkbox" data-bool="notify" ${draft.notify ? "checked" : ""}><span><b>Notify me when there’s something to review</b><small>Reminders work while the background monitor is running.</small></span></label><p class="setup-footnote">${autoEnabled ? "Your existing automatic cleanup settings will stay enabled. Review them in Settings." : "Automatic cleanup stays off. You decide what to close; you can enable auto mode later in Settings."}</p>`;
    return `<h1 id="setup-title">When should autoTrim run?</h1><p id="setup-description">Keep the monitor ready at login, or open autoTrim when you need it.</p><fieldset class="setup-options"><legend class="setup-sr">Startup preference</legend><label class="setup-option ${!supported ? "unavailable" : ""}"><input type="radio" name="startup" value="background" ${draft.background ? "checked" : ""} ${!supported ? "disabled" : ""}><span><b>Always on, starting at login</b><small>${supported ? "Start the background monitor now and at every login. It keeps working after you close or quit the app." : "Background startup is currently available on macOS."}</small></span></label><label class="setup-option"><input type="radio" name="startup" value="manual" ${!draft.background ? "checked" : ""}><span><b>Open manually</b><small>Use live scans when you open autoTrim. Disable the login monitor; trends and reminders need background monitoring.</small></span></label></fieldset><label class="setup-window"><input type="checkbox" data-bool="open_window_at_launch" ${draft.open_window_at_launch ? "checked" : ""}> Show the dashboard when I launch the app</label><p class="setup-footnote">The startup choice controls the background monitor. Open the menu bar app from Applications. You can change these choices in Settings.</p>`;
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
      };
      autoEnabled = settings.auto_close_sessions || settings.auto_stop_servers;
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
