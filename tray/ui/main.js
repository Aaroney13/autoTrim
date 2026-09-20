import { refresh, tickSync, renderAll } from "./render.js";
import { state } from "./state.js";
import { initializeOnboarding } from "./onboarding.js";

// Setup loads independently of a scan, so it is available immediately.
initializeOnboarding(async settings => {
  state.settings = settings;
  ++state.settingsRevision;
  if (state.snap) renderAll(true);
  await refresh();
});
refresh();
setInterval(tickSync, 1000);
