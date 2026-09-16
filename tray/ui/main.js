import { refresh, tickSync } from "./render.js";

refresh();
setInterval(tickSync, 1000);
