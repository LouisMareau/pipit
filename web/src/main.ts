import { registerSW } from "virtual:pwa-register";
import { loadSettings } from "./platform/storage";
import { App } from "./ui/app";
import "./ui/styles.css";

// The desktop app ships its own copy of the files; only the website needs a
// service worker for offline use and updates.
if (!("__TAURI_INTERNALS__" in window)) registerSW({ immediate: true });

const root = document.getElementById("app");
if (!root) throw new Error("missing #app");

loadSettings().then((settings) => new App(root, settings).start());
